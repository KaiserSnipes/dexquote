//! Non-blocking quote stream for the TUI.
//!
//! Each selected backend is spawned as its own tokio task. Results flow
//! through an unbounded mpsc channel with `(index, BackendResult)` pairs so
//! the draw loop can update exactly the row that changed. The event loop
//! drains the channel on every tick (~80ms) before rendering, which means
//! a backend that returns in 120ms shows up on screen within ~200ms of
//! actually finishing — no waiting for the slowest one.

use dexquote_core::{BackendResult, DexBackend, DexQuoteError, QuoteRequest};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

pub type PendingRx = mpsc::UnboundedReceiver<(usize, BackendResult)>;

/// Fan out `backends` over independent tokio tasks. Returns immediately
/// with the receiver end of a channel that each task posts its finished
/// `BackendResult` into. Callers drain the receiver on their own cadence.
///
/// Dropping the receiver doesn't cancel the tasks (they'll keep running
/// and fail to send on a closed channel, which they silently ignore). This
/// is fine — backend quote calls are cheap, bounded by `per_backend_timeout`,
/// and don't hold any meaningful resources after they finish.
pub fn spawn(
    backends: Vec<Arc<dyn DexBackend>>,
    request: QuoteRequest,
    per_backend_timeout: Duration,
) -> PendingRx {
    let (tx, rx) = mpsc::unbounded_channel();

    for (idx, backend) in backends.into_iter().enumerate() {
        let tx = tx.clone();
        let request = request.clone();
        tokio::spawn(async move {
            let name = backend.name();
            let quote = match tokio::time::timeout(per_backend_timeout, backend.quote(&request))
                .await
            {
                Ok(Ok(mut q)) => {
                    // latency_ms is usually populated by the backend, but
                    // plumb through as-is.
                    if q.latency_ms == 0 {
                        q.latency_ms = 1;
                    }
                    Ok(q)
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(DexQuoteError::Timeout {
                    backend: name,
                    ms: per_backend_timeout.as_millis() as u64,
                }),
            };
            // If the receiver was dropped (user hit Esc) this send is a
            // no-op. That's the intended behavior.
            let _ = tx.send((idx, BackendResult { name, quote }));
        });
    }

    rx
}
