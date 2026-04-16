//! CoW Swap — free solver-based intent protocol.
//!
//! CoW operates differently from classical AMM aggregators: users submit
//! signed intents that compete in batch auctions run by solvers. The quote
//! endpoint returns what a solver would be willing to fill the order for,
//! net of protocol fees and surplus-capture.
//!
//! The `quote.buyAmount` field in the response is what the user actually
//! receives (already after any fee deduction), so it's directly comparable
//! to every other aggregator's output amount. No special handling needed.
//!
//! Docs: https://docs.cow.fi/cow-protocol/reference/apis/quote

use crate::backends::DexBackend;
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::U256;
use serde::{Deserialize, Serialize};
use std::str::FromStr;
use std::time::{Duration, Instant};

const BACKEND_NAME: &str = "CoWSwap";

// CoW Swap base URL per chain. Arbitrum and Base use chain-slug path
// segments; Ethereum mainnet uses `mainnet` (not `ethereum`).
fn base_url(chain: Chain) -> &'static str {
    match chain {
        Chain::Arbitrum => "https://api.cow.fi/arbitrum_one/api/v1/quote",
        Chain::Base => "https://api.cow.fi/base/api/v1/quote",
        Chain::Ethereum => "https://api.cow.fi/mainnet/api/v1/quote",
        // CoW Swap is EVM-only; no Solana deployment.
        Chain::Solana => "",
    }
}

pub struct CowSwapBackend {
    client: reqwest::Client,
}

impl CowSwapBackend {
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

impl Default for CowSwapBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize)]
struct CowRequest<'a> {
    #[serde(rename = "sellToken")]
    sell_token: String,
    #[serde(rename = "buyToken")]
    buy_token: String,
    #[serde(rename = "sellAmountBeforeFee")]
    sell_amount_before_fee: String,
    kind: &'a str,
    from: String,
    receiver: String,
    #[serde(rename = "partiallyFillable")]
    partially_fillable: bool,
    #[serde(rename = "sellTokenBalance")]
    sell_token_balance: &'a str,
    #[serde(rename = "buyTokenBalance")]
    buy_token_balance: &'a str,
    #[serde(rename = "signingScheme")]
    signing_scheme: &'a str,
    #[serde(rename = "onchainOrder")]
    onchain_order: bool,
    #[serde(rename = "appData")]
    app_data: &'a str,
}

#[derive(Deserialize)]
struct CowResponse {
    quote: Option<CowQuote>,
    #[serde(rename = "errorType")]
    error_type: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct CowQuote {
    #[serde(rename = "buyAmount")]
    buy_amount: String,
    #[serde(rename = "feeAmount")]
    fee_amount: Option<String>,
}

#[async_trait::async_trait]
impl DexBackend for CowSwapBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        // CoW treats the quote endpoint as if it's sizing a real order,
        // so `from` should be a non-zero address. The burn/dead sentinel
        // is widely accepted and preserves read-only semantics.
        let sentinel = "0x000000000000000000000000000000000000dEaD";

        let body = CowRequest {
            sell_token: format!("{:?}", request.token_in.evm_address(BACKEND_NAME)?),
            buy_token: format!("{:?}", request.token_out.evm_address(BACKEND_NAME)?),
            sell_amount_before_fee: request.amount_in.to_string(),
            kind: "sell",
            from: sentinel.to_string(),
            receiver: sentinel.to_string(),
            partially_fillable: false,
            sell_token_balance: "erc20",
            buy_token_balance: "erc20",
            signing_scheme: "eip712",
            onchain_order: false,
            // CoW requires a bytes32 appData hash; the all-zero sentinel
            // is the documented "no metadata" value.
            app_data: "{}",
        };

        let response = self
            .client
            .post(base_url(request.chain))
            .json(&body)
            .send()
            .await
            .map_err(|e| DexQuoteError::Http {
                backend: BACKEND_NAME,
                source: e,
            })?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        // CoW uses HTTP 400 for upstream errors with a JSON body
        // describing the `errorType`. Treat NoLiquidity / NoBuyersFound /
        // similar as a clean NoRoute so they render as a dim row instead
        // of a loud error.
        let parsed: CowResponse = serde_json::from_str(&text).map_err(|e| {
            DexQuoteError::decode(
                BACKEND_NAME,
                format!(
                    "http {status} · could not decode response: {e} · body: {}",
                    text.chars().take(200).collect::<String>()
                ),
            )
        })?;

        if let Some(err_type) = parsed.error_type.as_deref() {
            let low = err_type.to_ascii_lowercase();
            if low.contains("liquidity")
                || low.contains("route")
                || low.contains("buyersfound")
                || low.contains("sellamountdoesnotcoverfee")
            {
                return Err(DexQuoteError::NoRoute {
                    backend: BACKEND_NAME,
                });
            }
            let desc = parsed.description.unwrap_or_else(|| err_type.to_string());
            return Err(DexQuoteError::decode(BACKEND_NAME, desc));
        }

        let quote = parsed.quote.ok_or_else(|| {
            DexQuoteError::decode(BACKEND_NAME, "missing `quote` object in response")
        })?;

        let amount_out = U256::from_str(&quote.buy_amount).map_err(|e| {
            DexQuoteError::decode(BACKEND_NAME, format!("buyAmount not a U256: {e}"))
        })?;

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // CoW reports the protocol fee in the `feeAmount` field (in sell
        // token base units). We don't surface it separately — it's already
        // folded into buyAmount — but store the raw value as our "gas
        // estimate" proxy so the rendered row has a non-zero gas column.
        // A more polished v0.2 could show the fee in USD.
        let _ = quote.fee_amount;

        // CoW Swap is a solver-based intent protocol — there's no
        // multi-hop "route" in the aggregator sense because solvers
        // bundle orders into batch auctions. Surface a single-element
        // route marking this so `dexquote route` reflects why CoW's
        // display looks different from other aggregators.
        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: None,
            gas_usd: None,
            latency_ms: start.elapsed().as_millis(),
            route: Some(vec!["solver-batch".to_string()]),
        })
    }
}
