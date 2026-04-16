//! Raydium Trade API — the flagship Solana DEX's own route API.
//!
//! Raydium is a single-DEX backend (it only ever routes through
//! Raydium-owned pools — CPMM, CLMM, stable-ng), but it's worth
//! shipping as a comparison point against Jupiter and the other
//! aggregators. If Raydium's internal pools alone beat an aggregator's
//! spread, that's a clean signal about where Solana liquidity actually
//! sits.
//!
//! **Endpoint**: `transaction-v1.raydium.io/compute/swap-base-in`.
//! Free and anonymous.
//!
//! **Route label caveat**: Raydium's response has `routePlan[].poolId`
//! but no DEX label — all hops are Raydium pools by definition. We
//! hardcode `"Raydium"` per hop so `dexquote route` shows e.g.
//! `Raydium → Raydium` for two-hop trades. Informative because it
//! tells users this quote didn't bridge through another DEX.

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "Raydium";
const BASE_URL: &str = "https://transaction-v1.raydium.io/compute/swap-base-in";

pub struct RaydiumBackend {
    client: reqwest::Client,
}

impl RaydiumBackend {
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

impl Default for RaydiumBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Deserialize)]
struct RaydiumResponse {
    success: Option<bool>,
    data: Option<RaydiumData>,
}

#[derive(Deserialize)]
struct RaydiumData {
    #[serde(rename = "outputAmount")]
    output_amount: Option<String>,
    #[serde(rename = "routePlan", default)]
    route_plan: Vec<RaydiumRouteStep>,
}

#[derive(Deserialize, Default)]
struct RaydiumRouteStep {
    // No DEX label — Raydium only routes through its own pools.
    #[serde(rename = "poolId", default)]
    _pool_id: String,
}

#[async_trait::async_trait]
impl DexBackend for RaydiumBackend {
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
            ("slippageBps", "50".to_string()),
            ("txVersion", "V0".to_string()),
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

        let parsed: RaydiumResponse = response.json().await.map_err(|e| DexQuoteError::Http {
            backend: BACKEND_NAME,
            source: e,
        })?;

        if matches!(parsed.success, Some(false)) {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let data = parsed.data.ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;
        let raw = data.output_amount.ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        let amount_out = U256::from_str(&raw).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("outputAmount not a decimal U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // All hops are Raydium pools by definition — no DEX label in
        // the response, so we synthesize one per hop. For a single-hop
        // quote the route is ["Raydium"]; for a multi-hop it's
        // ["Raydium", "Raydium", …] signaling a pure-Raydium path.
        let route = if data.route_plan.is_empty() {
            None
        } else {
            Some(
                data.route_plan
                    .iter()
                    .map(|_| "Raydium".to_string())
                    .collect(),
            )
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
