//! Solana-side live gas tracker.
//!
//! Mirrors the shape of the EVM `gas_tracker` but hits different
//! sources because Solana has neither `eth_gasPrice` nor Chainlink:
//!
//!   - **SOL/USD** via Pyth Hermes HTTPS endpoint
//!     (`hermes.pyth.network/v2/updates/price/latest`)
//!   - **Priority fee** via Solana RPC `getRecentPrioritizationFees`
//!     (returns ~150 recent slot entries with micro-lamports per CU)
//!   - **Typical swap cost** computed off base fee (5000 lamports per
//!     signature) + median priority fee × 250k compute units, converted
//!     to USD via the Pyth price.
//!
//! Emits `SolanaGasSnapshot` into an mpsc channel the TUI drains on
//! each tick. Same polling cadence as the EVM tracker (5s). No API
//! keys — both endpoints are free and anonymous.

use crate::tui::gas_tracker::GasSnapshot;
use serde::Deserialize;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Pyth Hermes price feed ID for SOL/USD. Hex-encoded, 32 bytes.
/// Source: https://pyth.network/developers/price-feed-ids#solana
const PYTH_SOL_USD_FEED_ID: &str =
    "ef0d8b6fda2ceba41da15d4095d1da392a0d2f8ed0c6c7bc0f4cfac8c280b56d";
const HERMES_URL: &str = "https://hermes.pyth.network/v2/updates/price/latest";
const SOLANA_RPC: &str = "https://api.mainnet-beta.solana.com";
const POLL_INTERVAL: Duration = Duration::from_secs(5);

/// Base fee per signature on Solana (fixed at the protocol level).
/// A typical Jupiter swap uses 1–2 signatures; we assume 1 for the
/// header estimate.
const BASE_FEE_LAMPORTS: u64 = 5000;

/// Typical compute-unit budget for a Jupiter swap. Real swaps range
/// from 150k (simple) to 500k (complex multi-hop) — 250k is the
/// commonly-observed median.
const TYPICAL_SWAP_CU: u64 = 250_000;

const LAMPORTS_PER_SOL: f64 = 1_000_000_000.0;

/// Snapshot of Solana network state. Sibling of `GasSnapshot` but
/// carries Solana-native fields.
#[derive(Debug, Clone, Copy)]
pub struct SolanaGasSnapshot {
    /// Median priority fee across the last ~150 slots, micro-lamports
    /// per compute unit. Often 0 during low-load windows.
    pub priority_fee_micro_lamports_per_cu: u64,
    /// SOL/USD spot price from Pyth. `0.0` when the Pyth call fails.
    pub sol_usd: f64,
    /// Slot of the latest prioritization-fee entry — Solana's rough
    /// equivalent of a block number.
    pub slot: u64,
    /// When the snapshot was collected.
    pub fetched_at: Instant,
}

impl SolanaGasSnapshot {
    /// Estimated cost of a typical Jupiter swap in USD.
    ///
    ///   total_lamports = base_fee + (priority_µlmp × CU / 1_000_000)
    ///   total_sol      = total_lamports / 1e9
    ///   usd            = total_sol × sol_usd
    pub fn swap_cost_usd(&self) -> f64 {
        let priority_lamports =
            (self.priority_fee_micro_lamports_per_cu as u128 * TYPICAL_SWAP_CU as u128 / 1_000_000)
                as u64;
        let total_lamports = BASE_FEE_LAMPORTS + priority_lamports;
        (total_lamports as f64 / LAMPORTS_PER_SOL) * self.sol_usd
    }

    pub fn age_secs(&self) -> u64 {
        self.fetched_at.elapsed().as_secs()
    }
}

/// Spawn the background Solana gas tracker. Same ownership model as
/// the EVM tracker: task runs for the lifetime of the TUI and exits
/// automatically when the receiver is dropped (TUI close).
pub fn spawn() -> mpsc::Receiver<SolanaGasSnapshot> {
    let (tx, rx) = mpsc::channel(4);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(3))
        .user_agent(concat!("dexquote/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    tokio::spawn(async move {
        loop {
            if let Some(snap) = fetch_snapshot(&client).await {
                if tx.send(snap).await.is_err() {
                    break;
                }
            }
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });

    rx
}

async fn fetch_snapshot(client: &reqwest::Client) -> Option<SolanaGasSnapshot> {
    let pyth_fut = fetch_sol_usd(client);
    let rpc_fut = fetch_priority_fee(client);
    let (sol_usd, (priority_fee, slot)) = tokio::join!(pyth_fut, rpc_fut);

    // If both calls failed, skip this snapshot entirely.
    if sol_usd.is_none() && priority_fee.is_none() {
        return None;
    }

    Some(SolanaGasSnapshot {
        priority_fee_micro_lamports_per_cu: priority_fee.unwrap_or(0),
        sol_usd: sol_usd.unwrap_or(0.0),
        slot: slot.unwrap_or(0),
        fetched_at: Instant::now(),
    })
}

async fn fetch_sol_usd(client: &reqwest::Client) -> Option<f64> {
    let url = format!("{HERMES_URL}?ids[]={PYTH_SOL_USD_FEED_ID}");
    let resp = client.get(&url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let parsed: HermesResponse = resp.json().await.ok()?;
    let first = parsed.parsed.first()?;
    let price_raw: i64 = first.price.price.parse().ok()?;
    let expo = first.price.expo;
    // Pyth prices are `price × 10^expo`. `expo` is typically negative
    // (e.g. -8), so multiplying by `10^expo` divides.
    let scale = 10f64.powi(expo);
    Some(price_raw as f64 * scale)
}

async fn fetch_priority_fee(client: &reqwest::Client) -> (Option<u64>, Option<u64>) {
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"getRecentPrioritizationFees"}"#;
    let Ok(resp) = client
        .post(SOLANA_RPC)
        .header("content-type", "application/json")
        .body(body)
        .send()
        .await
    else {
        return (None, None);
    };
    if !resp.status().is_success() {
        return (None, None);
    }
    let Ok(parsed) = resp.json::<RpcResponse>().await else {
        return (None, None);
    };
    let entries = parsed.result;
    if entries.is_empty() {
        return (None, None);
    }
    let mut fees: Vec<u64> = entries.iter().map(|e| e.prioritization_fee).collect();
    fees.sort();
    let median = fees[fees.len() / 2];
    // Latest slot is the max across the returned window.
    let latest_slot = entries.iter().map(|e| e.slot).max();
    (Some(median), latest_slot)
}

// Pyth Hermes response shapes — only the fields we actually read.
#[derive(Deserialize)]
struct HermesResponse {
    parsed: Vec<HermesEntry>,
}

#[derive(Deserialize)]
struct HermesEntry {
    price: HermesPrice,
}

#[derive(Deserialize)]
struct HermesPrice {
    price: String,
    expo: i32,
}

// Solana JSON-RPC response — typed just enough to extract the fee
// and slot arrays.
#[derive(Deserialize)]
struct RpcResponse {
    #[serde(default)]
    result: Vec<PrioEntry>,
}

#[derive(Deserialize)]
struct PrioEntry {
    slot: u64,
    #[serde(rename = "prioritizationFee")]
    prioritization_fee: u64,
}

/// Unused re-export to hint that the EVM snapshot type still exists
/// in the sibling module — call sites pick one or the other based
/// on chain.
#[allow(dead_code)]
pub type EvmGasSnapshot = GasSnapshot;
