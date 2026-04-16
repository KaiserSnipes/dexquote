//! PancakeSwap V3 on Arbitrum.
//!
//! PancakeSwap V3 is a direct Uniswap V3 fork with an identical QuoterV2 ABI.
//! We reuse the `IQuoterV2` bindings from `uniswap_v3` rather than redefining
//! the contract — the only differences are the deployed address and the
//! display name.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, aliases::U24, Address, U256};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

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

const BACKEND_NAME: &str = "Pancake";

// PancakeSwap V3 QuoterV2 addresses per chain.
// Every supported chain shares the same address thanks to CREATE2
// deterministic deployment. Verified via `eth_getCode` on all three RPCs.
// Source: https://developer.pancakeswap.finance/contracts/v3/addresses
const PANCAKE_V3_QUOTER_ARBITRUM: Address =
    address!("b048bBc1Ee6b733FFfCFb9e9CeF7375518e25997");
const PANCAKE_V3_QUOTER_BASE: Address =
    address!("b048bBc1Ee6b733FFfCFb9e9CeF7375518e25997");
const PANCAKE_V3_QUOTER_ETHEREUM: Address =
    address!("b048bBc1Ee6b733FFfCFb9e9CeF7375518e25997");

// PancakeSwap V3 uses 100, 500, 2500, 10000 bps fee tiers on Arbitrum.
const FEE_TIERS: [u32; 4] = [100, 500, 2500, 10000];

pub struct PancakeV3Backend {
    ctx: OnChainContext,
}

impl PancakeV3Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Arbitrum => Some(PANCAKE_V3_QUOTER_ARBITRUM),
            Chain::Base => Some(PANCAKE_V3_QUOTER_BASE),
            Chain::Ethereum => Some(PANCAKE_V3_QUOTER_ETHEREUM),
            Chain::Solana => None,
        }
    }

    pub fn supports(chain: Chain) -> bool {
        Self::quoter_address(chain).is_some()
    }
}

#[async_trait::async_trait]
impl DexBackend for PancakeV3Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = IQuoterV2::new(addr, self.ctx.provider.clone());

        let token_in = request.token_in.evm_address(BACKEND_NAME)?;
        let token_out = request.token_out.evm_address(BACKEND_NAME)?;
        let amount_in = request.amount_in;

        let block_id = request.block_id;
        let calls = FEE_TIERS.iter().map(|&fee| {
            let params = IQuoterV2::QuoteExactInputSingleParams {
                tokenIn: token_in,
                tokenOut: token_out,
                amountIn: amount_in,
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
            if amount_out.is_zero() {
                continue;
            }
            let gas = saturating_to_u64(ret.gasEstimate);
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
