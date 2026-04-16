//! Balancer V2 — Vault-level quoting via `queryBatchSwap`.
//!
//! Balancer V2 is a single-vault design: every swap routes through the
//! singleton `Vault` contract at `0xBA12…2C8` (same canonical address on
//! every chain). Pools are identified by `bytes32 poolId` where the first
//! 20 bytes are the pool contract's address and the last 12 encode pool
//! type + nonce.
//!
//! Quoting uses `queryBatchSwap(kind, steps[], assets[], funds)`. The
//! function is **not view** — the Vault reverts with encoded data
//! internally and catches — but it's safe to call via `eth_call` because
//! it doesn't commit any state. Alloy's generated `.call()` handles the
//! static-call semantics transparently.
//!
//! We ship only single-pool (direct-pair) quotes in v0.2. Balancer
//! supports `BatchSwapStep[]` chaining for multi-pool routes but that
//! requires tracking shared-asset indices and is deferred.
//!
//! Pool ID verification trail: every entry in the `ARBITRUM_POOLS` /
//! `BASE_POOLS` tables was confirmed by calling
//! `Vault.getPoolTokens(poolId)` against the real Arbitrum / Base RPC
//! during implementation and cross-checking the returned token set
//! against the `PoolSpec.tokens` declaration.

use crate::backends::{DexBackend, OnChainContext};
use crate::chain::Chain;
use crate::error::DexQuoteError;
use crate::quote::{Quote, QuoteRequest};
use alloy::primitives::{address, b256, Address, Bytes, FixedBytes, I256, U256};
use alloy::sol;
use std::time::Instant;

const BACKEND_NAME: &str = "BalancerV2";

// Canonical Balancer V2 Vault address — identical on every EVM chain
// thanks to CREATE2. Verified 49,026 bytes of contract code on Base and
// 35k+ on Arbitrum.
const VAULT: Address = address!("BA12222222228d8Ba445958a75a0704d566BF2C8");

// Typical gas cost for a single-pool Balancer V2 swap: ~180k for a
// weighted pool, ~220k for a stable pool. Flat estimate is fine for
// the USD-gas display column.
const GAS_ESTIMATE_BAL: u64 = 200_000;

struct PoolSpec {
    /// 32-byte pool ID. First 20 bytes are the pool contract address,
    /// the remaining 12 encode the pool type + nonce.
    id: FixedBytes<32>,
    /// All tokens held by the pool, in the order the Vault reports them
    /// from `getPoolTokens`. The backend finds the matching pair and
    /// builds the `BatchSwapStep` using this slice.
    tokens: &'static [Address],
}

// Arbitrum Balancer V2 pools. Verified live via
// `Vault.getPoolTokens(poolId)` on arb1.arbitrum.io.
const ARBITRUM_POOLS: &[PoolSpec] = &[
    // WBTC / WETH / USDC.e weighted pool. $338k TVL as of verification.
    PoolSpec {
        id: b256!("64541216bafffeec8ea535bb71fbc927831d0595000100000000000000000002"),
        tokens: &[
            address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"), // WBTC
            address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"), // WETH
            address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"), // USDC.e
        ],
    },
    // DAI / USDT / USDC.e composable stable pool. $130k TVL.
    PoolSpec {
        id: b256!("1533a3278f3f9141d5f820a184ea4b017fce2382000000000000000000000016"),
        tokens: &[
            address!("DA10009cBd5D07dd0CeCc66161FC93D7c9000da1"), // DAI
            address!("Fd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9"), // USDT
            address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"), // USDC.e
        ],
    },
    // WBTC / 2BTC / tBTC tri-BTC composable stable pool. $237k TVL.
    // Useful for BTC-variant swaps (WBTC↔tBTC etc) which no other
    // backend in dexquote covers directly.
    PoolSpec {
        id: b256!("542f16da0efb162d20bf4358efa095b70a100f9e000000000000000000000436"),
        tokens: &[
            address!("2f2a2543B76A4166549F7aaB2e75Bef0aefC5B0f"), // WBTC
            address!("542f16da0EfB162D20bF4358efA095B70A100f9E"), // 2BTC (the pool BPT itself in CSP)
            address!("6c84a8f1C29108F47a79964b5Fe888D4f4D0dE40"), // tBTC
        ],
    },
    // RDNT / WETH weighted pool. $257k TVL as of the v0.4 sweep.
    // Pulled from the Balancer V3 API (`protocolVersion: [2]`) and
    // verified live via `Vault.getPoolTokens` — returned `[RDNT, WETH]`
    // in that order. Gives dexquote a Balancer-native RDNT/WETH quote
    // which otherwise only routes through the aggregators.
    PoolSpec {
        id: b256!("32df62dc3aed2cd6224193052ce665dc181658410002000000000000000003bd"),
        tokens: &[
            address!("3082CC23568eA640225c2467653dB90e9250AaA0"), // RDNT
            address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1"), // WETH
        ],
    },
];

