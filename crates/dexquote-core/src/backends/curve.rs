//! Curve Finance direct-pool quoting.
//!
//! Curve pools come in two flavours that differ only in the `get_dy` ABI:
//!
//! - **Crypto pools** (Tricrypto-NG and friends) use
//!   `get_dy(uint256 i, uint256 j, uint256 dx) returns (uint256)`.
//! - **StableSwap-NG pools** use
//!   `get_dy(int128 i, int128 j, uint256 dx) returns (uint256)`.
//!
//! Every pool in the static tables declares which ABI it expects via its
//! `kind` field, and the backend dispatches the right `sol!` binding.
//! Pool coin compositions are stored as `&'static [Address]` slices so
//! 2-coin stablecoin pools don't have to pad to a fixed length.
//!
//! **Pool verification rule**: every entry in the pool tables must be
//! confirmed by calling `coins(i)` on the pool contract from a real RPC
//! AND by running a sample `get_dy` call to confirm the ABI flavour.
//! Unverified addresses never land here.
//!
//! **Dynamic discovery (v0.3+):** in addition to the hand-verified static
//! tables, the backend lazily queries the Curve API at `api.curve.finance`
//! on first quote per chain to fetch the full live pool list. Discovered
//! pools are cached in an in-process `OnceCell` and merged with the
//! static set. This removes the manual maintenance burden for the long
//! tail of pools while keeping the hand-verified ones as a fast path.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, Address, U256};
use alloy::sol;
use futures::future::join_all;
use serde::Deserialize;
use std::str::FromStr;
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

const BACKEND_NAME: &str = "Curve";

const GAS_ESTIMATE_CURVE: u64 = 230_000;

/// Which `get_dy` signature the pool exposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PoolKind {
    /// `get_dy(uint256 i, uint256 j, uint256 dx) returns (uint256)` —
    /// Tricrypto-NG and later crypto pools.
    Crypto,
    /// `get_dy(int128 i, int128 j, uint256 dx) returns (uint256)` —
    /// StableSwap and StableSwap-NG pools.
    Stable,
}

struct PoolSpec {
    address: Address,
    /// Tokens in the order reported by `coins(i)` — i.e. the indices
    /// passed to `get_dy`. Variable-length to accommodate 2-coin stable
    /// pools and 3-coin tricrypto pools without padding.
    coins: &'static [Address],
    kind: PoolKind,
}

/// Owned mirror of `PoolSpec` for dynamically-discovered pools. Same
/// shape, different lifetime. We intentionally don't share a single type
/// because the static tables are `&'static`-friendly (compile-time
/// constants) while the dynamic ones are heap-allocated `Vec`s.
///
/// `usd_total` is the TVL reported by the Curve API — used to rank
/// discovered candidates before the parallel `get_dy` fanout, so that
/// mainnet (which has ~1000 pools across registries) doesn't explode the
/// RPC budget. Arbitrum/Base typically see <20 matches per query so the
/// cap never bites on those chains.
#[derive(Debug, Clone)]
struct DiscoveredPool {
    address: Address,
    coins: Vec<Address>,
    kind: PoolKind,
    usd_total: f64,
}

/// Maximum number of dynamic-discovery candidates to race per quote.
/// The sort step ensures we're keeping the 8 highest-TVL pools, so
/// the cap trades tail-pool coverage for RPC budget sanity — a fair
/// deal on mainnet where the long tail is dominated by dead/dust pools
/// anyway.
const MAX_DYNAMIC_CANDIDATES: usize = 8;

/// Arbitrum pools. Verified live at implementation time via `coins(i)`
/// and a `get_dy` sanity check.
const ARBITRUM_POOLS: &[PoolSpec] = &[
    // Tricrypto-USDT (Arbitrum). USDT / WBTC / WETH.
    // Source: https://arbiscan.io/address/0x960ea3e3C7FB317332d990873d354E18d7645590
    PoolSpec {
        address: address!("960ea3e3C7FB317332d990873d354E18d7645590"),
        coins: &[
            address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"), // USDT
            address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"), // WBTC
            address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"), // WETH
        ],
        kind: PoolKind::Crypto,
    },
];

