//! Li.Fi — free public API (anonymous tier: ~200 req / 2 hr).
//!
//! Docs: https://docs.li.fi/api-reference
//! Li.Fi is cross-chain first, but handles same-chain quotes cleanly via
//! `/quote` with matching `fromChain` / `toChain`. Field names: `toAmount`
//! is the output in base units, `estimate.gasCosts[0].amountUSD` is the
//! dollar-denominated gas estimate.

use crate::backends::DexBackend;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "LiFi";
const BASE_URL: &str = "https://li.quest/v1/quote";

pub struct LiFiBackend {
    client: reqwest::Client,
}

impl LiFiBackend {
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

impl Default for LiFiBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct LiFiResponse {
    estimate: LiFiEstimate,
    /// Top-level list of underlying steps — one per DEX/bridge Li.Fi
    /// stitches together. For same-chain swaps this is usually a
    /// single step, but Li.Fi does split across multiple tools when
    /// beneficial.
    #[serde(rename = "includedSteps", default)]
    included_steps: Vec<LiFiStep>,
}

#[derive(Deserialize)]
struct LiFiStep {
    /// Slug identifier of the underlying aggregator or DEX
    /// (e.g. "uniswap", "curve", "1inch", "sushiswap").
    #[serde(default)]
    tool: String,
}

#[derive(Deserialize)]
struct LiFiEstimate {
    #[serde(rename = "toAmount")]
    to_amount: String,
    #[serde(rename = "gasCosts")]
    gas_costs: Option<Vec<LiFiGasCost>>,
}

#[derive(Deserialize)]
struct LiFiGasCost {
    #[serde(rename = "amountUSD")]
    amount_usd: Option<String>,
    estimate: Option<String>,
}

#[async_trait::async_trait]
impl DexBackend for LiFiBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let chain_id = request.chain.id().to_string();
        let params = [
            ("fromChain", chain_id.clone()),
            ("toChain", chain_id),
            ("fromToken", format!("{:?}", request.token_in.evm_address(BACKEND_NAME)?)),
            ("toToken", format!("{:?}", request.token_out.evm_address(BACKEND_NAME)?)),
            ("fromAmount", request.amount_in.to_string()),
            // Li.Fi rejects the zero address for fromAddress (error code 1011
            // "Zero address is provided"). The burn/dead address is widely
            // accepted by aggregators as a read-only sentinel.
            (
                "fromAddress",
                "0x000000000000000000000000000000000000dEaD".to_string(),
            ),
            ("slippage", "0.01".to_string()),
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
            // 404 from Li.Fi generally means no route for this pair.
            if status.as_u16() == 404 {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::decode(
                BACKEND_NAME,
                format!("http {status}: {}", text.chars().take(200).collect::<String>()),
            ));
        }

        let parsed: LiFiResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        let amount_out = U256::from_str(&parsed.estimate.to_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("toAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let (gas_estimate, gas_usd) = match parsed.estimate.gas_costs.as_ref() {
            Some(costs) if !costs.is_empty() => {
                let first = &costs[0];
                let units = first
                    .estimate
                    .as_deref()
                    .and_then(|s| s.parse::<u64>().ok());
                let usd = first
                    .amount_usd
                    .as_deref()
                    .and_then(|s| s.parse::<f64>().ok());
                (units, usd)
            }
            _ => (None, None),
        };

        // Flatten includedSteps[].tool into a deduped route list.
        // Li.Fi's `tool` slugs are lowercase by convention.
        let route = {
            let mut out: Vec<String> = Vec::new();
            for step in &parsed.included_steps {
                if step.tool.is_empty() {
                    continue;
                }
                if out.last().map(|s| s == &step.tool).unwrap_or(false) {
                    continue;
                }
                out.push(step.tool.clone());
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
