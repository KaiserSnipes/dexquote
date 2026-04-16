//! Maverick V2 direct-pool quoting.
//!
//! Maverick V2 is a bin-based "Dynamic Distribution AMM" that concentrates
//! liquidity across tick ranges similar in spirit to Uniswap V3 but with
//! a different accounting model. Unlike UniV3 / Slipstream there is no
//! on-chain factory lookup that maps `(tokenA, tokenB) → pool`, so we
//! maintain a small hand-verified pool table per chain exactly like the
//! `CurveBackend` does.
//!
//! The Quoter contract exposes `calculateSwap` which simulates a swap
//! against a specific pool and returns the `(amountIn, amountOut,
//! gasEstimate)` tuple. We call it via `eth_call`; the function is
//! non-view but the Quoter handles the static-call semantics so alloy's
//! generated `.call()` resolves normally.
//!
//! Verification rule: every pool in the tables was confirmed live with
//! `pool.tokenA()` / `pool.tokenB()` matching the declared pair before
//! landing. No LLM-generated pool addresses.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use std::time::Instant;

const BACKEND_NAME: &str = "Maverick";

// Maverick V2 Quoter on Base.
// Source: https://docs.mav.xyz/technical-reference/contract-addresses/v2-contract-addresses
// Verified: 11,954 bytes of contract code at this address.
const MAVERICK_V2_QUOTER_BASE: Address =
    address!("b40AfdB85a07f37aE217E7D6462e609900dD8D7A");

// Maverick V2 Quoter on Arbitrum.
// Same CREATE2 deterministic address as Base — the Maverick factory is
// deployed cross-chain with identical init-code so the Quoter lands at
// the same address on every supported EVM. Verified live via
// `eth_getCode arb1.arbitrum.io` (returns non-empty bytecode).
const MAVERICK_V2_QUOTER_ARBITRUM: Address =
    address!("b40AfdB85a07f37aE217E7D6462e609900dD8D7A");

// Maverick V2 Quoter on Ethereum mainnet.
// Same CREATE2 address again. Verified live via
// `eth_getCode ethereum.publicnode.com`.
const MAVERICK_V2_QUOTER_ETHEREUM: Address =
    address!("b40AfdB85a07f37aE217E7D6462e609900dD8D7A");

// Typical Maverick V2 swap gas: bin-based math is cheaper than UniV3
// ticks but more expensive than flat-curve AMMs. 180k is a reasonable
// midpoint for the USD-gas display.
const GAS_ESTIMATE_MAV: u64 = 180_000;

/// A single Maverick V2 pool with its two tokens in canonical order
/// (tokenA first, tokenB second, matching `pool.tokenA()` /
/// `pool.tokenB()` as reported on-chain).
struct PoolSpec {
    pool: Address,
    token_a: Address,
    token_b: Address,
}

/// Base pools. Every entry verified via `pool.tokenA()` /
/// `pool.tokenB()` against `mainnet.base.org` at implementation time.
const BASE_POOLS: &[PoolSpec] = &[
    // Maverick V2 WETH/USDC, 0.008% fee tier.
    // Source: https://www.geckoterminal.com/base/pools/0x1b433fe555af6016a48ce82548ed77849b9832d8
    // Verified: tokenA = WETH (0x4200…0006), tokenB = USDC (0x8335…2913).
    PoolSpec {
        pool: address!("1b433fE555af6016A48cE82548Ed77849B9832d8"),
        token_a: address!("4200000000000000000000000000000000000006"),
        token_b: address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"),
    },
];

/// Arbitrum pools. Maverick V2 is primarily a stableswap venue on
/// Arbitrum — the platform's WETH-paired pools all sit below $20k TVL so
/// they're not worth the RPC budget. This table exists so that USDC/USDT
/// stablecoin routes have a Maverick comparison point; for WETH-paired
/// routes the Maverick backend will simply return `NoRoute` and the
/// filter-out logic in `supports()` plus the render layer hide it.
///
/// Every entry verified via `pool.tokenA()` / `pool.tokenB()` against
/// `arb1.arbitrum.io` at implementation time.
const ARBITRUM_POOLS: &[PoolSpec] = &[
    // Maverick V2 USDC/USDT, 0.001% fee tier. ~$67k TVL at add time.
    // Source: https://www.geckoterminal.com/arbitrum/pools/0x713e1346d585a1dccadc093210e33cd6bc8cf3d1
    // Verified: tokenA = USDC native (0xaf88…5831), tokenB = USDT (0xFd08…cbb9).
    PoolSpec {
        pool: address!("713e1346d585a1dccadc093210e33cd6bc8cf3d1"),
        token_a: address!("af88d065e77c8cC2239327C5EDb3A432268e5831"),
        token_b: address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"),
    },
];