/// Base pools. Verified live against mainnet.base.org during
/// implementation: each `coins(i)` call returned the address below and
/// a sample `get_dy` call returned non-zero output for the ABI variant
/// marked in `kind`.
/// Ethereum mainnet pools. Static fast path — dynamic discovery via the
/// Curve API covers the long tail (and is TVL-capped at 8 candidates
/// before fanout to keep the RPC budget sane given mainnet's ~1000 pools).
/// The canonical 3pool is included here because it's THE stablecoin pool
/// on mainnet and every USDC/USDT/DAI quote should hit it without the
/// API round-trip.
const ETHEREUM_POOLS: &[PoolSpec] = &[
    // 3pool (DAI/USDC/USDT) — the canonical mainnet stablecoin pool.
    // ~$163M TVL. Uses int128 indices. Verified via `coins(0..2)` on
    // ethereum.publicnode.com returning DAI/USDC/USDT respectively.
    PoolSpec {
        address: address!("bEbc44782C7dB0a1A60Cb6fe97d0b483032FF1C7"),
        coins: &[
            address!("6B175474E89094C44Da98b954EedeAC495271d0F"), // DAI
            address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), // USDC
            address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
        ],
        kind: PoolKind::Stable,
    },
];

const BASE_POOLS: &[PoolSpec] = &[
    // crvUSD / tBTC / WETH tricrypto-ng. Top Base factory-tricrypto pool
    // by TVL (~$620k at verification). Uses uint256 indices.
    PoolSpec {
        address: address!("6e53131F68a034873b6bFA15502aF094Ef0c5854"),
        coins: &[
            address!("417Ac0e078398C154EdFadD9Ef675d30Be60Af93"), // crvUSD
            address!("236aa50979D5f3De3Bd1Eeb40E81137F22ab794b"), // tBTC
            address!("4200000000000000000000000000000000000006"), // WETH (Base)
        ],
        kind: PoolKind::Crypto,
    },
    // superOETHb / WETH stable-ng. Top Base LST pool (~$22.7M TVL at
    // verification — by far the biggest Curve pool on Base).
    PoolSpec {
        address: address!("302A94E3C28c290EAF2a4605FC52e11Eb915f378"),
        coins: &[
            address!("4200000000000000000000000000000000000006"), // WETH
            address!("DBFeFD2e8460a6Ee4955A68582F85708BAEA60A3"), // superOETHb
        ],
        kind: PoolKind::Stable,
    },
    // USDC / scrvUSD stable-ng. Savings crvUSD, $1.2M TVL. Covers the
    // native-USDC-to-crvUSD path that no other Base backend quotes.
    PoolSpec {
        address: address!("5aB01ee6208596f2204B85bDFA39d34c2aDD98F6"),
        coins: &[
            address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            address!("646A737B9B6024e49f5908762B3fF73e65B5160c"), // scrvUSD
        ],
        kind: PoolKind::Stable,
    },
    // USDSM / USDC stable-ng. $1M TVL. Stable-stable coverage.
    PoolSpec {
        address: address!("33885a2851f68B02863cDA6a7622d26fB1172884"),
        coins: &[
            address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            address!("26c358F7c5FEdb20A6DDEF108CD91eFb6B8dA0Cb"), // USDSM
        ],
        kind: PoolKind::Stable,
    },
    // NOTE: there's a USDC/USDbC stable-ng pool at
    // `0xD4e59bfd7bCe4A5Bc1Ee12Ea930C7495831D6aeF` listed by the Curve
    // API, but verified live to be a dead pool: 1000 USDC quotes ~1.01
    // USDbC and vice versa. We deliberately exclude it. If a working
    // USDC↔USDbC pool ships in the future, drop it back in here.
];

sol! {
    #[sol(rpc)]
    interface ICurveCrypto {
        // Tricrypto-NG and later crypto pools — uint256 indices.
        function get_dy(uint256 i, uint256 j, uint256 dx) external view returns (uint256);
    }

    #[sol(rpc)]
    interface ICurveStable {
        // StableSwap-NG — int128 indices.
        function get_dy(int128 i, int128 j, uint256 dx) external view returns (uint256);
    }
}

pub struct CurveBackend {
    ctx: OnChainContext,
    /// Lazily-populated cache of pools fetched from the Curve API.
    /// Per-chain caches keyed on the active request's chain — for v0.3
    /// dexquote only ever runs one chain per backend instance, so a
    /// single OnceCell is sufficient.
    discovered: OnceCell<Vec<DiscoveredPool>>,
}

