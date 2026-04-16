use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::token::Token;
use alloy::eips::BlockId;
use alloy::primitives::U256;
use futures::future::join_all;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct QuoteRequest {
    pub chain: Chain,
    pub token_in: Token,
    pub token_out: Token,
    pub amount_in: U256,
    /// When set, every on-chain `eth_call` is pinned to this block.
    /// HTTP aggregator backends can't replay history and are filtered
    /// upstream when this is `Some`. `None` → live quote against latest.
    pub block_id: Option<BlockId>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Quote {
    pub backend: &'static str,
    #[serde(serialize_with = "serialize_u256")]
    pub amount_out: U256,
    pub gas_estimate: Option<u64>,
    pub gas_usd: Option<f64>,
    pub latency_ms: u128,
    /// Multi-hop path taken by the quote, when the backend surfaces it.
    /// Populated by aggregator backends (ODOS, Paraswap, KyberSwap,
    /// OpenOcean, LiFi, CoW Swap) from their response payloads. On-chain
    /// backends leave this as `None` because they ARE the route.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub route: Option<Vec<String>>,
}

fn serialize_u256<S: serde::Serializer>(
    value: &U256,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&value.to_string())
}

#[derive(Debug)]
pub struct BackendResult {
    pub name: &'static str,
    pub quote: Result<Quote, DexQuoteError>,
}

/// Fan out a `QuoteRequest` across every backend in `backends` in parallel,
/// applying a per-backend timeout. Results preserve the input ordering so the
/// rendered table is deterministic. Individual failures are captured as
/// `Err(...)` inside `BackendResult.quote` — a single broken backend never
/// fails the whole batch.
pub async fn quote_all(
    backends: &[Arc<dyn DexBackend>],
    request: &QuoteRequest,
    per_backend_timeout: Duration,
) -> Vec<BackendResult> {
    let futures = backends.iter().map(|backend| {
        let backend = backend.clone();
        let request = request.clone();
        async move {
            let name = backend.name();
            let start = Instant::now();
            let result =
                match tokio::time::timeout(per_backend_timeout, backend.quote(&request)).await {
                    Ok(Ok(mut q)) => {
                        if q.latency_ms == 0 {
                            q.latency_ms = start.elapsed().as_millis();
                        }
                        Ok(q)
                    }
                    Ok(Err(e)) => Err(e),
                    Err(_) => Err(DexQuoteError::Timeout {
                        backend: name,
                        ms: per_backend_timeout.as_millis() as u64,
                    }),
                };
            BackendResult {
                name,
                quote: result,
            }
        }
    });

    join_all(futures).await
}
