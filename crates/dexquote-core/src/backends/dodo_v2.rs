//! DODO V2 (Proactive Market Maker) backend for Ethereum mainnet.
//!
//! DODO uses a PMM (Proactive Market Maker) pricing curve that behaves
//! roughly like a constant-product AMM near the oracle price but flattens
//! out in the tails — giving the deepest liquidity exactly at the peg.
//! Every pool type (DVM, DSP, DPP) exposes the same quote ABI:
//!
//! - `querySellBase(trader, payBaseAmount)` → `(receiveQuoteAmount, mtFee)`
//! - `querySellQuote(trader, payQuoteAmount)` → `(receiveBaseAmount, mtFee)`
//!
//! Both are `view` functions so a straight `eth_call` returns the quote
//! without any Vault-style revert-with-data dance. This makes DODO V2 one
//! of the simplest on-chain backends to integrate.
//!
//! Like Curve and Maverick V2, we maintain a hand-verified pool table —
//! DODO has no on-chain factory lookup that maps `(tokenA, tokenB) → pool`
//! and the aggregator helper contract isn't Etherscan-verified. Every
//! entry in `ETHEREUM_POOLS` was confirmed via `_BASE_TOKEN_()` /
//! `_QUOTE_TOKEN_()` calls against `ethereum.publicnode.com`. Pool
//! candidates came from GeckoTerminal's DODO PMM Ethereum pool list
//! (`api.geckoterminal.com/.../dexes/dodo-pmm-ethereum/pools`), sorted
//! by 24h volume because PMM pools are volume-first (high turnover
//! despite moderate TVL).

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use futures::future::join_all;
use std::time::Instant;

const BACKEND_NAME: &str = "DODO";

const GAS_ESTIMATE_DODO: u64 = 180_000;

/// A single DODO V2 pool with its base and quote tokens in the canonical
/// ordering reported on-chain by `_BASE_TOKEN_()` and `_QUOTE_TOKEN_()`.
/// The backend flips `sell_base` vs `sell_quote` based on which side of
/// this pair the user is paying in.
struct PoolSpec {
    pool: Address,
    base_token: Address,
    quote_token: Address,
}

/// Ethereum mainnet DODO V2 pool table. Volume-ranked from
/// GeckoTerminal's `dodo-pmm-ethereum` dex listing; each pool verified
/// live via `_BASE_TOKEN_()` / `_QUOTE_TOKEN_()` calls at implementation
/// time.
const ETHEREUM_POOLS: &[PoolSpec] = &[
    // USDT / USDC — the deepest DODO PMM pool on mainnet.
    // ~$308k TVL, ~$26M 24h volume. `_BASE_TOKEN_` = USDT,
    // `_QUOTE_TOKEN_` = USDC verified live.
    // Source: https://www.geckoterminal.com/eth/pools/0x04571c32a4e1c5f39bc3a238cb95b215058c432c
    PoolSpec {
        pool: address!("04571c32a4e1c5f39bc3a238cb95b215058c432c"),
        base_token: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
        quote_token: address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), // USDC
    },
    // DAI / USDT — secondary DODO stable pool.
    // ~$17k TVL, ~$481k 24h volume. Verified `_BASE_TOKEN_` = DAI,
    // `_QUOTE_TOKEN_` = USDT.
    // Source: https://www.geckoterminal.com/eth/pools/0x3058ef90929cb8180174d74c507176cca6835d73
    PoolSpec {
        pool: address!("3058ef90929cb8180174d74c507176cca6835d73"),
        base_token: address!("6B175474E89094C44Da98b954EedeAC495271d0F"), // DAI
        quote_token: address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
    },
];

sol! {
    #[sol(rpc)]
    interface IDodoV2Pool {
        /// Sell `payBaseAmount` of the pool's base token for quote token.
        /// `trader` influences MT fees for whitelist cases; `address(0)`
        /// is always safe for quoting.
        function querySellBase(address trader, uint256 payBaseAmount)
            external
            view
            returns (uint256 receiveQuoteAmount, uint256 mtFee);

        /// Sell `payQuoteAmount` of the quote token for base token.
        function querySellQuote(address trader, uint256 payQuoteAmount)
            external
            view
            returns (uint256 receiveBaseAmount, uint256 mtFee);
    }
}

pub struct DodoV2Backend {
    ctx: OnChainContext,
}

impl DodoV2Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn pools_for(chain: Chain) -> &'static [PoolSpec] {
        match chain {
            Chain::Ethereum => ETHEREUM_POOLS,
            Chain::Arbitrum => &[],
            Chain::Base => &[],
            Chain::Solana => &[],
        }
    }

    pub fn supports(chain: Chain) -> bool {
        !Self::pools_for(chain).is_empty()
    }
}

/// One pool candidate for the `querySell*` fan-out. Owns the pool
/// address and a flag indicating which direction the user's pay-in hits
/// — `is_base_in = true` means the user is paying the pool's base token
/// (→ call `querySellBase`); false means they're paying the quote token.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    pool: Address,
    is_base_in: bool,
}

fn find_matches(
    pools: &'static [PoolSpec],
    token_in: Address,
    token_out: Address,
) -> Vec<Candidate> {
    pools
        .iter()
        .filter_map(|spec| {
            if spec.base_token == token_in && spec.quote_token == token_out {
                Some(Candidate {
                    pool: spec.pool,
                    is_base_in: true,
                })
            } else if spec.quote_token == token_in && spec.base_token == token_out {
                Some(Candidate {
                    pool: spec.pool,
                    is_base_in: false,
                })
            } else {
                None
            }
        })
        .collect()
}

#[async_trait::async_trait]
impl DexBackend for DodoV2Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        let candidates = find_matches(
            Self::pools_for(request.chain),
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
        );

        if candidates.is_empty() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let provider = self.ctx.provider.clone();
        let amount_in = request.amount_in;
        let trader = Address::ZERO;
        let block_id = request.block_id;

        let calls = candidates.into_iter().map(|cand| {
            let provider = provider.clone();
            async move {
                let pool = IDodoV2Pool::new(cand.pool, provider);
                if cand.is_base_in {
                    let call = pool.querySellBase(trader, amount_in);
                    let call = match block_id {
                        Some(id) => call.block(id),
                        None => call,
                    };
                    call.call().await.ok().map(|ret| ret.receiveQuoteAmount)
                } else {
                    let call = pool.querySellQuote(trader, amount_in);
                    let call = match block_id {
                        Some(id) => call.block(id),
                        None => call,
                    };
                    call.call().await.ok().map(|ret| ret.receiveBaseAmount)
                }
            }
        });

        let amount_out = join_all(calls)
            .await
            .into_iter()
            .flatten()
            .filter(|a| !a.is_zero())
            .max()
            .ok_or(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            })?;

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_DODO));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_DODO),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}

// Suppress unused-import warning for U256 if future edits remove the
// direct use — the sol! macro generates code that references it.
#[allow(dead_code)]
fn _u256_usage_hint() -> U256 {
    U256::ZERO
}