impl CurveBackend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self {
            ctx,
            discovered: OnceCell::new(),
        }
    }

    fn pools_for(chain: Chain) -> &'static [PoolSpec] {
        match chain {
            Chain::Arbitrum => ARBITRUM_POOLS,
            Chain::Base => BASE_POOLS,
            Chain::Ethereum => ETHEREUM_POOLS,
            Chain::Solana => &[],
        }
    }

    /// Curve has deployments on every EVM chain dexquote covers, but
    /// not on Solana. v1.0 filters Curve out of Solana quote requests
    /// via this chain check instead of falling through to a failing
    /// API call.
    pub fn supports(chain: Chain) -> bool {
        !matches!(chain, Chain::Solana)
    }
}

/// Curve API base path per chain. The API is at `api.curve.finance/v1/`
/// and exposes per-registry pool lists. We aggregate across the
/// registries that contain stable-ng and crypto-ng pools, which together
/// account for 99% of live Curve liquidity.
fn curve_api_chain(chain: Chain) -> &'static str {
    match chain {
        Chain::Arbitrum => "arbitrum",
        Chain::Base => "base",
        Chain::Ethereum => "ethereum",
        Chain::Solana => "", // Curve has no Solana deployment; backend filters out
    }
}

/// Registries to fetch from the Curve API. Each lives at
/// `https://api.curve.finance/v1/getPools/{chain}/{registry}`. The order
/// is intentional: stable pools first (most common), then tricrypto.
const CURVE_REGISTRIES: &[&str] =
    &["factory-stable-ng", "factory-tricrypto", "main", "crypto"];

#[derive(Deserialize)]
struct CurveApiResponse {
    data: Option<CurveApiData>,
}

#[derive(Deserialize)]
struct CurveApiData {
    #[serde(rename = "poolData")]
    pool_data: Vec<CurveApiPool>,
}

#[derive(Deserialize)]
struct CurveApiPool {
    address: String,
    coins: Vec<CurveApiCoin>,
    #[serde(rename = "assetTypeName", default)]
    asset_type_name: Option<String>,
    #[serde(rename = "usdTotal", default)]
    usd_total: f64,
}

#[derive(Deserialize)]
struct CurveApiCoin {
    address: String,
}

/// Fetch every pool in every relevant registry for `chain` via the Curve
/// API. Returns an empty vec on any error — callers fall back to the
/// static pool table. Shapes the API response into `DiscoveredPool`
/// entries by inferring the ABI flavour from the registry slug:
/// `factory-stable-ng` and `main` are int128-indexed Stable pools;
/// `factory-tricrypto` and `crypto` are uint256-indexed Crypto pools.
async fn fetch_curve_pools(chain: Chain) -> Vec<DiscoveredPool> {
    let chain_slug = curve_api_chain(chain);
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(8))
        .connect_timeout(Duration::from_secs(2))
        .user_agent(concat!("dexquote/", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<DiscoveredPool> = Vec::new();
    for registry in CURVE_REGISTRIES {
        let url = format!(
            "https://api.curve.finance/v1/getPools/{}/{}",
            chain_slug, registry
        );
        let kind = match *registry {
            "factory-tricrypto" | "crypto" => PoolKind::Crypto,
            _ => PoolKind::Stable,
        };
        let Ok(resp) = client.get(&url).send().await else {
            continue;
        };
        let Ok(parsed) = resp.json::<CurveApiResponse>().await else {
            continue;
        };
        let Some(data) = parsed.data else { continue };
        for pool in data.pool_data {
            let Ok(addr) = Address::from_str(&pool.address) else {
                continue;
            };
            let coins: Vec<Address> = pool
                .coins
                .iter()
                .filter_map(|c| Address::from_str(&c.address).ok())
                .collect();
            // Skip degenerate / corrupt entries — at least 2 coins needed.
            if coins.len() < 2 {
                continue;
            }
            // Suppress unused-warning for asset_type_name; reserved for
            // smarter ABI inference in v0.4 when stable-ng vs old stable
            // matters more.
            let _ = &pool.asset_type_name;
            out.push(DiscoveredPool {
                address: addr,
                coins,
                kind,
                usd_total: pool.usd_total,
            });
        }
    }
    out
}

#[async_trait::async_trait]
impl DexBackend for CurveBackend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        // Fast path: hand-verified static table. Sub-millisecond.
        // Returns every static pool that contains both tokens — typically
        // 0 or 1 entries, but if multiple pools match we race them too.
        let mut candidates: Vec<Candidate> = find_pool_static_all(
            Self::pools_for(request.chain),
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
        );

        // Slow path: dynamic discovery. Fires only when the static set is
        // empty. The Curve API-fetched pool list is cached in the
        // OnceCell; subsequent quotes skip the HTTP round-trip.
        if candidates.is_empty() {
            let chain = request.chain;
            let pools = self
                .discovered
                .get_or_init(|| async move { fetch_curve_pools(chain).await })
                .await;
            candidates = find_pool_dynamic_all(
                pools,
                request.token_in.evm_address(BACKEND_NAME)?,
                request.token_out.evm_address(BACKEND_NAME)?,
            );
        }

        if candidates.is_empty() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Fan out get_dy calls across every candidate. Each candidate
        // dispatches the right sol! binding based on its PoolKind. The
        // provider is cloned per-closure (cheap — it's an Arc). Failed
        // calls (dead pools, reverts) silently map to None and are
        // filtered out at the fold step.
        let provider = self.ctx.provider.clone();
        let amount_in = request.amount_in;
        let block_id = request.block_id;
        let calls = candidates.into_iter().map(|cand| {
            let provider = provider.clone();
            async move {
                match cand.kind {
                    PoolKind::Crypto => {
                        let contract = ICurveCrypto::new(cand.pool, provider);
                        let call = contract.get_dy(
                            U256::from(cand.i),
                            U256::from(cand.j),
                            amount_in,
                        );
                        let call = match block_id {
                            Some(id) => call.block(id),
                            None => call,
                        };
                        call.call().await.ok()
                    }
                    PoolKind::Stable => {
                        let contract = ICurveStable::new(cand.pool, provider);
                        let call = contract.get_dy(
                            cand.i as i128,
                            cand.j as i128,
                            amount_in,
                        );
                        let call = match block_id {
                            Some(id) => call.block(id),
                            None => call,
                        };
                        call.call().await.ok()
                    }
                }
            }
        });

        // Race them all, filter out failures and zeros, take the max.
        // This is how we avoid picking a dead pool when a live pool
        // exists for the same pair — exact same pattern as UniswapV3's
        // fee-tier race in uniswap_v3.rs.
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
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_CURVE));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_CURVE),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}

