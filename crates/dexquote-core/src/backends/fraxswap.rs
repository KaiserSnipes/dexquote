//! FraxSwap V2 backend for Ethereum mainnet.
//!
//! FraxSwap V2 is a UniswapV2 fork with TWAMM (Time-Weighted Average
//! Market Maker) order execution on top — users can submit long-running
//! orders that the contract drips through liquidity over time.
//!
//! **TWAMM quote caveat:** the Router's `getAmountsOut` only returns a
//! quote when the TWAMM oracle state is fresh. After a period of
//! inactivity the contract reverts with `"twamm out of date"` and the
//! backend returns `NoRoute`. This makes FraxSwap a "conditional"
//! backend — it quotes during active periods and sits silent otherwise.
//! Users who need a reliable FRAX quote should rely on Curve (which
//! already covers FRAX via its stable pools) as the primary backend,
//! and treat FraxSwap as a bonus when it happens to be fresh.
//!
//! The Router ABI is otherwise byte-compatible with UniswapV2's
//! `getAmountsOut`.
//!
//! Mainnet-only for v0.6. FraxSwap also exists on Arbitrum but with much
//! thinner liquidity — the Arbitrum routes already have Camelot, Curve,
//! and Balancer covering the FRAX-adjacent pairs.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "FraxSwap";

// FraxSwap Router V2 address on Ethereum mainnet.
// Source: https://docs.frax.finance/smart-contracts/fraxswap
// Verified: `eth_getCode` against `ethereum.publicnode.com` returns the
// deployed router bytecode.
const FRAXSWAP_ROUTER_ETHEREUM: Address =
    address!("C14d550632db8592D1243Edc8B95b0Ad06703867");

// Hub tokens for intermediate hops. FRAX is the natural hub for the
// Frax ecosystem; WETH catches anything that doesn't pair directly
// against FRAX.
const ETHEREUM_HUBS: &[Address] = &[
    address!("853d955aCEf822Db058eb8505911ED77F175b99e"), // FRAX
    address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH
];

const GAS_ESTIMATE_FRAXSWAP: u64 = 160_000;

sol! {
    #[sol(rpc)]
    interface IFraxSwapRouter {
        function getAmountsOut(uint256 amountIn, address[] path)
            external view returns (uint256[] amounts);
    }
}

pub struct FraxSwapBackend {
    ctx: OnChainContext,
}

impl FraxSwapBackend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn router_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Ethereum => Some(FRAXSWAP_ROUTER_ETHEREUM),
            Chain::Arbitrum => None,
            Chain::Base => None,
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::router_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for FraxSwapBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::router_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let router = IFraxSwapRouter::new(addr, self.ctx.provider.clone());

        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;

        let mut paths: Vec<Vec<Address>> = vec![vec![token_in, token_out]];
        for hub in ETHEREUM_HUBS {
            if *hub == token_in || *hub == token_out {
                continue;
            }
            paths.push(vec![token_in, *hub, token_out]);
        }

        let block_id = request.block_id;
        let calls = paths.into_iter().map(|path| {
            let router = &router;
            async move {
                let call = router.getAmountsOut(request.amount_in, path);
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
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_FRAXSWAP));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_FRAXSWAP),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}
