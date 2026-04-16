//! Camelot V3 (Algebra) on Arbitrum.
//!
//! Camelot V3 is built on Algebra Integral, which has dynamic fees — the
//! quoter function therefore does NOT take a fee tier parameter the way
//! Uniswap V3's QuoterV2 does. Only tokenIn, tokenOut, amountIn, and a
//! (usually zero) sqrt price limit.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use std::time::Instant;

const BACKEND_NAME: &str = "Camelot";

// Camelot V3 (Algebra) Quoter on Arbitrum One.
// Source: https://docs.camelot.exchange/contracts/arbitrum
const CAMELOT_V3_QUOTER_ARBITRUM: Address =
    address!("0Fc73040b26E9bC8514fA028D998E73A254Fa76E");

// Camelot LB gas is roughly 180k on a one-pool swap; good enough for USD display.
const GAS_ESTIMATE_CAMELOT: u64 = 180_000;

sol! {
    #[sol(rpc)]
    interface IAlgebraQuoter {
        function quoteExactInputSingle(
            address tokenIn,
            address tokenOut,
            uint256 amountIn,
            uint160 limitSqrtPrice
        ) external returns (uint256 amountOut, uint16 fee);
    }
}

pub struct CamelotV3Backend {
    ctx: OnChainContext,
}

impl CamelotV3Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn quoter_address(chain: Chain) -> Option<Address> {
        match chain {
            Chain::Arbitrum => Some(CAMELOT_V3_QUOTER_ARBITRUM),
            // Camelot is Arbitrum-native; no Base / Ethereum / Solana deployment.
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
impl DexBackend for CamelotV3Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();
        let addr = Self::quoter_address(request.chain).ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let quoter = IAlgebraQuoter::new(addr, self.ctx.provider.clone());

        let call = quoter.quoteExactInputSingle(
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
            request.amount_in,
            alloy::primitives::Uint::<160, 3>::ZERO,
        );
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

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_CAMELOT));

        let _ = U256::ZERO;

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_CAMELOT),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}