/// One pool candidate for the `get_dy` fan-out. Owns just the minimum
/// data needed to dispatch a single `get_dy` call: the pool contract
/// address, which `sol!` ABI variant to use, and the (i, j) coin indices.
/// The coin slice / vec isn't kept because `get_dy` only takes indices.
#[derive(Debug, Clone, Copy)]
struct Candidate {
    pool: Address,
    kind: PoolKind,
    i: usize,
    j: usize,
}

/// Static-table lookup. Returns every pool in the static table that
/// contains both tokens as distinct coins. Multiple matches are possible
/// when a chain has several hand-verified pools over the same pair; the
/// caller races them.
fn find_pool_static_all(
    pools: &'static [PoolSpec],
    token_in: Address,
    token_out: Address,
) -> Vec<Candidate> {
    pools
        .iter()
        .filter_map(|pool| {
            let i = pool.coins.iter().position(|&c| c == token_in)?;
            let j = pool.coins.iter().position(|&c| c == token_out)?;
            (i != j).then_some(Candidate {
                pool: pool.address,
                kind: pool.kind,
                i,
                j,
            })
        })
        .collect()
}

/// Dynamic-discovery lookup. Returns up to `MAX_DYNAMIC_CANDIDATES`
/// matching pools sorted by TVL (highest first). This is the v0.4 fix
/// for the dead-USDC/USDbC-pool bug combined with the v0.5 fix for
/// mainnet's ~1000-pool API response: we pick the top-N by TVL and race
/// them in parallel, same pattern as UniswapV3's fee-tier race. Tail
/// pools (low-TVL / dead / dust) are deliberately dropped because they
/// can't plausibly win the race against their larger siblings anyway.
fn find_pool_dynamic_all(
    pools: &[DiscoveredPool],
    token_in: Address,
    token_out: Address,
) -> Vec<Candidate> {
    let mut matches: Vec<(Candidate, f64)> = pools
        .iter()
        .filter_map(|pool| {
            let i = pool.coins.iter().position(|&c| c == token_in)?;
            let j = pool.coins.iter().position(|&c| c == token_out)?;
            (i != j).then_some((
                Candidate {
                    pool: pool.address,
                    kind: pool.kind,
                    i,
                    j,
                },
                pool.usd_total,
            ))
        })
        .collect();

    // Sort by TVL descending. partial_cmp handles NaN (shouldn't happen
    // but safe) by treating them as equal — they'll sort to arbitrary
    // positions and the cap filters them out anyway.
    matches.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    matches.truncate(MAX_DYNAMIC_CANDIDATES);
    matches.into_iter().map(|(c, _)| c).collect()
}