/// Ethereum mainnet Balancer V2 pools. Verified live via
/// `Vault.getPoolTokens(poolId)` on `ethereum.publicnode.com` at v0.5
/// implementation time. Balancer V2 on mainnet is considerably smaller
/// than you'd expect — most liquidity has migrated to V3. These are the
/// top V2 pools by TVL that cover canonical pairs.
const ETHEREUM_POOLS: &[PoolSpec] = &[
    // 80BAL/20WETH weighted pool. ~$6M TVL — the canonical Balancer
    // governance pool and the primary BAL/WETH venue on mainnet.
    PoolSpec {
        id: b256!("5c6ee304399dbdb9c8ef030ab642b10820db8f56000200000000000000000014"),
        tokens: &[
            address!("ba100000625a3754423978a60c9317c58a424e3D"), // BAL
            address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH
        ],
    },
    // 50WBTC/50WETH weighted pool. ~$1.75M TVL — Balancer's main
    // WBTC/WETH venue, useful for comparing against UniV3's deep pools.
    PoolSpec {
        id: b256!("a6f548df93de924d73be7d25dc02554c6bd66db500020000000000000000000e"),
        tokens: &[
            address!("2260FAC5E5542a773Aa44fBCfeDf7C193bc2C599"), // WBTC
            address!("C02aaA39b223FE8D0A0e5C4F27eAD9083C756Cc2"), // WETH
        ],
    },
    // GHO/USDT/USDC composable stable pool. ~$120k TVL. The primary
    // venue for GHO (Aave's stablecoin) on-chain and the only backend
    // in dexquote that reaches it without relying on an aggregator.
    PoolSpec {
        id: b256!("8353157092ed8be69a9df8f95af097bbf33cb2af0000000000000000000005d9"),
        tokens: &[
            address!("40D16FC0246aD3160Ccc09B8D0D3A2cd28aE6C2f"), // GHO
            address!("8353157092ED8Be69a9DF8F95af097bbf33Cb2aF"), // BPT itself (CSP convention)
            address!("A0b86991c6218b36c1D19D4a2e9Eb0cE3606eB48"), // USDC
            address!("dAC17F958D2ee523a2206206994597C13D831ec7"), // USDT
        ],
    },
];

// Base Balancer V2 pools. Verified live via
// `Vault.getPoolTokens(poolId)` on mainnet.base.org. TVL is lower than
// Arbitrum because most Balancer liquidity on Base has migrated to v3;
// the v2 pools below are the top 2 by TVL as of verification.
//
// v0.4 sweep result: the Balancer V3 API's top 4 V2 pools on Base are
// all niche (GYD/AUSDC, OLAS/USDC, WETH/IMO, GYFI/GYD). Nothing worth
// adding, so the table is unchanged.
const BASE_POOLS: &[PoolSpec] = &[
    // WETH / USDC weighted pool. $56k TVL.
    PoolSpec {
        id: b256!("10bdbb4fe8dfd348d44397eedabb737df68bc9a0000200000000000000000248"),
        tokens: &[
            address!("4200000000000000000000000000000000000006"), // WETH
            address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
        ],
    },
    // USDC / USDbC stable pool. $33k TVL. Useful for native ↔ bridged
    // USDC swaps which no other Base backend covers cleanly.
    PoolSpec {
        id: b256!("8f360baf899845441eccdc46525e26bb8860752a0002000000000000000001cd"),
        tokens: &[
            address!("833589fCD6eDb6E08f4c7C32D4f71b54bdA02913"), // USDC
            address!("d9aAEc86B65D86f6A7B5B1b0c42FFA531710b6CA"), // USDbC
        ],
    },
];

sol! {
    #[sol(rpc)]
    interface IBalancerVault {
        struct BatchSwapStep {
            bytes32 poolId;
            uint256 assetInIndex;
            uint256 assetOutIndex;
            uint256 amount;
            bytes userData;
        }

        struct FundManagement {
            address sender;
            bool fromInternalBalance;
            address recipient;
            bool toInternalBalance;
        }

        /// `kind` is `SwapKind` — 0 = GIVEN_IN (exact-input), 1 = GIVEN_OUT.
        /// Encoded as a uint8 in the function selector since sol! doesn't
        /// expose Solidity enums directly.
        function queryBatchSwap(
            uint8 kind,
            BatchSwapStep[] memory swaps,
            address[] memory assets,
            FundManagement memory funds
        ) external returns (int256[] memory);
    }
}

