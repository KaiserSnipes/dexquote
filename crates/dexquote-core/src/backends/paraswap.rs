//! Paraswap (rebranded Velora) aggregator — free public API.
//!
//! Docs: https://developers.paraswap.network/
//! We hit `/prices` with `side=SELL` and read `priceRoute.destAmount`.

use crate::backends::DexBackend;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "Paraswap";
const BASE_URL: &str = "https://apiv5.paraswap.io/prices";

pub struct ParaswapBackend {
    client: reqwest::Client,
}

impl ParaswapBackend {
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

impl Default for ParaswapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct ParaswapResponse {
    #[serde(rename = "priceRoute")]
    price_route: PriceRoute,
}

#[derive(Deserialize)]
struct PriceRoute {
    #[serde(rename = "destAmount")]
    dest_amount: String,
    #[serde(rename = "gasCostUSD")]
    gas_cost_usd: Option<String>,
    #[serde(rename = "gasCost")]
    gas_cost: Option<String>,
    /// Top-level route: one entry per (source, dest) pair; nested
    /// swaps are split across multiple exchanges inside each entry.
    #[serde(rename = "bestRoute", default)]
    best_route: Vec<ParaswapRouteEntry>,
}

#[derive(Deserialize)]
struct ParaswapRouteEntry {
    #[serde(default)]
    swaps: Vec<ParaswapSwap>,
}

#[derive(Deserialize)]
struct ParaswapSwap {
    #[serde(rename = "swapExchanges", default)]
    swap_exchanges: Vec<ParaswapExchange>,
}

#[derive(Deserialize)]
struct ParaswapExchange {
    /// DEX name used at this hop (e.g. "UniswapV3", "CurveV2",
    /// "BalancerV2"). Note that Paraswap uses its own naming scheme
    /// so some names differ from other aggregators' labels.
    #[serde(default)]
    exchange: String,
}

#[async_trait::async_trait]
impl DexBackend for ParaswapBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let params = [
            ("srcToken", format!("{:?}", request.token_in.evm_address(BACKEND_NAME)?)),
            ("destToken", format!("{:?}", request.token_out.evm_address(BACKEND_NAME)?)),
            ("amount", request.amount_in.to_string()),
            ("srcDecimals", request.token_in.decimals.to_string()),
            ("destDecimals", request.token_out.decimals.to_string()),
            ("network", request.chain.id().to_string()),
            ("side", "SELL".to_string()),
            ("version", "6.2".to_string()),
        ];

        let response = self
            .client
            .get(BASE_URL)
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
            // Paraswap returns 400 when there's no route; treat that as NoRoute
            // for cleaner UX rather than surfacing as an error.
            if status.as_u16() == 400 && text.to_ascii_lowercase().contains("no route") {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::decode(
                BACKEND_NAME,
                format!("http {status}: {}", text.chars().take(200).collect::<String>()),
            ));
        }

        let parsed: ParaswapResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        let amount_out = U256::from_str(&parsed.price_route.dest_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("destAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let gas_usd = parsed
            .price_route
            .gas_cost_usd
            .as_deref()
            .and_then(|s| s.parse::<f64>().ok());
        let gas_estimate = parsed
            .price_route
            .gas_cost
            .as_deref()
            .and_then(|s| s.parse::<u64>().ok());

        // Flatten bestRoute → swaps → swapExchanges into a deduped
        // list of DEX names. Consecutive duplicates are collapsed
        // (Paraswap occasionally splits a single hop across multiple
        // pools of the same venue).
        let route = {
            let mut out: Vec<String> = Vec::new();
            for entry in &parsed.price_route.best_route {
                for swap in &entry.swaps {
                    for ex in &swap.swap_exchanges {
                        if ex.exchange.is_empty() {
                            continue;
                        }
                        if out.last().map(|s| s == &ex.exchange).unwrap_or(false) {
                            continue;
                        }
                        out.push(ex.exchange.clone());
                    }
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
