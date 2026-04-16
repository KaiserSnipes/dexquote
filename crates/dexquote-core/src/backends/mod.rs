use crate::error::DexQuoteError;
use crate::gas::GasPricer;
use crate::quote::{Quote, QuoteRequest};
use alloy::network::Ethereum;
use alloy::providers::DynProvider;
use std::sync::Arc;

mod aerodrome;
mod aerodrome_slipstream;
mod balancer_v2;
mod camelot_v3;
mod cowswap;
mod curve;
mod dodo_v2;
mod fraxswap;
mod jupiter_swap;
mod jupiter_ultra;
mod kyberswap;
mod lifi_solana;
mod lifi;
mod maverick_v2;
mod odos;
mod openocean;
mod openocean_solana;
mod pancake_v3;
mod paraswap;
mod raydium;
mod trader_joe;
mod uniswap_v2;
mod uniswap_v2_canonical;
mod uniswap_v3;
mod uniswap_v4;

pub use aerodrome::AerodromeBackend;
pub use aerodrome_slipstream::SlipstreamBackend;
pub use balancer_v2::BalancerV2Backend;
pub use camelot_v3::CamelotV3Backend;
pub use cowswap::CowSwapBackend;
pub use curve::CurveBackend;
pub use dodo_v2::DodoV2Backend;
pub use fraxswap::FraxSwapBackend;
pub use jupiter_swap::JupiterSwapBackend;
pub use jupiter_ultra::JupiterUltraBackend;
pub use kyberswap::KyberSwapBackend;
pub use lifi_solana::LiFiSolanaBackend;
pub use lifi::LiFiBackend;
pub use maverick_v2::MaverickV2Backend;
pub use odos::OdosBackend;
pub use openocean::OpenOceanBackend;
pub use openocean_solana::OpenOceanSolanaBackend;
pub use pancake_v3::PancakeV3Backend;
pub use paraswap::ParaswapBackend;
pub use raydium::RaydiumBackend;
pub use trader_joe::TraderJoeBackend;
pub use uniswap_v2::SushiV2Backend;
pub use uniswap_v2_canonical::UniswapV2Backend;
pub use uniswap_v3::UniswapV3Backend;
pub use uniswap_v4::UniswapV4Backend;

#[async_trait::async_trait]
pub trait DexBackend: Send + Sync {
    fn name(&self) -> &'static str;
    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError>;
}

/// Common construction info for on-chain backends.
///
/// The `provider` is built exactly once per run and shared across every
/// on-chain backend and the `GasPricer`, so a 10-backend quote makes a
/// single TCP connection to the RPC instead of ten.
#[derive(Clone)]
pub struct OnChainContext {
    pub provider: DynProvider<Ethereum>,
    pub gas_pricer: Arc<GasPricer>,
}
