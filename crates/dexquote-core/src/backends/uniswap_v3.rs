//! UniswapV3 QuoterV2 backend for Arbitrum.
//!
//! QuoterV2 is marked non-view — it reverts with the encoded result — but is
//! callable via `eth_call` as long as we don't broadcast. Alloy's generated
//! `.call().await` does exactly that and decodes the returned tuple.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use crate::token::Token;
use alloy::primitives::{address, aliases::U24, Address, U256};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "UniswapV3";

// Uniswap V3 QuoterV2 addresses per chain.
// Arbitrum: https://docs.uniswap.org/contracts/v3/reference/deployments/arbitrum-deployments
// Base:     https://docs.uniswap.org/contracts/v3/reference/deployments/base-deployments
// Ethereum: https://docs.uniswap.org/contracts/v3/reference/deployments/ethereum-deployments
// Note: mainnet uses the same deterministic address as Arbitrum.
const QUOTER_V2_ARBITRUM: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
const QUOTER_V2_BASE: Address = address!("3d4e44Eb1374240CE5F1B871ab261CD16335B76a");
const QUOTER_V2_ETHEREUM: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");

// Fee tiers queried in order. The 100-bip tier is only used when both
// tokens are stablecoin-like (USDC/USDC.e/USDT/DAI).
const FEE_TIERS_STANDARD: [u32; 3] = [500, 3000, 10000];
const FEE_TIER_STABLE: u32 = 100;

sol! {
    #[sol(rpc)]
    interface IQuoterV2 {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            uint24 fee;
            uint160 sqrtPriceLimitX96;
        }

        function quoteExactInputSingle(QuoteExactInputSingleParams memory params)
            external
            returns (
                uint256 amountOut,
                uint160 sqrtPriceX96After,
                uint32 initializedTicksCrossed,
                uint256 gasEstimate
            );
    }
}

pub struct UniswapV3Backend {
    ctx: OnChainContext,
}

impl UniswapV3Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Arbitrum => Some(QUOTER_V2_ARBITRUM),
            Chain::Base => Some(QUOTER_V2_BASE),
            Chain::Ethereum => Some(QUOTER_V2_ETHEREUM),
            Chain::Solana => None,
        }
    }

    /// Whether this backend has contracts on the given chain. Used by
    /// `build_backends` to filter out unsupported combinations at
    /// construction time rather than letting them surface as NoRoute rows.
    pub fn supports(chain: Chain) -> bool {
        Self::quoter_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for UniswapV3Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = IQuoterV2::new(addr, self.ctx.provider.clone());

        let fee_tiers: Vec<u32> = {
            let mut tiers = FEE_TIERS_STANDARD.to_vec();
            if is_stable_pair(&request.token_in, &request.token_out) {
                tiers.insert(0, FEE_TIER_STABLE);
            }
            tiers
        };

        let block_id = request.block_id;
        let calls = fee_tiers.iter().map(|&fee| {
            let params = IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: request.token_in.evm_address(BACKEND_NAME).unwrap_or_default(),
                tokenOut: request.token_out.evm_address(BACKEND_NAME).unwrap_or_default(),
                amountIn: request.amount_in,
                fee: U24::from(fee),
                sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
            };
            let quoter = &quoter;
            async move {
                let call = quoter.quoteExactInputSingle(params);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call().await
            }
        });

        let results = join_all(calls).await;

        let mut best: Option<(U256, u64)> = None;
        for ret in results.into_iter().flatten() {
            let amount_out = ret.amountOut;
            let gas = saturating_to_u64(ret.gasEstimate);
            if amount_out.is_zero() {
                continue;
            }
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

fn is_stable_pair(a: &Token, b: &Token) -> bool {
    is_stable(&a.symbol) && is_stable(&b.symbol)
}

fn is_stable(symbol: &str) -> bool {
    matches!(
        symbol.to_ascii_uppercase().as_str(),
        "USDC" | "USDC.E" | "USDT" | "DAI"
    )
}

fn saturating_to_u64(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to::<u64>()
    }
}
