//! Aerodrome Slipstream on Base.
//!
//! Slipstream is Aerodrome's concentrated-liquidity fork of Uniswap V3.
//! The Quoter ABI is nearly identical to Uniswap's QuoterV2, with one key
//! difference: pools are keyed by `tickSpacing` (Solidly convention) rather
//! than by a `fee` tier. We try the common tick spacings in parallel for
//! the direct pair, AND fire multi-hop routes through canonical bridge
//! tokens (WETH / USDC) so pairs like DEGEN → USDC that need a two-hop
//! path still resolve.
//!
//! This backend complements the existing `AerodromeBackend` (classic
//! volatile/stable factory pools). Together they cover the entire
//! Aerodrome protocol surface: v1 pools for stable pairs and deep legacy
//! liquidity, Slipstream for concentrated memecoin and alt liquidity
//! (DEGEN / BRETT / TOSHI / etc).

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, aliases::I24, Address, Bytes, U256};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "Slipstream";

// Aerodrome Slipstream QuoterV2 on Base.
// Source: https://github.com/aerodrome-finance/slipstream and
//         basescan.org — confirmed 13,870 bytes of contract code live.
const SLIPSTREAM_QUOTER_BASE: Address =
    address!("254cF9E1E6E233aa1AC962CB9B05b2cfeAae15b0");

// Tick spacings used by Aerodrome Slipstream pools. Slipstream allows
// arbitrary integers but pools are deployed at a small set of conventional
// values: 1 (stable/stable), 50 (tight volatile), 100 (~0.3% fee tier
// equivalent), 200 (1% equivalent), 2000 (memecoin wide). We also try 500
// and 1000 because some meme and alt pairs on Aerodrome have been
// deployed at non-standard intermediate spacings.
const TICK_SPACINGS: [i32; 7] = [1, 50, 100, 200, 500, 1000, 2000];

// Tick spacing pairs attempted for two-hop paths. Fewer combinations
// than the direct fanout because the combinatorial explosion otherwise
// blows the RPC budget: 4 combos × up to 2 bridge tokens = 8 calls max.
// The combos were picked to cover common cases — tight/tight for
// stablecoin hops, wide/wide for memecoin hops, mixed for asymmetric
// pairs.
const MULTIHOP_SPACINGS: [(i32, i32); 4] = [(100, 100), (200, 200), (2000, 2000), (100, 200)];

// Tick spacing triples attempted for three-hop paths. Bounded to two
// combinations to keep the candidate set sane: a wide/wide/wide
// combination for memecoin → WETH → USDC → stablecoin paths, and a
// medium combination for cleaner intermediate hops. Three-hop is the
// fallback for tokens that have no direct pool with either bridge — the
// canonical case is `MEME → WETH → USDC → STABLE`.
const THREEHOP_SPACINGS: [(i32, i32, i32); 2] =
    [(2000, 100, 100), (200, 100, 100)];

// Canonical bridge tokens on Base. WETH covers memecoin-through-ETH
// paths; USDC covers stablecoin-adjacent paths. Kept deliberately small
// so multi-hop fanout stays bounded.
const BRIDGE_TOKENS_BASE: &[Address] = &[
    address!("4200000000000000000000000000000000000006"), // WETH (Base canonical)
    address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC (Base native Circle)
];

// Slipstream swap cost is comparable to UniV3 (~150–200k gas). Flat
// estimate keeps the USD-gas display consistent. Multi-hop swaps cost
// more (~250–350k), so the returned `gasEstimate` from the quoter takes
// precedence when available.
const GAS_ESTIMATE_SLIPSTREAM: u64 = 180_000;

sol! {
    #[sol(rpc)]
    interface ISlipstreamQuoter {
        struct QuoteExactInputSingleParams {
            address tokenIn;
            address tokenOut;
            uint256 amountIn;
            int24 tickSpacing;
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

        /// Multi-hop quote. `path` is packed as
        /// `token(20) | tickSpacing(3) | token(20) | tickSpacing(3) | …`
        /// identical binary layout to Uniswap V3's `quoteExactInput` (the
        /// 3-byte tickSpacing field fits the same slot as V3's uint24 fee).
        function quoteExactInput(bytes memory path, uint256 amountIn)
            external
            returns (
                uint256 amountOut,
                uint160[] memory sqrtPriceX96AfterList,
                uint32[] memory initializedTicksCrossedList,
                uint256 gasEstimate
            );
    }
}

/// Encode a Slipstream multi-hop path as raw bytes.
///
/// Layout: `token0 || tickSpacing0 || token1 || tickSpacing1 || … || tokenN`
/// with each address taking 20 bytes and each tickSpacing taking 3 bytes
/// big-endian. For an N-hop path there are N tickSpacings and N+1 tokens
/// (so bytes length = `20*(N+1) + 3*N`).
///
/// `hops` is `[(token_before_hop, tickSpacing_of_hop), …]` plus a final
/// `token_out`. For a two-hop `A → B → C` at spacings `s1, s2` the caller
/// passes `hops = &[(A, s1), (B, s2)]` and `final_token = C`.
///
/// This encoder uses only the low 24 bits of the signed tickSpacing;
/// negative tick spacings aren't used by any deployed Slipstream pool,
/// but the encoding matches what V3 expects for compatibility.
fn encode_v3_path(hops: &[(Address, i32)], final_token: Address) -> Bytes {
    let mut out = Vec::with_capacity(20 * (hops.len() + 1) + 3 * hops.len());
    for (token, spacing) in hops {
        out.extend_from_slice(token.as_slice());
        let raw = *spacing as u32;
        out.push(((raw >> 16) & 0xff) as u8);
        out.push(((raw >> 8) & 0xff) as u8);
        out.push((raw & 0xff) as u8);
    }
    out.extend_from_slice(final_token.as_slice());
    Bytes::from(out)
}

fn bridge_tokens_for(chain: Chain) -> &'static [Address] {
    match chain {
        Chain::Base => BRIDGE_TOKENS_BASE,
        Chain::Arbitrum => &[],
        Chain::Ethereum => &[],
        Chain::Solana => &[],
    }
}

