//! OpenOcean v3 Solana swap quote.
//!
//! OpenOcean's Solana endpoint is a completely different API version
//! from its EVM endpoints (v3 vs v4), including a **different unit
//! convention**: the `amount` parameter is human units (e.g. `1` for
//! 1 SOL), not atomic lamports. We format the value via `format_amount`
//! at call time to match.
//!
//! OpenOcean internally bundles Jupiter + Titan and returns whichever
//! gave the best price — so shipping this backend gives us Titan's
//! pricing without needing Titan's WebSocket / Triton subscription.
//!
//! **Endpoint**: `open-api.openocean.finance/v3/solana/quote`. Free
//! and anonymous.

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
const BASE_URL: &str = "https://open-api.openocean.finance/v3/solana/quote";

pub struct OpenOceanSolanaBackend {
    client: reqwest::Client,
}

impl OpenOceanSolanaBackend {
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

impl Default for OpenOceanSolanaBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct OoResponse {
    code: Option<i64>,
    data: Option<OoData>,
}

#[derive(Deserialize)]
struct OoData {
    #[serde(rename = "outAmount")]
    out_amount: String,
    #[serde(default)]
    dexes: Vec<OoDex>,
}

#[derive(Deserialize, Default)]
struct OoDex {
    #[serde(default, rename = "dexCode")]
    dex_code: String,
}

#[async_trait::async_trait]
impl DexBackend for OpenOceanSolanaBackend {
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

        // OpenOcean v3 Solana takes HUMAN units for the amount
        // parameter. `1` means 1 SOL, not 1 lamport. This is the v3
        // convention and differs from the v4 EVM endpoint.
        let amount_human = format_amount(request.amount_in, request.token_in.decimals, 18);

        let params = [
            ("inTokenAddress", input_b58),
            ("outTokenAddress", output_b58),
            ("amount", amount_human),
            ("slippage", "1".to_string()),
            ("gasPrice", "1".to_string()),
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

        if !response.status().is_success() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let parsed: OoResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        if parsed.code.unwrap_or(200) != 200 {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let data = parsed.data.ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let amount_out = U256::from_str(&data.out_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Route from dexes[].dexCode — dedupe consecutive duplicates.
        let route = {
            let mut out: Vec<String> = Vec::new();
            for d in &data.dexes {
                if d.dex_code.is_empty() {
                    continue;
                }
                if out.last().map(|s| s == &d.dex_code).unwrap_or(false) {
                    continue;
                }
                out.push(d.dex_code.clone());
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
