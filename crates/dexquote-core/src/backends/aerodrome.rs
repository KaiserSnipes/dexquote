//! Aerodrome on Base.
//!
//! Aerodrome is the dominant Base DEX (Solidly fork with vote-escrowed
//! emissions) and the highest-liquidity venue for most Base pairs. The
//! router exposes a Solidly-style `getAmountsOut(uint256, Route[])`
//! function where each `Route` is `(from, to, stable, factory)` — a swap
//! can mix stable and volatile pools along the same path.
//!
//! Our quoting strategy mirrors the aggregator approach: try the direct
//! pair in both "stable" and "volatile" modes, plus a WETH-bridged two-hop
//! route when neither side is WETH, and take the maximum. The four
//! candidate routes fire in parallel, so the added work is latency-bounded
//! by the slowest single RPC call.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "Aerodrome";

// Aerodrome Router + default Factory on Base.
// Source: https://aerodrome.finance/security (Router + Factory listed) and
// confirmed via eth_getCode that both addresses are live contracts.
const AERODROME_ROUTER_BASE: Address =
    address!("cF77a3Ba9A5CA399B7c97c74d54e5b1Beb874E43");
const AERODROME_FACTORY_BASE: Address =
    address!("420DD381b31aEf6683db6B902084cB0FFECe40Da");

// Canonical Base WETH (the OP-stack predeploy at 0x42…0006). Used as the
// bridge token for WETH-through multi-hop quotes.
const WETH_BASE: Address = address!("4200000000000000000000000000000000000006");

// Conservative gas estimate for a typical Aerodrome swap. Real swaps are
// 150–300k depending on whether the pool is stable (straight-line curve)
// or volatile (xy=k). Good enough for the USD display column.
const GAS_ESTIMATE_AERO: u64 = 200_000;

sol! {
    #[sol(rpc)]
    interface IAerodromeRouter {
        struct Route {
            address from;
            address to;
            bool stable;
            address factory;
        }

        function getAmountsOut(uint256 amountIn, Route[] memory routes)
            external
            view
            returns (uint256[] memory amounts);
    }
}

pub struct AerodromeBackend {
    ctx: OnChainContext,
}

impl AerodromeBackend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn router_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Base => Some(AERODROME_ROUTER_BASE),
            // Aerodrome is Base-native; no deployment elsewhere.
            Chain::Arbitrum => None,
            Chain::Ethereum => None,
            Chain::Solana => None,
        }
    }

    fn factory_for(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Base => Some(AERODROME_FACTORY_BASE),
            Chain::Arbitrum => None,
            Chain::Ethereum => None,
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::router_address(chain).is_some()
    }

    fn weth_for(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Base => Some(WETH_BASE),
            Chain::Arbitrum => None,
            Chain::Ethereum => None,
            Chain::Solana => None,
        }
    }
}

#[async_trait::async_trait]
impl DexBackend for AerodromeBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let router_addr = Self::router_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let factory = Self::factory_for(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let weth = Self::weth_for(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let router = IAerodromeRouter::new(router_addr, self.ctx.provider.clone());

        // Build the candidate route set. Each `Route[]` slice describes a
        // complete path; within a path each hop specifies its own pool
        // variant (stable or volatile).
        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;

        let mut candidates: Vec<Vec<IAerodromeRouter::Route>> = Vec::with_capacity(4);

        // Direct pair, volatile pool.
        candidates.push(vec![IAerodromeRouter::Route {
            from: token_in,
            to: token_out,
            stable: false,
            factory,
        }]);
        // Direct pair, stable pool — cheap to try, wildly better for
        // actual stablecoin pairs.
        candidates.push(vec![IAerodromeRouter::Route {
            from: token_in,
            to: token_out,
            stable: true,
            factory,
        }]);

        // Two-hop through WETH when neither side is WETH. We try both
        // hops as volatile, which is the common case for non-stable
        // altcoin pairs on Aerodrome.
        if token_in != weth && token_out != weth {
            candidates.push(vec![
                IAerodromeRouter::Route {
                    from: token_in,
                    to: weth,
                    stable: false,
                    factory,
                },
                IAerodromeRouter::Route {
                    from: weth,
                    to: token_out,
                    stable: false,
                    factory,
                },
            ]);
        }

        // Fire every candidate concurrently. `getAmountsOut` reverts when
        // no pool exists for a given (from, to, stable) tuple, so we
        // tolerate per-candidate errors and only fail the whole quote if
        // every candidate fell over.
        let block_id = request.block_id;
        let calls = candidates.into_iter().map(|routes| {
            let router = &router;
            let amount_in = request.amount_in;
            async move {
                let call = router.getAmountsOut(amount_in, routes);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call().await.ok().and_then(|ret| ret.last().copied())
            }
        });

        let results = join_all(calls).await;

        let amount_out = results
            .into_iter()
            .flatten()
            .filter(|a| !a.is_zero())
            .max()
            .ok_or(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            })?;

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_AERO));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_AERO),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}
