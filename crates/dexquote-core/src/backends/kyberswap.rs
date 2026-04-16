//! KyberSwap Aggregator — free public API, no auth required.
//!
//! Docs: https://docs.kyberswap.com/kyberswap-solutions/kyberswap-aggregator
//! The route endpoint returns a `routeSummary` with `outAmount` in base units.

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "KyberSwap";

fn base_url(chain: Chain) -> &'static str {
    match chain {
        Chain::Arbitrum => "https://aggregator-api.kyberswap.com/arbitrum/api/v1/routes",
        Chain::Base => "https://aggregator-api.kyberswap.com/base/api/v1/routes",
        Chain::Ethereum => "https://aggregator-api.kyberswap.com/ethereum/api/v1/routes",
        // KyberSwap's Solana endpoint was removed in 2025 (404). Solana
        // support pulled from their docs. Backend filters out via the
        // empty URL returning an HTTP error.
        Chain::Solana => "",
    }
}

pub struct KyberSwapBackend {
    client: reqwest::Client,
}

impl KyberSwapBackend {
    pub fn new() -> Self {
        Self::with_timeout(Duration::from_secs(5))
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        let client = reqwest::Client::builder()
            .timeout(timeout)
            .user_agent(concat!("dexquote/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for KyberSwapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct KyberResponse {
    data: Option<KyberData>,
    code: Option<i64>,
    message: Option<String>,
}

#[derive(Deserialize)]
struct KyberData {
    #[serde(rename = "routeSummary")]
    route_summary: RouteSummary,
}

#[derive(Deserialize)]
struct RouteSummary {
    #[serde(rename = "amountOut")]
    amount_out: Option<String>,
    #[serde(rename = "outAmount")]
    out_amount: Option<String>,
    #[serde(rename = "gasUsd")]
    gas_usd: Option<String>,
    gas: Option<String>,
    /// `route` is a `[[RouteHop, ...], ...]` — the outer array is
    /// parallel splits, the inner array is the sequential hops of
    /// that split. We flatten + dedupe to show the set of DEXs used.
    #[serde(default)]
    route: Vec<Vec<KyberHop>>,
}

#[derive(Deserialize)]
struct KyberHop {
    /// DEX identifier. KyberSwap uses slugs like "uniswapv3",
    /// "curve", "balancer-v2"; we pass them through as-is.
    #[serde(rename = "exchange", default)]
    exchange: String,
}

#[async_trait::async_trait]
impl DexBackend for KyberSwapBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let params = [
            ("tokenIn", format!("{:?}", request.token_in.evm_address(BACKEND_NAME)?)),
            ("tokenOut", format!("{:?}", request.token_out.evm_address(BACKEND_NAME)?)),
            ("amountIn", request.amount_in.to_string()),
        ];

        let response = self
            .client
            .get(base_url(request.chain))
            .header("x-client-id", "dexquote-cli")
            .query(&params)
            .send()
            .await
            .map_err(|e| DexQuoteError::Http {
                backend: BACKEND_NAME,
                source: e,
            })?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            return Err(DexQuoteError::decode(
                BACKEND_NAME,
                format!("http {status}: {}", text.chars().take(200).collect::<String>()),
            ));
        }

        let parsed: KyberResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        // KyberSwap uses code=0 for success; anything else is an error (often
        // "no route found").
        if parsed.code.unwrap_or(0) != 0 {
            let msg = parsed.message.unwrap_or_else(|| "no route".into());
            if msg.to_ascii_lowercase().contains("no route")
                || msg.to_ascii_lowercase().contains("not found")
            {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::decode(BACKEND_NAME, msg));
        }

        let summary = parsed
            .data
            .map(|d| d.route_summary)
            .ok_or_else(|| DexQuoteError::decode(BACKEND_NAME, "missing data.routeSummary"))?;

        let raw = summary
            .amount_out
            .or(summary.out_amount)
            .ok_or_else(|| DexQuoteError::decode(BACKEND_NAME, "missing amountOut in response"))?;

        let amount_out = U256::from_str(&raw).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("amountOut not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let gas_usd = summary
            .gas_usd
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok());
        let gas_estimate = summary.gas.as_deref().and_then(|s| s.parse::<u64>().ok());

        // Flatten the nested route arrays into a deduped ordered
        // list of DEXs. KyberSwap's route is `splits[hops[...]]`; we
        // walk splits first then hops within each split. Consecutive
        // duplicates collapse.
        let route = {
            let mut out: Vec<String> = Vec::new();
            for split in &summary.route {
                for hop in split {
                    if hop.exchange.is_empty() {
                        continue;
                    }
                    if out.last().map(|s| s == &hop.exchange).unwrap_or(false) {
                        continue;
                    }
                    out.push(hop.exchange.clone());
                }
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        };

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate,
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}
