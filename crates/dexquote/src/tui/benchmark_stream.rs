//! Benchmark streaming for the v1.2 TUI.
//!
//! Spawns a single background task that walks the hardcoded
//! `BENCHMARK_PAIRS` set, optionally filtered to one chain. For
//! each pair it builds the backend list, calls `quote_all`, and
//! emits a `BenchmarkProgress` event as the pair completes. At the
//! end it aggregates stats via `benchmark::aggregate` and emits a
//! final `Finished` event with the full `BenchmarkStats`.
//!
//! Architecture mirrors `quote_stream` and `depth_stream`:
//! returns an unbounded mpsc receiver, drop-to-cancel semantics.
//! The event loop polls the receiver on every tick.

use crate::benchmark::{aggregate, BenchmarkStats, BENCHMARK_PAIRS};
use crate::BackendKind;
use dexquote_core::token::parse_amount;
use dexquote_core::{quote_all, BackendResult, Chain, QuoteRequest, Token};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

/// Per-pair summary carried in `BenchmarkProgress::PairDone` for
/// the live scroll buffer in the TUI. Contains just enough to
/// render one row ("Ethereum · 1 WETH → USDC · 11/12 ok · best
/// Paraswap · 923ms").
#[derive(Debug, Clone)]
pub struct PairSummary {
    pub chain: Chain,
    pub sell: String,
    pub buy: String,
    pub amount: String,
    pub ok_count: usize,
    pub total_count: usize,
    pub elapsed_ms: u128,
    pub best_backend: Option<String>,
}

/// Event payload emitted by the benchmark stream.
#[derive(Debug, Clone)]
pub enum BenchmarkProgress {
    /// A new pair has started running. `idx` is 0-based across the
    /// full filtered pair set; `total` is the count of pairs that
    /// will be attempted.
    PairStarted {
        chain: Chain,
        sell: String,
        buy: String,
        idx: usize,
        total: usize,
    },
    /// A pair has completed with its per-backend results.
    PairDone { summary: PairSummary },
    /// A chain was skipped during setup (usually: backend build
    /// failed — e.g. RPC unreachable).
    ChainSkipped { chain: Chain, reason: String },
    /// All pairs are done; `stats` carries the aggregated
    /// leaderboard. This is the final event; the spawned task
    /// exits immediately after.
    Finished { stats: BenchmarkStats },
}

pub type BenchmarkRx = mpsc::UnboundedReceiver<BenchmarkProgress>;

/// Spawn the benchmark sweep. `chain_filter` optionally restricts
/// the sweep to a single chain (matches the CLI `--chain-filter`
/// flag). Returns the receiver end of the progress channel
/// immediately.
pub fn spawn(
    selection: Vec<BackendKind>,
    chain_filter: Option<Chain>,
    timeout: Duration,
) -> BenchmarkRx {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        // Count how many pairs we'll actually attempt so the UI
        // can render a progress bar up front.
        let total_pairs: usize = BENCHMARK_PAIRS
            .iter()
            .filter(|(c, _, _, _)| chain_filter.map(|f| f == *c).unwrap_or(true))
            .count();

        let total_start = Instant::now();
        let mut all_results: Vec<Vec<BackendResult>> = Vec::new();
        let mut global_idx: usize = 0;

        for chain in Chain::ALL.iter().copied() {
            if let Some(filter) = chain_filter {
                if chain != filter {
                    continue;
                }
            }

            // Build the backend list for this chain. Failure (RPC
            // unreachable, etc.) skips the whole chain — emit a
            // `ChainSkipped` event so the UI can show a warning.
            let rpc_url = chain.default_public_rpc().to_string();
            let built = match crate::build_backends(&selection, chain, Some(&rpc_url), timeout)
                .await
            {
                Ok(b) => b,
                Err(e) => {
                    let _ = tx.send(BenchmarkProgress::ChainSkipped {
                        chain,
                        reason: e.message.clone(),
                    });
                    continue;
                }
            };
            let backends = built.backends;
            if backends.is_empty() {
                let _ = tx.send(BenchmarkProgress::ChainSkipped {
                    chain,
                    reason: "no backends".to_string(),
                });
                continue;
            }

            for (pair_chain, sell_sym, buy_sym, amount_str) in BENCHMARK_PAIRS {
                if *pair_chain != chain {
                    continue;
                }

                let sell = match Token::resolve_static(sell_sym, chain).ok().flatten() {
                    Some(t) => t,
                    None => continue,
                };
                let buy = match Token::resolve_static(buy_sym, chain).ok().flatten() {
                    Some(t) => t,
                    None => continue,
                };
                let amount_in = match parse_amount(amount_str, sell.decimals) {
                    Ok(a) => a,
                    Err(_) => continue,
                };

                let request = QuoteRequest {
                    chain,
                    token_in: sell,
                    token_out: buy,
                    amount_in,
                    block_id: None,
                };

                if tx
                    .send(BenchmarkProgress::PairStarted {
                        chain,
                        sell: sell_sym.to_string(),
                        buy: buy_sym.to_string(),
                        idx: global_idx,
                        total: total_pairs,
                    })
                    .is_err()
                {
                    return;
                }

                let pair_start = Instant::now();
                let results = quote_all(&backends, &request, timeout).await;
                let pair_elapsed = pair_start.elapsed().as_millis();

                // Best backend for the scroll buffer: highest
                // amount_out among non-thin-liq / non-dead-pool
                // successes. Matches the aggregator logic so the
                // live scroll buffer agrees with the final
                // leaderboard.
                let successes: Vec<&dexquote_core::Quote> = results
                    .iter()
                    .filter_map(|r| r.quote.as_ref().ok())
                    .collect();
                let median = crate::render::table::median_amount(&successes);
                let best_backend = successes
                    .iter()
                    .filter(|q| !crate::render::table::is_thin_liquidity(q.amount_out, median))
                    .filter(|q| !crate::render::table::is_dead_pool(q.amount_out, median))
                    .max_by_key(|q| q.amount_out)
                    .map(|q| q.backend.to_string());

                let summary = PairSummary {
                    chain,
                    sell: sell_sym.to_string(),
                    buy: buy_sym.to_string(),
                    amount: amount_str.to_string(),
                    ok_count: successes.len(),
                    total_count: results.len(),
                    elapsed_ms: pair_elapsed,
                    best_backend,
                };

                all_results.push(results);
                global_idx += 1;

                if tx.send(BenchmarkProgress::PairDone { summary }).is_err() {
                    return;
                }
            }
        }

        let stats = aggregate(&all_results, total_start.elapsed());
        let _ = tx.send(BenchmarkProgress::Finished { stats });
    });

    rx
}
