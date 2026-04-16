use crate::backends::DexBackend;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{Address, U256};
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{Duration, Instant};

const ODOS_QUOTE_URL: &str = "https://api.odos.xyz/sor/quote/v2";
const BACKEND_NAME: &str = "ODOS";

/// HTTP backend for the ODOS Smart Order Router. Calls the public
/// `api.odos.xyz/sor/quote/v2` endpoint which requires no API key for
/// low-volume use and returns an aggregated best-route quote across all
/// liquidity sources ODOS knows about on the target chain.
pub struct OdosBackend {
    client: reqwest::Client,
}

impl OdosBackend {
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

    /// Construct with a pre-built shared `reqwest::Client`. Preferred
    /// constructor when multiple HTTP backends are spun up together so they
    /// can reuse the same connection pool.
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }
}

impl Default for OdosBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize)]
struct OdosToken {
    #[serde(rename = "tokenAddress")]
    token_address: String,
    amount: String,
}

#[derive(Serialize)]
struct OdosOutToken {
    #[serde(rename = "tokenAddress")]
    token_address: String,
    proportion: f64,
}

#[derive(Serialize)]
struct OdosRequest {
    #[serde(rename = "chainId")]
    chain_id: u64,
    #[serde(rename = "inputTokens")]
    input_tokens: Vec<OdosToken>,
    #[serde(rename = "outputTokens")]
    output_tokens: Vec<OdosOutToken>,
    #[serde(rename = "userAddr")]
    user_addr: String,
    #[serde(rename = "slippageLimitPercent")]
    slippage_limit_percent: f64,
    #[serde(rename = "disableRFQs")]
    disable_rfqs: bool,
    compact: bool,
}

#[derive(Deserialize)]
struct OdosResponse {
    #[serde(rename = "outAmounts")]
    out_amounts: Vec<String>,
    #[serde(rename = "gasEstimate")]
    gas_estimate: Option<f64>,
    #[serde(rename = "gasEstimateValue")]
    gas_estimate_value: Option<f64>,
    /// Only populated when `compact: false`. Contains a structured DAG
    /// of the route ODOS selected: `nodes` are token steps, `links` are
    /// DEX hops labeled with the underlying venue name.
    #[serde(rename = "pathViz")]
    path_viz: Option<OdosPathViz>,
}

#[derive(Deserialize)]
struct OdosPathViz {
    #[serde(default)]
    links: Vec<OdosPathLink>,
}

#[derive(Deserialize)]
struct OdosPathLink {
    /// Human-readable name of the DEX at this hop (e.g. "Uniswap V3",
    /// "Curve", "Balancer V2"). ODOS occasionally splits a single hop
    /// across multiple venues; we dedupe consecutive duplicates at parse
    /// time so the display stays tight.
    #[serde(default)]
    label: String,
}

#[async_trait::async_trait]
impl DexBackend for OdosBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let body = OdosRequest {
            chain_id: request.chain.id(),
            input_tokens: vec![OdosToken {
                token_address: checksum(request.token_in.evm_address(BACKEND_NAME)?),
                amount: request.amount_in.to_string(),
            }],
            output_tokens: vec![OdosOutToken {
                token_address: checksum(request.token_out.evm_address(BACKEND_NAME)?),
                proportion: 1.0,
            }],
            user_addr: format!("{:?}", Address::ZERO),
            slippage_limit_percent: 0.3,
            disable_rfqs: true,
            // `compact: false` so the response includes `pathViz`, which
            // is how we surface the underlying route for v0.8's
            // `dexquote route` subcommand. Adds ~1kb to the response
            // body — negligible vs the round-trip cost.
            compact: false,
        };

        let start = Instant::now();
        let response = self
            .client
            .post(ODOS_QUOTE_URL)
            .json(&body)
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

        let parsed: OdosResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        let raw = parsed.out_amounts.first().ok_or_else(|| {
            DexQuoteError::decode(BACKEND_NAME, "outAmounts array was empty")
        })?;
        let amount_out = U256::from_str(raw).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outAmounts[0] not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Build route list from pathViz.links[].label. Dedupe
        // consecutive duplicates (ODOS splits some hops across
        // multiple venue instances) but keep meaningful re-entries
        // (e.g. a circular route through the same DEX twice).
        let route = parsed.path_viz.and_then(|pv| {
            let mut out: Vec<String> = Vec::new();
            for link in pv.links {
                if link.label.is_empty() {
                    continue;
                }
                if out.last().map(|s| s == &link.label).unwrap_or(false) {
                    continue;
                }
                out.push(link.label);
            }
            if out.is_empty() {
                None
            } else {
                Some(out)
            }
        });

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: parsed.gas_estimate.map(|g| g as u64),
            gas_usd: parsed.gas_estimate_value,
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}

fn checksum(address: Address) -> String {
    format!("{:?}", address)
}
