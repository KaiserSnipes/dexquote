//! Trader Joe (LFJ) Liquidity Book v2.2 backend for Arbitrum.
//!
//! Uses the LBQuoter helper contract which finds the best route across all
//! Liquidity Book versions and returns the amounts along the path. We call
//! it with the direct route `[tokenIn, tokenOut]` and (when neither token
//! is WETH) the bridged route `[tokenIn, WETH, tokenOut]`, then pick the
//! larger output.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use crate::token::Token;
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "TraderJoe";

// LBQuoter v2.2 on Arbitrum One.
// Source: https://developers.lfj.gg/deployment-addresses/arbitrum
const LB_QUOTER_ARBITRUM: Address = address!("9A550a522BBaDFB69019b0432800Ed17855A51C3");

// LB swap gas is larger than a V2 swap because it walks bins. 250k is a
// conservative midpoint — good enough for the USD display.
const GAS_ESTIMATE_LB_SWAP: u64 = 250_000;

sol! {
    #[sol(rpc)]
    interface ILBQuoter {
        struct Quote {
            address[] route;
            address[] pairs;
            uint256[] binSteps;
            uint8[] versions;
            uint128[] amounts;
            uint128[] virtualAmountsWithoutSlippage;
            uint128[] fees;
        }

        function findBestPathFromAmountIn(address[] route, uint128 amountIn)
            external view returns (Quote memory);
    }
}

pub struct TraderJoeBackend {
    ctx: OnChainContext,
}

impl TraderJoeBackend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Arbitrum => Some(LB_QUOTER_ARBITRUM),
            // Trader Joe LB has no Base / Ethereum / Solana deployment.
            // Liquid Book is Arbitrum + Avalanche only.
            Chain::Base => None,
            Chain::Ethereum => None,
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::quoter_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for TraderJoeBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = ILBQuoter::new(addr, self.ctx.provider.clone());

        let weth = Token::weth(request.chain)
            .address
            .as_evm()
            .ok_or(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            })?;
        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;

        let mut routes: Vec<Vec<Address>> = vec![vec![token_in, token_out]];
        if token_in != weth && token_out != weth {
            routes.push(vec![token_in, weth, token_out]);
        }

        let amount_in = u256_to_u128_saturating(request.amount_in);

        let block_id = request.block_id;
        let calls = routes.into_iter().map(|route| {
            let quoter = &quoter;
            async move {
                let call = quoter.findBestPathFromAmountIn(route, amount_in);
                let call = match block_id {
                    Some(id) => call.block(id),
                    None => call,
                };
                call.call().await.ok().and_then(|q| q.amounts.last().copied())
            }
        });

        let results = join_all(calls).await;

        let amount_out_u128 = results
            .into_iter()
            .flatten()
            .filter(|a| *a != 0)
            .max()
            .ok_or(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            })?;

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_LB_SWAP));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out: U256::from(amount_out_u128),
            gas_estimate: Some(GAS_ESTIMATE_LB_SWAP),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}

fn u256_to_u128_saturating(value: U256) -> u128 {
    if value > U256::from(u128::MAX) {
        u128::MAX
    } else {
        value.to::<u128>()
    }
}