pub struct SlipstreamBackend {
    ctx: OnChainContext,
}

impl SlipstreamBackend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Base => Some(SLIPSTREAM_QUOTER_BASE),
            Chain::Arbitrum => None,
            Chain::Ethereum => None,
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::quoter_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for SlipstreamBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = ISlipstreamQuoter::new(addr, self.ctx.provider.clone());

        // Fanout is broken into two concurrent groups that share the same
        // `join_all`: direct single-hop at every tickSpacing, plus multi-hop
        // paths through each bridge token at the reduced combo set. Every
        // candidate is folded into the same "best amount" at the end, so
        // the backend simply returns whichever route paid out most.
        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;
        let amount_in = request.amount_in;

        let block_id = request.block_id;

        // ---- Single-hop candidates ----
        let single_calls = TICK_SPACINGS.iter().map(|&spacing| {
            let params = ISlipstreamQuoter::QuoteExactInputSingleParams {
                tokenIn: token_in,
                tokenOut: token_out,
                amountIn: amount_in,
                tickSpacing: I24::try_from(spacing).unwrap_or(I24::ZERO),
                sqrtPriceLimitX96: alloy::primitives::Uint::<160, 3>::ZERO,
            };
            let quoter = &quoter;
            async move {
                let call = quoter.quoteExactInputSingle(params);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call()
                    .await
                    .ok()
                    .map(|ret| (ret.amountOut, saturating_to_u64(ret.gasEstimate)))
            }
        });

        // ---- Multi-hop candidates ----
        // Two-hop: for each bridge token that isn't already an endpoint,
        // try the reduced combo set as `in → bridge → out`.
        // Three-hop: for ordered bridge pairs (WETH→USDC and USDC→WETH),
        // try a smaller combo set as `in → bridge1 → bridge2 → out`.
        // Skip any path containing an endpoint as an intermediate hop.
        let mut multi_paths: Vec<Bytes> = Vec::new();
        let bridges = bridge_tokens_for(request.chain);
        for bridge in bridges {
            if *bridge == token_in || *bridge == token_out {
                continue;
            }
            for (s1, s2) in MULTIHOP_SPACINGS {
                let path = encode_v3_path(&[(token_in, s1), (*bridge, s2)], token_out);
                multi_paths.push(path);
            }
        }
        // Three-hop: enumerate ordered (bridge1, bridge2) pairs.
        for b1 in bridges {
            if *b1 == token_in || *b1 == token_out {
                continue;
            }
            for b2 in bridges {
                if *b2 == *b1 || *b2 == token_in || *b2 == token_out {
                    continue;
                }
                for (s1, s2, s3) in THREEHOP_SPACINGS {
                    let path = encode_v3_path(
                        &[(token_in, s1), (*b1, s2), (*b2, s3)],
                        token_out,
                    );
                    multi_paths.push(path);
                }
            }
        }

        let multi_calls = multi_paths.into_iter().map(|path| {
            let quoter = &quoter;
            async move {
                let call = quoter.quoteExactInput(path, amount_in);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call()
                    .await
                    .ok()
                    .map(|ret| (ret.amountOut, saturating_to_u64(ret.gasEstimate)))
            }
        });

        // ---- Merge ----
        // `join_all` accepts a homogeneous future iterator, so we collect
        // both groups into Vec<BoxFuture<...>> and drive them together.
        // BoxFuture avoids the type mismatch between the two async blocks.
        let mut all: Vec<futures::future::BoxFuture<'_, Option<(U256, u64)>>> =
            Vec::new();
        for f in single_calls {
            all.push(Box::pin(f));
        }
        for f in multi_calls {
            all.push(Box::pin(f));
        }
        let results = join_all(all).await;

        // Fold every successful route into the best amount. Failed routes
        // (pool doesn't exist, slippage limits hit, etc.) silently skip.
        let mut best: Option<(U256, u64)> = None;
        for (amount_out, gas) in results.into_iter().flatten() {
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
            .map(|p| p.gas_units_to_usd(gas_units.max(GAS_ESTIMATE_SLIPSTREAM)));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(gas_units.max(GAS_ESTIMATE_SLIPSTREAM)),
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
