//! Jupiter Ultra — Jupiter's newer "iris" router.
//!
//! Different endpoint and router from the Swap v1 backend — both land
//! on Solana quotes but take different routing paths, so shipping them
//! as separate backends surfaces the price diversity as two rows in
//! the table.
//!
//! **Endpoint**: `lite-api.jup.ag/ultra/v1/order`. Same `lite-api`
//! host as Swap v1, anonymous free tier.
//!
//! **Taker placeholder quirk**: Ultra's `/order` endpoint expects a
//! `taker` address. For pure quoting we pass the system program ID
//! (`11111111111111111111111111111111`) — 32 ones in base58, decodes
//! to all-zero bytes. Ultra responds with `errorMessage: "Insufficient
//! funds"` because the placeholder has no balance, but the `outAmount`
//! and `routePlan` fields are still populated and valid. We
//! deliberately ignore the error message as long as `outAmount` parses.

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "JupUltra";
const BASE_URL: &str = "https://lite-api.jup.ag/ultra/v1/order";

/// System program placeholder — 32 ones in base58 decodes to 32 zero
/// bytes. Any real caller who wants to execute would pass a real
/// signer pubkey; we only want the quote fields, so zero works.
const PLACEHOLDER_TAKER: &str = "11111111111111111111111111111111";

pub struct JupiterUltraBackend {
    client: reqwest::Client,
}

impl JupiterUltraBackend {
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

impl Default for JupiterUltraBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct UltraResponse {
    #[serde(rename = "outAmount", default)]
    out_amount: Option<String>,
    #[serde(rename = "routePlan", default)]
    route_plan: Vec<UltraRouteStep>,
}

#[derive(Deserialize)]
struct UltraRouteStep {
    #[serde(rename = "swapInfo", default)]
    swap_info: UltraSwapInfo,
}

#[derive(Deserialize, Default)]
struct UltraSwapInfo {
    #[serde(default)]
    label: String,
}

#[async_trait::async_trait]
impl DexBackend for JupiterUltraBackend {
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
        let amount_str = request.amount_in.to_string();

        let params = [
            ("inputMint", input_b58),
            ("outputMint", output_b58),
            ("amount", amount_str),
            ("taker", PLACEHOLDER_TAKER.to_string()),
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

        // Ultra returns 200 even when errorMessage is set (we ignore
        // that error as long as outAmount is populated). Other HTTP
        // status codes are hard failures.
        let status = response.status();
        if !status.is_success() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let parsed: UltraResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        let raw = parsed.out_amount.ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let amount_out = U256::from_str(&raw).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

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
            gas_usd: None,
            latency_ms: start.elapsed().as_millis(),
            route,
        })
    }
}
