//! Gas pricing helper. A `GasPricer` is created once per run and holds an
//! `OnceCell` for the `(gas_price_wei, eth_usd)` tuple. On-chain backends
//! share it via `Arc` and call `gas_usd(gas_units)` to get a dollar estimate
//! using the cached values — so a 4-backend run only burns two extra RPC
//! round-trips total (one `eth_gasPrice`, one Chainlink `latestAnswer`).

use crate::chain::Chain;
use alloy::network::Ethereum;
use alloy::primitives::{address, Address, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::sol;
use std::sync::Arc;
use tokio::sync::OnceCell;

// Chainlink ETH/USD feeds per chain. All are 8-decimal feeds verified by
// calling `latestAnswer()` on-chain.
// Arbitrum: https://docs.chain.link/data-feeds/price-feeds/addresses?network=arbitrum
// Base:     https://docs.chain.link/data-feeds/price-feeds/addresses?network=base
// Ethereum: https://docs.chain.link/data-feeds/price-feeds/addresses?network=ethereum
const CHAINLINK_ETH_USD_ARBITRUM: Address =
    address!("639Fe6ab55C921f74e7fac1ee960C0B6293ba612");
const CHAINLINK_ETH_USD_BASE: Address =
    address!("71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70");
const CHAINLINK_ETH_USD_ETHEREUM: Address =
    address!("5f4eC3Df9cbd43714FE2740f5E3616155c5b8419");

sol! {
    #[sol(rpc)]
    interface IChainlinkAggregator {
        function latestAnswer() external view returns (int256);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GasPriceUsd {
    pub gas_price_wei: U256,
    pub eth_usd: f64,
}

impl GasPriceUsd {
    /// Convert a gas-units estimate to USD using the cached prices.
    pub fn gas_units_to_usd(&self, gas_units: u64) -> f64 {
        let wei = self.gas_price_wei.saturating_mul(U256::from(gas_units));
        let eth = u256_to_f64_scaled(wei, 18);
        eth * self.eth_usd
    }
}

pub struct GasPricer {
    provider: Option<DynProvider<Ethereum>>,
    chain: Chain,
    cache: OnceCell<Option<GasPriceUsd>>,
}

impl GasPricer {
    /// Create a pricer that reuses an already-connected provider. Pass `None`
    /// if no RPC is available (in which case every `get()` returns `None`).
    pub fn new(chain: Chain, provider: Option<DynProvider<Ethereum>>) -> Arc<Self> {
        Arc::new(Self {
            chain,
            provider,
            cache: OnceCell::new(),
        })
    }

    /// Returns the cached (gas_price, eth_usd) tuple, fetching it on first
    /// call. Returns `None` if no provider is configured or if either call
    /// failed — callers should treat this as "unknown" and render `—`.
    pub async fn get(&self) -> Option<GasPriceUsd> {
        *self
            .cache
            .get_or_init(|| async { self.fetch().await })
            .await
    }

    async fn fetch(&self) -> Option<GasPriceUsd> {
        let provider = self.provider.as_ref()?;
        let gas_price = fetch_gas_price(provider).await?;
        let eth_usd = fetch_eth_usd(provider, self.chain).await?;
        Some(GasPriceUsd {
            gas_price_wei: gas_price,
            eth_usd,
        })
    }
}

async fn fetch_gas_price(provider: &DynProvider<Ethereum>) -> Option<U256> {
    let price = provider.get_gas_price().await.ok()?;
    Some(U256::from(price))
}

async fn fetch_eth_usd(provider: &DynProvider<Ethereum>, chain: Chain) -> Option<f64> {
    let feed_address = match chain {
        Chain::Arbitrum => CHAINLINK_ETH_USD_ARBITRUM,
        Chain::Base => CHAINLINK_ETH_USD_BASE,
        Chain::Ethereum => CHAINLINK_ETH_USD_ETHEREUM,
        // Solana has no Chainlink feed — Solana backends leave
        // gas_usd as None (matches the existing CoWSwap pattern).
        Chain::Solana => return None,
    };
    let feed = IChainlinkAggregator::new(feed_address, provider);
    let builder = feed.latestAnswer();
    let answer = builder.call().await.ok()?;
    if answer.is_negative() {
        return None;
    }
    // Chainlink feeds use 8 decimals. Convert to f64.
    let unsigned = answer.into_raw();
    Some(u256_to_f64_scaled(unsigned, 8))
}

fn u256_to_f64_scaled(value: U256, decimals: u32) -> f64 {
    // Split into whole and fractional parts to avoid losing precision for
    // values larger than f64 can hold exactly in integer form. For gas
    // prices and Chainlink feeds, f64 is plenty once scaled.
    let divisor = U256::from(10u128).pow(U256::from(decimals));
    if divisor.is_zero() {
        return 0.0;
    }
    let whole = value / divisor;
    let frac = value % divisor;
    let whole_f = u256_to_f64_saturating(whole);
    let frac_f = u256_to_f64_saturating(frac);
    let scale = 10f64.powi(decimals as i32);
    whole_f + (frac_f / scale)
}

fn u256_to_f64_saturating(value: U256) -> f64 {
    // `U256::to::<u128>()` can panic on overflow — we saturate instead.
    if value > U256::from(u128::MAX) {
        u128::MAX as f64
    } else {
        value.to::<u128>() as f64
    }
}
