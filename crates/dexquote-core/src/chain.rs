use crate::error::DexQuoteError;

/// An EVM chain dexquote knows how to quote on.
///
/// The enum is intentionally small — each variant implies a hardcoded
/// token registry, a default public RPC, a Chainlink ETH/USD feed address,
/// and per-backend contract addresses. Adding a chain = adding a variant
/// here plus a new entry in every lookup table downstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Chain {
    Arbitrum,
    Base,
    Ethereum,
    Solana,
}

impl Chain {
    /// Every chain dexquote currently supports. Used by the doctor
    /// subcommand and anywhere we want to iterate.
    pub const ALL: &'static [Chain] =
        &[Chain::Arbitrum, Chain::Base, Chain::Ethereum, Chain::Solana];

    pub fn id(&self) -> u64 {
        match self {
            Self::Arbitrum => 42161,
            Self::Base => 8453,
            Self::Ethereum => 1,
            // Solana has no EVM chain-id convention. We use 101 which
            // matches Solana's internal mainnet-beta slot cluster ID —
            // it's distinct from any EVM chain id in practice and lets
            // us serialize `Chain` as a u64 without a special case.
            Self::Solana => 101,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Arbitrum => "Arbitrum",
            Self::Base => "Base",
            Self::Ethereum => "Ethereum",
            Self::Solana => "Solana",
        }
    }

    /// The URL slug used by aggregator APIs that expose chain-per-path
    /// endpoints (KyberSwap, OpenOcean). Not every aggregator uses the
    /// same slug — if an aggregator uses a different convention it
    /// overrides this locally. Solana aggregators don't use this slug
    /// pattern at all (each has its own URL shape), so the value here
    /// is a placeholder.
    pub fn url_slug(&self) -> &'static str {
        match self {
            Self::Arbitrum => "arbitrum",
            Self::Base => "base",
            Self::Ethereum => "ethereum",
            Self::Solana => "solana",
        }
    }

    /// A publicly accessible, rate-limited RPC endpoint. Baked into the
    /// first-run wizard so `cargo install dexquote && dexquote WETH USDC 1`
    /// works with zero setup. Users should swap for their own RPC once
    /// they hit the rate limit.
    ///
    /// For Solana, returns the public mainnet-beta endpoint but it's
    /// unused in v1.0 — every Solana backend is an HTTP aggregator and
    /// doesn't need a Solana RPC. The value is a placeholder for v1.1+
    /// when direct on-chain Solana DEX integrations land.
    pub fn default_public_rpc(&self) -> &'static str {
        match self {
            Self::Arbitrum => "https://arb1.arbitrum.io/rpc",
            Self::Base => "https://mainnet.base.org",
            // PublicNode serves mainnet without a Cloudflare anti-bot
            // challenge, unlike llamarpc which otherwise would have been
            // the default. Still rate-limited — users should swap for
            // their own RPC for any serious use.
            Self::Ethereum => "https://ethereum.publicnode.com",
            Self::Solana => "https://api.mainnet-beta.solana.com",
        }
    }

    pub fn parse(input: &str) -> Result<Self, DexQuoteError> {
        match input.to_ascii_lowercase().as_str() {
            "arbitrum" | "arb" | "42161" => Ok(Self::Arbitrum),
            "base" | "8453" => Ok(Self::Base),
            "ethereum" | "eth" | "mainnet" | "1" => Ok(Self::Ethereum),
            "solana" | "sol" | "101" => Ok(Self::Solana),
            other => Err(DexQuoteError::UnsupportedChain(other.to_string())),
        }
    }
}
