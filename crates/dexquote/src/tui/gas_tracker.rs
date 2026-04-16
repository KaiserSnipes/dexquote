//! Live gas tracker for the TUI.
//!
//! A small background task that polls the shared RPC provider every ~5s
//! for the current gas price, ETH/USD (via the Chainlink feed on-chain),
//! and the latest block number. Snapshots are posted through an mpsc
//! channel that the TUI event loop drains on each tick.
//!
//! The task exits automatically when the receiver is dropped (which
//! happens as soon as the TUI is closed), so there's no lingering
//! background work after the app quits.

use alloy::network::Ethereum;
use alloy::primitives::{address, Address, U256};
use alloy::providers::{DynProvider, Provider};
use alloy::sol;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

const CHAINLINK_ETH_USD_ARBITRUM: Address =
    address!("639Fe6ab55C921f74e7fac1ee960C0B6293ba612");
const CHAINLINK_ETH_USD_BASE: Address =
    address!("71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70");
const CHAINLINK_ETH_USD_ETHEREUM: Address =
    address!("5f4eC3Df9cbd43714FE2740f5E3616155c5b8419");

fn chainlink_feed_for(chain: dexquote_core::Chain) -> Option<Address> {
    match chain {
        dexquote_core::Chain::Arbitrum => Some(CHAINLINK_ETH_USD_ARBITRUM),
        dexquote_core::Chain::Base => Some(CHAINLINK_ETH_USD_BASE),
        dexquote_core::Chain::Ethereum => Some(CHAINLINK_ETH_USD_ETHEREUM),
        // Solana has no Chainlink ETH/USD feed. The TUI gas tracker
        // sits idle for Solana — there's no meaningful per-chain
        // gas display to show.
        dexquote_core::Chain::Solana => None,
    }
}

/// How often the tracker polls. Arbitrum blocks are ~250ms but gas rarely
/// moves fast enough to justify aggressive polling, and we want to keep
/// the public RPC happy. Five seconds is ~12 calls/minute — well under
/// any public endpoint's rate limits.
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Assumed gas units for a "simple swap". Used to turn `gas_price_wei` into
/// a dollar figure for the "swap ~$0.03" header estimate. 200k is a fair
/// midpoint between a V2 router call (~150k) and an LB/UniV3 call (~250k).
const SIMPLE_SWAP_GAS_UNITS: u64 = 200_000;

sol! {
    #[sol(rpc)]
    interface IChainlinkAggregator {
        function latestAnswer() external view returns (int256);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct GasSnapshot {
    pub gas_price_wei: U256,
    pub eth_usd: f64,
    pub block_number: u64,
    pub fetched_at: Instant,
}

impl GasSnapshot {
    pub fn gas_price_gwei(&self) -> f64 {
        let wei_per_gwei = U256::from(1_000_000_000u64);
        if wei_per_gwei.is_zero() {
            return 0.0;
        }
        let whole = self.gas_price_wei / wei_per_gwei;
        let frac = self.gas_price_wei % wei_per_gwei;
        let whole_f = u256_to_f64_saturating(whole);
        let frac_f = u256_to_f64_saturating(frac) / 1_000_000_000.0;
        whole_f + frac_f
    }

    /// Rough USD cost of a 200k-gas-unit swap, ignoring L1 data fees. Close
    /// enough for a header indicator; the per-backend gas columns shown
    /// alongside actual quotes are more precise because they use the
    /// backend-reported gas estimate.
    pub fn swap_cost_usd(&self) -> f64 {
        let gas_cost_wei = self.gas_price_wei.saturating_mul(U256::from(SIMPLE_SWAP_GAS_UNITS));
        let eth = u256_to_f64_scaled(gas_cost_wei, 18);
        eth * self.eth_usd
    }

    /// How many seconds ago was this snapshot fetched?
    pub fn age_secs(&self) -> u64 {
        self.fetched_at.elapsed().as_secs()
    }
}

/// Spawn the background tracker task. Returns the receiver end of a channel
/// that the caller polls from its event loop. Dropping the receiver ends
/// the task at the next send.
pub fn spawn(
    provider: DynProvider<Ethereum>,
    chain: dexquote_core::Chain,
) -> mpsc::Receiver<GasSnapshot> {
    let (tx, rx) = mpsc::channel(4);
    let Some(feed_address) = chainlink_feed_for(chain) else {
        // No Chainlink feed for this chain (e.g. Solana). The TUI
        // receiver stays alive but never receives snapshots — the
        // `gas` strip in the header renders "—" for every field.
        return rx;
    };
    tokio::spawn(async move {
        let feed = IChainlinkAggregator::new(feed_address, provider.clone());
        loop {
            if let Some(snap) = fetch_snapshot(&provider, &feed).await {
                // A closed receiver means the TUI exited — clean shutdown.
                if tx.send(snap).await.is_err() {
                    break;
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
    rx
}

async fn fetch_snapshot(
    provider: &DynProvider<Ethereum>,
    feed: &IChainlinkAggregator::IChainlinkAggregatorInstance<DynProvider<Ethereum>, Ethereum>,
) -> Option<GasSnapshot> {
    // Fan all three RPC calls out in parallel so a single snapshot is one
    // RTT-equivalent on a reasonably-fast provider. The binding for the
    // CallBuilder has to outlive the .call() future, so we split the builder
    // and the future across two let-statements.
    let gas_fut = provider.get_gas_price();
    let block_fut = provider.get_block_number();
    let eth_builder = feed.latestAnswer();
    let eth_fut = eth_builder.call();

    let (gas_res, block_res, eth_res) = tokio::join!(gas_fut, block_fut, eth_fut);
    let gas_price_wei = U256::from(gas_res.ok()?);
    let block_number = block_res.ok()?;
    let answer = eth_res.ok()?;
    if answer.is_negative() {
        return None;
    }
    let eth_usd = u256_to_f64_scaled(answer.into_raw(), 8);

    Some(GasSnapshot {
        gas_price_wei,
        eth_usd,
        block_number,
        fetched_at: Instant::now(),
    })
}

fn u256_to_f64_scaled(value: U256, decimals: u32) -> f64 {
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
    if value > U256::from(u128::MAX) {
        u128::MAX as f64
    } else {
        value.to::<u128>() as f64
    }
}
