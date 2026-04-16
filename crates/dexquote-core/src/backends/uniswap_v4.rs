//! Uniswap V4 Quoter backend.
//!
//! V4 moves to a singleton PoolManager and introduces Hook-enabled pools,
//! but the Quoter pattern is otherwise a close sibling of V3's: a
//! non-view function that reverts with the encoded quote result. Alloy's
//! generated `.call().await` does exactly that and decodes the tuple.
//!
//! For v0.5 we only race the four canonical `(fee, tickSpacing)` combos
//! with `hooks = address(0)` — same pattern as UniV3's fee-tier race.
//! Hook-enabled pools (dynamic fees, MEV protection, custom AMM logic)
//! are intentionally out of scope until a future release figures out how
//! to enumerate them.
//!
//! V4 requires `currency0 < currency1` in the PoolKey, so the backend
//! canonicalizes the token order and sets `zeroForOne` accordingly.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{
    address, aliases::{I24, U24}, Address, Bytes, U256,
};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "UniswapV4";

// Uniswap V4 Quoter on Ethereum mainnet.
// Source: https://docs.uniswap.org/contracts/v4/deployments
// Verified: `eth_getCode` against `ethereum.publicnode.com` returns the
// deployed Quoter bytecode.
const V4_QUOTER_ETHEREUM: Address = address!("52F0E24D1c21C8A0cB1e5a5dD6198556BD9E1203");

// Canonical V4 fee-tier / tickSpacing combinations with no hooks.
// These are the tiers the Uniswap frontend and router use by default.
// `(fee_bps, tick_spacing)` — matches V3's semantics 1:1.
const FEE_TIERS: [(u32, i32); 4] = [
    (100, 1),       // stablecoin / stablecoin
    (500, 10),      // volatile low
    (3000, 60),     // volatile mid
    (10000, 200),   // volatile high
];

// Typical V4 swap gas is comparable to V3's ~180k for a single-hop.
// Used as the floor when the quoter-reported gas is zero.
const GAS_ESTIMATE_V4: u64 = 180_000;

sol! {
    #[sol(rpc)]
    interface IUniswapV4Quoter {
        struct PoolKey {
            address currency0;
            address currency1;
            uint24 fee;
            int24 tickSpacing;
            address hooks;
        }

        struct QuoteExactSingleParams {
            PoolKey poolKey;
            bool zeroForOne;
            uint128 exactAmount;
            bytes hookData;
        }

        /// Non-view quoter call. Reverts internally with the encoded
        /// result on success, which alloy's static-call handling
        /// transparently decodes back into `(amountOut, gasEstimate)`.
        function quoteExactInputSingle(QuoteExactSingleParams memory params)
            external
            returns (uint256 amountOut, uint256 gasEstimate);
    }
}

pub struct UniswapV4Backend {
    ctx: OnChainContext,
}

impl UniswapV4Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            // V4 is currently mainnet-first. When V4 ships deeper L2
            // liquidity we add the other arms here.
            Chain::Ethereum => Some(V4_QUOTER_ETHEREUM),
            Chain::Arbitrum => None,
            Chain::Base => None,
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::quoter_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for UniswapV4Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = IUniswapV4Quoter::new(addr, self.ctx.provider.clone());

        // V4 PoolKey requires `currency0 < currency1`. Canonicalize the
        // pair and set the direction flag accordingly. `zero_for_one` is
        // true when the caller is selling the lower-address token.
        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;
        let (currency0, currency1, zero_for_one) = if token_in < token_out {
            (token_in, token_out, true)
        } else {
            (token_out, token_in, false)
        };

        // `exactAmount` is `uint128` in the V4 quoter ABI. Clamp for the
        // degenerate case of an absurdly large requested amount; no real
        // swap hits the u128 ceiling.
        let exact_amount: u128 = if request.amount_in > U256::from(u128::MAX) {
            u128::MAX
        } else {
            request.amount_in.to::<u128>()
        };

        // Fan out one call per fee-tier / tickSpacing combo, all with
        // `hooks = address(0)`. Every combo that corresponds to an
        // uninitialized PoolKey reverts and is silently dropped by
        // `.ok()`. We take the max non-zero amount across the survivors.
        let block_id = request.block_id;
        let calls = FEE_TIERS.iter().map(|&(fee, spacing)| {
            let pool_key = IUniswapV4Quoter::PoolKey {
                currency0,
                currency1,
                fee: U24::from(fee),
                tickSpacing: I24::try_from(spacing).unwrap_or(I24::ZERO),
                hooks: Address::ZERO,
            };
            let params = IUniswapV4Quoter::QuoteExactSingleParams {
                poolKey: pool_key,
                zeroForOne: zero_for_one,
                exactAmount: exact_amount,
                hookData: Bytes::new(),
            };
            let quoter = &quoter;
            async move {
                let call = quoter.quoteExactInputSingle(params);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call().await.ok()
            }
        });

        let results = join_all(calls).await;

        let mut best: Option<(U256, u64)> = None;
        for ret in results.into_iter().flatten() {
            let amount_out = ret.amountOut;
            if amount_out.is_zero() {
                continue;
            }
            let gas = saturating_to_u64(ret.gasEstimate).max(GAS_ESTIMATE_V4);
            best = Some(match best {
                Some((cur, cur_gas)) if cur >= amount_out => (cur, cur_gas),
                _ => (amount_out, gas),
            });
        }

        let (amount_out, gas_units) = best.ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(gas_units));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(gas_units),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}

fn saturating_to_u64(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to::<u64>()
    }
}