/// Ethereum mainnet pools. Maverick V2 on mainnet is extremely thin:
/// out of 51 deployed pools only one sits above $20k TVL, and the
/// priority pairs (WETH/USDC, USDC/USDT, WETH/DAI) are all sub-$12k.
/// Factory enumeration at `0x0A7e848Aca42d879EF06507Fca0E7b33A0a63c1e`
/// (via `lookup(uint256, uint256)`) plus on-chain `tokenA()/tokenB()`
/// verification against `ethereum.publicnode.com` confirmed the single
/// pool below at implementation time.
const ETHEREUM_POOLS: &[PoolSpec] = &[
    // Maverick V2 wstETH/WETH, 0.01% fee tier. ~$61k TVL at add time.
    // Source: https://www.geckoterminal.com/eth/pools/0x68875ad4dc276527790b3a80d397e04abf44344c
    // Verified: tokenA = wstETH (0x7f39…2Ca0), tokenB = WETH (0xC02a…6Cc2).
    PoolSpec {
        pool: address!("68875ad4dc276527790b3a80d397e04abf44344c"),
        token_a: address!("7f39C581F595B53c5cb19bD0b3f8dA6c935E2Ca0"),
        token_b: address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"),
    },
];

sol! {
    #[sol(rpc)]
    interface IMaverickV2Quoter {
        /// Simulate a swap against `pool`. `tokenAIn = true` means the
        /// caller is sending tokenA and receiving tokenB. `exactOutput`
        /// flips the amount semantics. `tickLimit` bounds how far the
        /// swap is allowed to walk through bins — `int32::MAX` for
        /// no practical limit.
        function calculateSwap(
            address pool,
            uint128 amount,
            bool tokenAIn,
            bool exactOutput,
            int32 tickLimit
        ) external returns (uint256 amountIn, uint256 amountOut, uint256 gasEstimate);
    }
}

pub struct MaverickV2Backend {
    ctx: OnChainContext,
}

impl MaverickV2Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Base => Some(MAVERICK_V2_QUOTER_BASE),
            Chain::Arbitrum => Some(MAVERICK_V2_QUOTER_ARBITRUM),
            Chain::Ethereum => Some(MAVERICK_V2_QUOTER_ETHEREUM),
            Chain::Solana => None,
        }
    }

    fn pools_for(chain: Chain) -> &'static [PoolSpec] {
        match chain {
            Chain::Base => BASE_POOLS,
            Chain::Arbitrum => ARBITRUM_POOLS,
            Chain::Ethereum => ETHEREUM_POOLS,
            Chain::Solana => &[],
        }
    }

    pub fn supports(chain: Chain) -> bool {
        // Both conditions must hold: a quoter to talk to AND at least one
        // pool to talk about. An empty pool table filters Maverick out of
        // every quote on that chain.
        Self::quoter_address(chain).is_some() && !Self::pools_for(chain).is_empty()
    }
}

#[async_trait::async_trait]
impl DexBackend for MaverickV2Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let quoter_addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        // Find a pool containing both tokens.
        let (pool, token_a_in) = find_pool(
            Self::pools_for(request.chain),
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
        )
        .ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        // Maverick's `amount` is `uint128`. Clamp gracefully if the
        // caller passes something absurd — no real swap hits u128::MAX.
        let amount_u128: u128 = if request.amount_in > U256::from(u128::MAX) {
            u128::MAX
        } else {
            request.amount_in.to::<u128>()
        };

        let quoter = IMaverickV2Quoter::new(quoter_addr, self.ctx.provider.clone());

        // tick_limit = i32::MAX means "walk as far as needed to fill the
        // order". Matches the Maverick UI's default behaviour.
        // Maverick V2 tickLimit is the bin tick where the swap halts.
        // Per docs (https://docs.mav.xyz/technical-reference/maverick-v2/v2-contracts/maverick-v2-supplemental-contracts/maverickv2quoter):
        //   tokenAIn = true  → use type(int32).max for unbounded
        //   tokenAIn = false → use type(int32).min for unbounded
        // The earlier ±443_636 attempt had the sign flipped from the docs.
        let tick_limit: i32 = if token_a_in { i32::MAX } else { i32::MIN };

        let call = quoter.calculateSwap(pool.pool, amount_u128, token_a_in, false, tick_limit);
        let call = match request.block_id {
            Some(id) => call.block(id),
            None => call,
        };
        let ret = call.call().await.map_err(|_| DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let amount_out = ret.amountOut;
        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Prefer the quoter-reported gas estimate when present, falling
        // back to our flat estimate. Real Maverick gas hovers between
        // 120k–300k depending on how many bins are crossed.
        let gas_units = if ret.gasEstimate.is_zero() {
            GAS_ESTIMATE_MAV
        } else {
            saturating_to_u64(ret.gasEstimate).max(GAS_ESTIMATE_MAV)
        };

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

/// Find a pool that matches the requested pair. Returns the pool spec
/// plus a `token_a_in` flag indicating whether the caller is paying in
/// tokenA (true) or tokenB (false).
fn find_pool(
    pools: &'static [PoolSpec],
    token_in: Address,
    token_out: Address,
) -> Option<(&'static PoolSpec, bool)> {
    for pool in pools {
        if pool.token_a == token_in && pool.token_b == token_out {
            return Some((pool, true));
        }
        if pool.token_b == token_in && pool.token_a == token_out {
            return Some((pool, false));
        }
    }
    None
}

fn saturating_to_u64(value: U256) -> u64 {
    if value > U256::from(u64::MAX) {
        u64::MAX
    } else {
        value.to::<u64>()
    }
}
