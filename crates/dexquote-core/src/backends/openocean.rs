//! OpenOcean aggregator — free public API.
//!
//! Docs: https://apis.openocean.finance
//! The `/quote` endpoint takes human-denominated amounts (not base units) via
//! the `amount` parameter, along with token addresses and a slippage value.
//! Response field: `outAmount` is in base units of the output token.

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use crate::token::format_amount;
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "OpenOcean";

fn base_url(chain: Chain) -> &'static str {
    match chain {
        Chain::Arbitrum => "https://open-api.openocean.finance/v4/arbitrum/quote",
        Chain::Base => "https://open-api.openocean.finance/v4/base/quote",
        // OpenOcean uses "eth" as the Ethereum mainnet slug, not
        // "ethereum" — so url_slug() can't be used here.
        Chain::Ethereum => "https://open-api.openocean.finance/v4/eth/quote",
        // OpenOcean does have a Solana endpoint but it uses a
        // completely different API version (v3) and human-units
        // amount convention. That's handled by a separate Solana
        // backend; this EVM file doesn't serve Solana.
        Chain::Solana => "",
    }
}

pub struct OpenOceanBackend {
    client: reqwest::Client,
}

impl OpenOceanBackend {
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

impl Default for OpenOceanBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct OpenOceanResponse {
    code: Option<i64>,
    data: Option<OpenOceanData>,
    error: Option<String>,
}

#[derive(Deserialize)]
struct OpenOceanData {
    #[serde(rename = "outAmount")]
    out_amount: String,
    #[serde(rename = "estimatedGas")]
    estimated_gas: Option<serde_json::Value>,
    /// When present, describes the underlying route(s) OpenOcean picked.
    /// Only populated on certain pair types and only by some chain
    /// endpoints; we gracefully fall back to `None` when missing.
    #[serde(default)]
    path: Option<OpenOceanPath>,
}

#[derive(Deserialize)]
struct OpenOceanPath {
    #[serde(default)]
    routes: Vec<OpenOceanRoute>,
}

#[derive(Deserialize)]
struct OpenOceanRoute {
    #[serde(default)]
    #[serde(rename = "subRoutes")]
    sub_routes: Vec<OpenOceanSubRoute>,
}

#[derive(Deserialize)]
struct OpenOceanSubRoute {
    #[serde(default)]
    dexes: Vec<OpenOceanDex>,
}

#[derive(Deserialize)]
struct OpenOceanDex {
    #[serde(default)]
    dex: String,
}

#[async_trait::async_trait]
impl DexBackend for OpenOceanBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        // OpenOcean expects the amount in human units with up to 18 decimals
        // — e.g. "1.5" for 1.5 ETH. We use the existing formatter with a
        // generous max_frac_digits to avoid losing precision.
        let amount_human = format_amount(request.amount_in, request.token_in.decimals, 18);

        let params = [
            (
                "inTokenAddress",
                format!("{:?}", request.token_in.evm_address(BACKEND_NAME)?),
            ),
            (
                "outTokenAddress",
                format!("{:?}", request.token_out.evm_address(BACKEND_NAME)?),
            ),
            ("amount", amount_human),
            // Slippage in basis points (100 = 1%). OpenOcean's quote route
            // requires it even though it doesn't affect the output amount.
            ("slippage", "100".to_string()),
            (
                "gasPrice",
                // OpenOcean requires gasPrice in gwei. Use a reasonable default
                // for Arbitrum (0.1 gwei); it doesn't affect the outAmount.
                "0.1".to_string(),
            ),
        ];

        let response = self
            .client
            .get(base_url(request.chain))
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

        let parsed: OpenOceanResponse =
            response.json().await.map_err(|e| DexQuoteError::Http {
                backend: BACKEND_NAME,
                source: e,
            })?;

        if parsed.code.unwrap_or(200) != 200 {
            let msg = parsed.error.unwrap_or_else(|| "upstream error".into());
            if msg.to_ascii_lowercase().contains("no route")
                || msg.to_ascii_lowercase().contains("not found")
            {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::decode(BACKEND_NAME, msg));
        }

        let data = parsed
            .data
            .ok_or_else(|| DexQuoteError::decode(BACKEND_NAME, "missing data field"))?;

        let amount_out = U256::from_str(&data.out_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let gas_estimate = data.estimated_gas.as_ref().and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        });

        // Extract route from path.routes[].subRoutes[].dexes[].dex if
        // present; gracefully return None when OpenOcean doesn't
        // include routing info for this chain/endpoint combination.
        let route = data.path.and_then(|p| {
            let mut out: Vec<String> = Vec::new();
            for r in &p.routes {
                for sub in &r.sub_routes {
                    for dex in &sub.dexes {
                        if dex.dex.is_empty() {
                            continue;
                        }
                        if out.last().map(|s| s == &dex.dex).unwrap_or(false) {
                            continue;
                        }
                        out.push(dex.dex.clone());
                    }
                }
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
            gas_estimate,
            gas_usd: None, // OpenOcean doesn't return a USD gas estimate
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}
