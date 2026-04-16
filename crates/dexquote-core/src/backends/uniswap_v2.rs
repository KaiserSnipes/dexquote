//! SushiSwap V2 (UniswapV2-style) backend for Arbitrum.
//!
//! Tries the direct path `[tokenIn, tokenOut]` and, if neither side is WETH,
//! also tries the bridged path `[tokenIn, WETH, tokenOut]`. Returns the
//! better of the two. Gas is estimated at a flat 150k units per swap —
//! UniswapV2 router calls are simple and well-bounded so a fixed estimate is
//! accurate enough for the on-screen USD display.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "SushiV2";

// SushiSwap V2 router addresses per chain.
// Arbitrum: https://arbiscan.io/address/0x1b02da8cb0d097eb8d57a175b88c7d8b47997506
// Base:     https://basescan.org/address/0x6BDED42c6DA8FBf0d2bA55B2fa120C5e0c8D7891
// Ethereum: https://etherscan.io/address/0xd9e1cE17f2641f24aE83637ab66a2cca9C378B9F
const SUSHI_V2_ROUTER_ARBITRUM: Address =
    address!("1b02dA8Cb0d097eB8D57A175b88c7D8b47997506");
const SUSHI_V2_ROUTER_BASE: Address =
    address!("6BDED42c6DA8FBf0d2bA55B2fa120C5e0c8D7891");
const SUSHI_V2_ROUTER_ETHEREUM: Address =
    address!("d9e1cE17f2641f24aE83637ab66a2cca9C378B9F");

// Hub tokens to try as intermediate hops. SushiV2 on Arbitrum historically
// built pools against USDC.e rather than native USDC, so routing through it
// recovers quotes for pairs the direct path can't find. Base is newer so
// the dominant hub is plain WETH. Ethereum mainnet uses the canonical
// WETH + USDC pair as hubs.
const ARBITRUM_HUBS: &[Address] = &[
    address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"), // WETH
    address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"), // USDC.e
];
const BASE_HUBS: &[Address] = &[
    address!("4200000000000000000000000000000000000006"), // WETH (Base canonical)
];
const ETHEREUM_HUBS: &[Address] = &[
    address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH (mainnet)
    address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), // USDC (mainnet)
];

const GAS_ESTIMATE_V2_SWAP: u64 = 150_000;

sol! {
    #[sol(rpc)]
    interface IUniswapV2Router {
        function getAmountsOut(uint256 amountIn, address[] path)
            external view returns (uint256[] amounts);
    }
}

pub struct SushiV2Backend {
    ctx: OnChainContext,
}

impl SushiV2Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn router_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Arbitrum => Some(SUSHI_V2_ROUTER_ARBITRUM),
            Chain::Base => Some(SUSHI_V2_ROUTER_BASE),
            Chain::Ethereum => Some(SUSHI_V2_ROUTER_ETHEREUM),
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::router_address(chain).is_some()
    }
}

fn hubs_for(chain: Chain) -> &'static [Address] {
    match chain {
        Chain::Arbitrum => ARBITRUM_HUBS,
        Chain::Base => BASE_HUBS,
        Chain::Ethereum => ETHEREUM_HUBS,
        Chain::Solana => &[],
    }
}

#[async_trait::async_trait]
impl DexBackend for SushiV2Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::router_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let router = IUniswapV2Router::new(addr, self.ctx.provider.clone());

        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;
        let hubs = hubs_for(request.chain);

        let mut paths: Vec<Vec<Address>> = vec![vec![token_in, token_out]];
        for hub in hubs {
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
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_V2_SWAP));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_V2_SWAP),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}
