//! Depth streaming for the v1.2 TUI.
//!
//! Spawns one background task that iterates the 5 canonical
//! notional multipliers (0.1×, 1×, 10×, 100×, 1000×) sequentially,
//! running a fresh `quote_all` round per level. Each completed
//! level is emitted as a `DepthProgress::LevelDone { idx, row }`
//! event through an unbounded mpsc channel; the final event is
//! `DepthProgress::Finished { report }` carrying the aggregated
//! `DepthReport` ready for rendering.
//!
//! Mirrors the shape of `quote_stream::spawn`: the caller polls the
//! receiver on every tick and transitions phase when the Finished
//! event lands. Dropping the receiver orphans the spawned task;
//! since each `quote_all` round is bounded by `per_backend_timeout`,
//! the orphaned work finishes cleanly within a few seconds and
//! no resources leak.

use crate::depth::{scale_amount, NOTIONALS};
use crate::render::depth::{DepthLevel, DepthReport};
use dexquote_core::{quote_all, DexBackend, QuoteRequest};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

/// Event payload emitted by the depth stream. The UI layer reads
/// these off the mpsc and updates the per-level row table.
#[derive(Debug, Clone)]
pub enum DepthProgress {
    /// A level has just started (user sees spinner for this row).
    LevelStarted { idx: usize },
    /// A level has completed with its picked best venue (or `None`
    /// if every backend returned no-route / dead-pool / thin-liq).
    LevelDone { idx: usize, row: DepthLevel },
    /// All levels are done — this is the final event, after which
    /// the spawned task exits and the UI should transition to
    /// `Phase::ShowingDepth`.
    Finished { report: DepthReport },
}

pub type DepthRx = mpsc::UnboundedReceiver<DepthProgress>;

/// Spawn the depth sweep. Returns immediately with the receiver.
/// The spawned task iterates the 5 notional levels sequentially;
/// on the final level it composes a `DepthReport` and sends it in
/// a `Finished` event, then exits.
pub fn spawn(
    backends: Vec<Arc<dyn DexBackend>>,
    base_request: QuoteRequest,
    base_amount_human: String,
    per_backend_timeout: Duration,
) -> DepthRx {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let mut levels: Vec<DepthLevel> = Vec::with_capacity(NOTIONALS.len());

        for (idx, &mult) in NOTIONALS.iter().enumerate() {
            if tx.send(DepthProgress::LevelStarted { idx }).is_err() {
                return;
            }

            let scaled_amount = scale_amount(base_request.amount_in, mult);
            let request = QuoteRequest {
                chain: base_request.chain,
                token_in: base_request.token_in.clone(),
                token_out: base_request.token_out.clone(),
                amount_in: scaled_amount,
                block_id: None,
            };
            let results = quote_all(&backends, &request, per_backend_timeout).await;

            // Reuse the same thin/dead filters the CLI depth mode
            // uses so the TUI's picked best matches byte-for-byte.
            let successes: Vec<&dexquote_core::Quote> = results
                .iter()
                .filter_map(|r| r.quote.as_ref().ok())
                .collect();
            let median = crate::render::table::median_amount(&successes);
            let best = successes
                .iter()
                .filter(|q| !crate::render::table::is_thin_liquidity(q.amount_out, median))
                .filter(|q| !crate::render::table::is_dead_pool(q.amount_out, median))
                .max_by_key(|q| q.amount_out);

            let level = match best {
                Some(q) => DepthLevel {
                    multiplier: mult,
                    amount_in: scaled_amount,
                    amount_out: Some(q.amount_out),
                    best_venue: Some(q.backend.to_string()),
                },
                None => DepthLevel {
                    multiplier: mult,
                    amount_in: scaled_amount,
                    amount_out: None,
                    best_venue: None,
                },
            };

            levels.push(level.clone());
            if tx
                .send(DepthProgress::LevelDone { idx, row: level })
                .is_err()
            {
                return;
            }
        }

        let report = DepthReport {
            chain: base_request.chain,
            sell: base_request.token_in.clone(),
            buy: base_request.token_out.clone(),
            base_amount_human,
            levels,
        };
        let _ = tx.send(DepthProgress::Finished { report });
    });

    rx
}
