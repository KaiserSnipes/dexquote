//! `dexquote-core` — library for comparing DEX quotes across multiple
//! protocols on EVM chains.
//!
//! # Example
//!
//! ```no_run
//! use dexquote_core::{Chain, Token, QuoteRequest, OdosBackend, DexBackend};
//! use dexquote_core::token::parse_amount;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let chain = Chain::Arbitrum;
//! let sell = Token::resolve("WETH", chain, None).await?;
//! let buy = Token::resolve("USDC", chain, None).await?;
//! let amount_in = parse_amount("1.0", sell.decimals)?;
//!
//! let req = QuoteRequest { chain, token_in: sell, token_out: buy, amount_in, block_id: None };
//! let backend = OdosBackend::new();
//! let quote = backend.quote(&req).await?;
//! println!("{} -> {}", quote.backend, quote.amount_out);
//! # Ok(()) }
//! ```

#![deny(clippy::unwrap_used, clippy::expect_used)]
#![allow(clippy::result_large_err)]

pub mod backends;
pub mod chain;
pub mod error;
pub mod gas;
pub mod quote;
pub mod token;

pub use backends::{
    AerodromeBackend, BalancerV2Backend, CamelotV3Backend, CowSwapBackend, CurveBackend,
    DexBackend, DodoV2Backend, FraxSwapBackend, JupiterSwapBackend, JupiterUltraBackend,
    KyberSwapBackend, LiFiBackend, LiFiSolanaBackend, MaverickV2Backend, OdosBackend,
    OnChainContext, OpenOceanBackend, OpenOceanSolanaBackend, PancakeV3Backend, ParaswapBackend,
    RaydiumBackend, SlipstreamBackend, SushiV2Backend, TraderJoeBackend, UniswapV2Backend,
    UniswapV3Backend, UniswapV4Backend,
};
pub use alloy::primitives::{Address, U256};
pub use chain::Chain;
pub use error::DexQuoteError;
pub use gas::{GasPriceUsd, GasPricer};
pub use quote::{quote_all, BackendResult, Quote, QuoteRequest};
pub use token::{list_tokens, suggest_symbols, Token};