pub struct BalancerV2Backend {
    ctx: OnChainContext,
}

impl BalancerV2Backend {
    pub fn new(ctx: OnChainContext) -> Self {
        Self { ctx }
    }

    fn pools_for(chain: Chain) -> &'static [PoolSpec] {
        match chain {
            Chain::Arbitrum => ARBITRUM_POOLS,
            Chain::Base => BASE_POOLS,
            Chain::Ethereum => ETHEREUM_POOLS,
            Chain::Solana => &[],
        }
    }

    /// Backends have this helper so `build_backends` in the binary can
    /// filter the selection to only chains where the backend is usable.
    /// For Balancer V2 that's "any chain with at least one pool
    /// configured", which matches every chain we support.
    pub fn supports(chain: Chain) -> bool {
        !Self::pools_for(chain).is_empty()
    }
}

#[async_trait::async_trait]
impl DexBackend for BalancerV2Backend {
    fn name(&self) -> &'static str {
        BACKEND_NAME
    }

    async fn quote(&self, request: &QuoteRequest) -> Result<Quote, DexQuoteError> {
        let start = Instant::now();

        // Find the first pool that contains both tokens. The static
        // tables are short (≤5 pools per chain) so linear search is fine.
        let pool = find_pool(
            Self::pools_for(request.chain),
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
        )
        .ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        // Assets array for this query — always two entries for a
        // single-pool direct swap. Indices into this array are 0 and 1.
        let assets = vec![
            request.token_in.evm_address(BACKEND_NAME)?,
            request.token_out.evm_address(BACKEND_NAME)?,
        ];

        let step = IBalancerVault::BatchSwapStep {
            poolId: pool.id,
            assetInIndex: U256::from(0u64),
            assetOutIndex: U256::from(1u64),
            amount: request.amount_in,
            userData: Bytes::new(),
        };

        let funds = IBalancerVault::FundManagement {
            sender: Address::ZERO,
            fromInternalBalance: false,
            recipient: Address::ZERO,
            toInternalBalance: false,
        };

        let vault = IBalancerVault::new(VAULT, self.ctx.provider.clone());

        // SwapKind::GIVEN_IN = 0. queryBatchSwap reverts internally and
        // the Vault catches before returning — `eth_call` handles that
        // transparently, so .call().await resolves normally.
        let call = vault.queryBatchSwap(0u8, vec![step], assets, funds);
        let call = match request.block_id {
            Some(id) => call.block(id),
            None => call,
        };
        let deltas = call.call().await.map_err(|_| DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        // `deltas[0]` = signed delta for asset[0] (token_in). Should be
        // positive — user pays in.
        // `deltas[1]` = signed delta for asset[1] (token_out). Should be
        // negative — vault pays out. We negate and convert to U256.
        let out_delta = deltas.get(1).copied().ok_or(DexQuoteError::NoRoute {
            backend: BACKEND_NAME,
        })?;

        if out_delta >= I256::ZERO {
            // Non-negative output means no liquidity came out. Treat as
            // NoRoute rather than a bogus zero-price quote.
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        // Negate the signed int256 to get the absolute amount paid out.
        // `I256::unchecked_neg` would overflow on I256::MIN but that's
        // impossible for any real swap amount.
        let amount_out_i = -out_delta;
        let amount_out: U256 = amount_out_i.into_raw();

        if amount_out.is_zero() {
            return Err(DexQuoteError::NoRoute {
                backend: BACKEND_NAME,
            });
        }

        let gas_usd = self
            .ctx
            .gas_pricer
            .get()
            .await
            .map(|p| p.gas_units_to_usd(GAS_ESTIMATE_BAL));

        Ok(Quote {
            backend: BACKEND_NAME,
            amount_out,
            gas_estimate: Some(GAS_ESTIMATE_BAL),
            gas_usd,
            latency_ms: start.elapsed().as_millis(),
            route: None,
        })
    }
}

fn find_pool(
    pools: &'static [PoolSpec],
    token_in: Address,
    token_out: Address,
) -> Option<&'static PoolSpec> {
    pools
        .iter()
        .find(|p| p.tokens.contains(&token_in) && p.tokens.contains(&token_out))
}
