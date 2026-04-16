//! Li.Fi Solana quote endpoint.
//!
//! Same host as the EVM `lifi.rs` backend (`li.quest/v1/quote`) but
//! with Solana-specific params: `fromChain=SOL`, `toChain=SOL`, and
//! a 32-char base58 `fromAddress` (the 33-char EVM placeholder
//! doesn't work — Li.Fi validates it as a pubkey shape).
//!
//! Li.Fi internally routes through OKX DEX and other downstream
//! aggregators for Solana, so this backend gets us pricing from
//! venues we can't hit directly (no OKX, no 1inch Solana).
//!
//! Rate limit: 75 req/min anonymous (per `ratelimit-limit` header).

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "LiFi";
const BASE_URL: &str = "https://li.quest/v1/quote";

/// System program pubkey — 32 ones in base58 decodes to 32 zero
/// bytes. Li.Fi accepts any valid 32-char base58 pubkey for read-only
/// quoting.
const PLACEHOLDER_FROM_ADDRESS: &str = "11111111111111111111111111111111";

pub struct LiFiSolanaBackend {
    client: reqwest::Client,
}

impl LiFiSolanaBackend {
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

impl Default for LiFiSolanaBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct LiFiResponse {
    estimate: LiFiEstimate,
    #[serde(rename = "includedSteps", default)]
    included_steps: Vec<LiFiStep>,
}

#[derive(Deserialize)]
struct LiFiEstimate {
    #[serde(rename = "toAmount")]
    to_amount: String,
}

#[derive(Deserialize)]
struct LiFiStep {
    #[serde(default)]
    tool: String,
}

#[async_trait::async_trait]
impl DexBackend for LiFiSolanaBackend {
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

        let params = [
            ("fromChain", "SOL".to_string()),
            ("toChain", "SOL".to_string()),
            ("fromToken", input_b58),
            ("toToken", output_b58),
            ("fromAmount", request.amount_in.to_string()),
            ("fromAddress", PLACEHOLDER_FROM_ADDRESS.to_string()),
            ("slippage", "0.01".to_string()),
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
            if status.as_u16() == 404 {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
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
            gas_estimate: None,
            gas_usd: None,
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}
