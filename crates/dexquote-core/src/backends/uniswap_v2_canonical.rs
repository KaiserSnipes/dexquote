//! Canonical UniswapV2 backend for Ethereum mainnet.
//!
//! Uniswap V2 is the original AMM, still holds real liquidity on mainnet
//! despite V3 / V4 migration. Almost every UniV2 fork (SushiSwap,
//! PancakeSwap V2, FraxSwap, ShibaSwap, ...) reuses its `Router02` ABI
//! byte-for-byte. The `sol!` macro binding is duplicated here rather than
//! shared because `sol!` doesn't allow `pub interface` — see the comment
//! at the top of `pancake_v3.rs` for the same workaround.
//!
//! Mainnet-only: UniV2 was deprecated on L2s in favour of V3 / V4. The
//! `supports()` helper returns `None` for every non-Ethereum chain so the
//! backend is filtered out of Arbitrum and Base quotes automatically.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "UniswapV2";

// Canonical UniswapV2 Router02 address on Ethereum mainnet.
// Source: https://docs.uniswap.org/contracts/v2/reference/smart-contracts/router-02
// Verified: `eth_getCode` against `ethereum.publicnode.com` returns the
// deployed router bytecode.
const UNISWAP_V2_ROUTER_ETHEREUM: Address =
    address!("7a250d5630B4cF539739dF2C5dAcb4c659F2488D");

// Hub tokens to try as intermediate hops. WETH + USDC cover the vast
// majority of V2 pairs.
const ETHEREUM_HUBS: &[Address] = &[
    address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH
    address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), // USDC
];

const GAS_ESTIMATE_V2_SWAP: u64 = 150_000;

sol! {
    #[sol(rpc)]
    interface IUniswapV2Router {
        function getAmountsOut(uint256 amountIn, address[] path)
            external view returns (uint256[] amounts);
    }
}

pub struct UniswapV2Backend {
    ctx: OnChainContext,
}

impl UniswapV2Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn router_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Ethereum => Some(UNISWAP_V2_ROUTER_ETHEREUM),
            // Canonical UniV2 was never deployed on Arbitrum or Base —
            // V3 was the first Uniswap release on those L2s.
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
impl DexBackend for UniswapV2Backend {
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
