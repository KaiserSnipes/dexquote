//! Jupiter Swap v1 — the dominant Solana DEX aggregator.
//!
//! Routes through every major Solana DEX (Raydium, Orca, Meteora,
//! Phoenix, Lifinity, OpenBook, Dooar, Saros, 1Dex, FluxBeam, and
//! dozens more). Jupiter's response includes a structured `routePlan`
//! that labels each hop with its underlying DEX, so `dexquote route`
//! shows exactly which venues Jupiter picked.
//!
//! **Endpoint**: `lite-api.jup.ag` is the free/anonymous tier. The
//! legacy `quote-api.jup.ag` DNS-fails now; `api.jup.ag` is the paid
//! gateway with tighter rate limits for unauthenticated traffic.
//!
//! No API key required.

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "Jupiter";
const BASE_URL: &str = "https://lite-api.jup.ag/swap/v1/quote";

pub struct JupiterSwapBackend {
    client: reqwest::Client,
}

impl JupiterSwapBackend {
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

    pub fn supports(chain: Chain) -> bool {
        matches!(chain, Chain::Solana)
    }
}

impl Default for JupiterSwapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct JupResponse {
    #[serde(rename = "outAmount")]
    out_amount: String,
    #[serde(rename = "routePlan", default)]
    route_plan: Vec<JupRoutePlanStep>,
}

#[derive(Deserialize)]
struct JupRoutePlanStep {
    #[serde(rename = "swapInfo")]
    swap_info: JupSwapInfo,
}

#[derive(Deserialize)]
struct JupSwapInfo {
    /// Human-readable DEX name (e.g. "Raydium CLMM", "Orca Whirlpool",
    /// "Meteora DLMM", "Phoenix", "HumidiFi"). Jupiter curates this
    /// list — new DEXs appear as they're integrated.
    #[serde(default)]
    label: String,
}

#[async_trait::async_trait]
impl DexBackend for JupiterSwapBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        if request.chain != Chain::Solana {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let input_mint = request.token_in.address.as_solana().ok_or(
            DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            },
        )?;
        let output_mint = request.token_out.address.as_solana().ok_or(
            DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            },
        )?;

        let input_b58 = bs58::encode(input_mint).into_string();
        let output_b58 = bs58::encode(output_mint).into_string();

        // Jupiter's amount is the atomic unit (lamports / token base units).
        let amount_str = request.amount_in.to_string();

        let params = [
            ("inputMint", input_b58),
            ("outputMint", output_b58),
            ("amount", amount_str),
            ("slippageBps", "50".to_string()),
        ];

        let start = Instant::now();
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

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            // Jupiter returns 400 when there's no route.
            if status.as_u16() == 400 {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::decode(
                BACKEND_NAME,
                format!("http {status}: {}", text.chars().take(200).collect::<String>()),
            ));
        }

        let parsed: JupResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        let amount_out = U256::from_str(&parsed.out_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Flatten route plan labels with consecutive-dup collapse, same
        // as our EVM aggregator backends (ODOS, Paraswap, etc).
        let route = {
            let mut out: Vec<String> = Vec::new();
            for step in &parsed.route_plan {
                let label = &step.swap_info.label;
                if label.is_empty() {
                    continue;
                }
                if out.last().map(|s| s == label).unwrap_or(false) {
                    continue;
                }
                out.push(label.clone());
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
            gas_estimate: None,
            // Jupiter doesn't return a USD gas estimate in the quote
            // endpoint — the /swap endpoint does but we don't call it.
            gas_usd: None,
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}
