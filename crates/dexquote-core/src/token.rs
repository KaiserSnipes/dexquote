use crate::chain::Chain;
use crate::error::DexQuoteError;
use alloy::primitives::{address, Address, U256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::sol;
use std::str::FromStr;

sol! {
    #[sol(rpc)]
    interface IERC20Metadata {
        function decimals() external view returns (uint8);
        function symbol() external view returns (string);
    }
}

/// A token identifier that can hold either a 20-byte EVM address or a
/// 32-byte Solana mint pubkey. Unified across chain families so a single
/// `Token` struct works on both sides — EVM backends extract via
/// `.as_evm()`, Solana backends via `.as_solana()`. Because
/// `BackendKind::supports_chain()` filters backends per-chain at the
/// `build_backends` layer, the extract path is guaranteed safe at
/// runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(untagged)]
pub enum TokenAddress {
    Evm(Address),
    Solana([u8; 32]),
}

impl TokenAddress {
    pub fn as_evm(&self) -> Option<Address> {
        match self {
            Self::Evm(a) => Some(*a),
            Self::Solana(_) => None,
        }
    }

    pub fn as_solana(&self) -> Option<[u8; 32]> {
        match self {
            Self::Solana(b) => Some(*b),
            Self::Evm(_) => None,
        }
    }

    /// Full hex display for EVM (0x…), full base58 display for Solana.
    /// Used by the render layer's token-list output and by the history
    /// key serializer — both want a stable, round-trippable string form.
    pub fn display_string(&self) -> String {
        match self {
            Self::Evm(a) => format!("{:?}", a),
            Self::Solana(b) => bs58::encode(b).into_string(),
        }
    }

    /// Short truncated form for error messages and inline display.
    /// Produces `0xABCD…1234` for EVM, `abCd…1234` for Solana.
    pub fn short(&self) -> String {
        let full = self.display_string();
        if full.len() <= 12 {
            return full;
        }
        let (head, tail) = (&full[..6], &full[full.len() - 4..]);
        format!("{}…{}", head, tail)
    }
}

impl From<Address> for TokenAddress {
    fn from(a: Address) -> Self {
        Self::Evm(a)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Token {
    pub symbol: String,
    pub name: String,
    pub address: TokenAddress,
    pub decimals: u8,
    pub chain: Chain,
}

/// EVM registry entry. Unchanged from v0.8 so the per-chain `const`
/// initializers in `tokens_arbitrum.rs` / `tokens_base.rs` /
/// `tokens_ethereum.rs` don't need to wrap every `address!(...)` call
/// in `TokenAddress::Evm(...)`. The wrap happens at `Token` construction
/// time in `list_tokens` / `lookup_symbol_evm`.
#[derive(Debug, Clone, Copy)]
pub struct RegistryEntry {
    pub symbol: &'static str,
    pub name: &'static str,
    pub address: Address,
    pub decimals: u8,
}

/// Solana registry entry. Mint is the decoded 32-byte pubkey, not the
/// base58 string, so lookups are pure byte comparisons at runtime. The
/// `const fn sol_mint()` helper in `tokens_solana.rs` decodes the base58
/// literal at the token-file source site.
#[derive(Debug, Clone, Copy)]
pub struct SolanaRegistryEntry {
    pub symbol: &'static str,
    pub name: &'static str,
    pub mint: [u8; 32],
    pub decimals: u8,
}

// `Entry` is the internal alias kept for the `const` initializer syntax
// in the EVM token files. `SolEntry` serves the same role for Solana.
type Entry = RegistryEntry;
type SolEntry = SolanaRegistryEntry;

/// Free-function alias for [`Token::suggest_symbols`] so callers can pull
/// it in via `use dexquote_core::suggest_symbols`.
pub fn suggest_symbols(input: &str, chain: Chain, limit: usize) -> Vec<String> {
    Token::suggest_symbols(input, chain, limit)
}

/// Iterate over every bundled token for a chain. Used by the CLI's
/// `--list-tokens` and the interactive picker. Collects into a Vec
/// rather than returning `impl Iterator` because the EVM and Solana
/// registries have different underlying entry types — the match arms
/// can't unify as a single iterator type.
pub fn list_tokens(chain: Chain) -> Vec<Token> {
    match chain {
        Chain::Arbitrum | Chain::Base | Chain::Ethereum => entries_for_evm(chain)
            .iter()
            .map(|e| Token {
                symbol: e.symbol.to_string(),
                name: e.name.to_string(),
                address: TokenAddress::Evm(e.address),
                decimals: e.decimals,
                chain,
            })
            .collect(),
        Chain::Solana => SOLANA_TOKENS
            .iter()
            .map(|e| Token {
                symbol: e.symbol.to_string(),
                name: e.name.to_string(),
                address: TokenAddress::Solana(e.mint),
                decimals: e.decimals,
                chain,
            })
            .collect(),
    }
}

// Per-chain registries live in their own files and are `include!`-d so
// the big address tables don't clutter `token.rs`. Each file defines a
// `const <CHAIN>_TOKENS: &[Entry]` or `const SOLANA_TOKENS: &[SolEntry]`
// array that the per-chain helpers dispatch on.
include!("tokens_arbitrum.rs");
include!("tokens_base.rs");
include!("tokens_ethereum.rs");
include!("tokens_solana.rs");

impl Token {
    /// Extract the EVM-variant address, or return a `NoRoute` error
    /// tagged with `backend`. Used by every on-chain + HTTP-aggregator
    /// EVM backend as a one-liner replacement for the former
    /// `request.token_in.address` access. Safe in practice because
    /// `BackendKind::supports_chain()` filters Solana requests out of
    /// EVM backends at `build_backends` time, but the type system
    /// requires the extraction.
    pub fn evm_address(
        &self,
        backend: &'static str,
    ) -> Result<Address, DexQuoteError> {
        self.address
            .as_evm()
            .ok_or(DexQuoteError::NoRoute { backend })
    }

    pub fn weth(chain: Chain) -> Self {
        // Safe: WETH is present in every bundled EVM registry; if this
        // were ever removed the crate wouldn't compile its integration
        // tests. For Solana, "WETH" resolves to the Wormhole-bridged
        // Ether token in the Solana registry — there's no native WETH
        // on Solana, so the registry provides the canonical mapping.
        Self::lookup_symbol("WETH", chain).unwrap_or(Self {
            symbol: "WETH".into(),
            name: "Wrapped Ether".into(),
            address: TokenAddress::Evm(Address::ZERO),
            decimals: 18,
            chain,
        })
    }

    /// Resolve a user-supplied token input (symbol or raw address) against
    /// the static registry for `chain`. Returns `Ok(Some(token))` on a
    /// hit, `Ok(None)` when the input is a valid raw address that is not
    /// in the registry (caller should follow up with [`Self::fetch_from_chain`]),
    /// and `Err` only for genuinely invalid input.
    ///
    /// For EVM chains, raw addresses are parsed as `0x…` hex. For Solana,
    /// raw addresses are base58-decoded to a 32-byte mint pubkey.
    pub fn resolve_static(
        input: &str,
        chain: Chain,
    ) -> Result<Option<Self>, DexQuoteError> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(DexQuoteError::InvalidTokenInput(
                input.to_string(),
                "empty".into(),
            ));
        }

        if let Some(token) = Self::lookup_symbol(trimmed, chain) {
            return Ok(Some(token));
        }

        match chain {
            Chain::Arbitrum | Chain::Base | Chain::Ethereum => {
                if trimmed.starts_with("0x") || trimmed.starts_with("0X") {
                    let address = Address::from_str(trimmed).map_err(|e| {
                        DexQuoteError::InvalidTokenInput(
                            input.to_string(),
                            e.to_string(),
                        )
                    })?;
                    return Ok(Self::lookup_address_evm(&address, chain));
                }
            }
            Chain::Solana => {
                // Solana pubkeys are 32-44 base58 chars. Try to decode;
                // if it's a valid 32-byte key, look it up.
                if let Ok(bytes) = bs58::decode(trimmed).into_vec() {
                    if bytes.len() == 32 {
                        let mut mint = [0u8; 32];
                        mint.copy_from_slice(&bytes);
                        return Ok(Self::lookup_address_solana(&mint));
                    }
                }
            }
        }

        Err(DexQuoteError::UnknownSymbol(
            trimmed.to_string(),
            chain.name(),
        ))
    }

    /// Resolve a user-supplied token input. Tries the static registry
    /// first; on EVM chains, falls back to fetching `decimals()` and
    /// `symbol()` via RPC when the input is a valid raw address that
    /// isn't in the registry. On Solana, unknown mints currently return
    /// an error pointing users to add the token to the static registry
    /// — v1.0 intentionally skips on-chain metadata fetch for SPL tokens.
    pub async fn resolve(
        input: &str,
        chain: Chain,
        rpc_url: Option<&str>,
    ) -> Result<Self, DexQuoteError> {
        if let Some(token) = Self::resolve_static(input, chain)? {
            return Ok(token);
        }

        // Solana unknown-mint fallback: no on-chain metadata fetch in v1.0.
        if chain == Chain::Solana {
            return Err(DexQuoteError::InvalidTokenInput(
                input.to_string(),
                "unknown Solana mint. dexquote v1.0 requires SPL tokens \
                 to be in the bundled registry — add an entry to \
                 tokens_solana.rs or file an issue"
                    .into(),
            ));
        }

        let trimmed = input.trim();
        let address = Address::from_str(trimmed).map_err(|e| {
            DexQuoteError::InvalidTokenInput(input.to_string(), e.to_string())
        })?;

        let rpc = rpc_url.ok_or_else(|| {
            DexQuoteError::InvalidTokenInput(
                input.to_string(),
                "unknown token address; an RPC (--rpc or DEXQUOTE_RPC) is required \
                 to fetch decimals() and symbol() from the contract"
                    .into(),
            )
        })?;

        Self::fetch_from_chain(address, chain, rpc).await
    }

    /// Fetch ERC20 metadata (`decimals`, `symbol`) for an arbitrary
    /// EVM address via `eth_call`. EVM-only — Solana tokens must be
    /// in the bundled registry.
    pub async fn fetch_from_chain(
        address: Address,
        chain: Chain,
        rpc_url: &str,
    ) -> Result<Self, DexQuoteError> {
        if chain == Chain::Solana {
            return Err(DexQuoteError::InvalidTokenInput(
                format!("{:?}", address),
                "fetch_from_chain is EVM-only; use resolve_static for Solana tokens".into(),
            ));
        }

        let provider = ProviderBuilder::new()
            .connect(rpc_url)
            .await
            .map_err(|e| DexQuoteError::rpc("token lookup", e))?
            .erased();

        let erc20 = IERC20Metadata::new(address, provider);

        let decimals = erc20
            .decimals()
            .call()
            .await
            .map_err(|e| DexQuoteError::rpc("token lookup", e))?;

        let symbol = erc20
            .symbol()
            .call()
            .await
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| short_addr(&address));

        Ok(Self {
            name: symbol.clone(),
            symbol,
            address: TokenAddress::Evm(address),
            decimals,
            chain,
        })
    }

    fn lookup_symbol(symbol: &str, chain: Chain) -> Option<Self> {
        let resolved = resolve_alias(symbol);
        match chain {
            Chain::Arbitrum | Chain::Base | Chain::Ethereum => {
                entries_for_evm(chain)
                    .iter()
                    .find(|e| e.symbol.eq_ignore_ascii_case(resolved))
                    .map(|e| Self {
                        symbol: e.symbol.to_string(),
                        name: e.name.to_string(),
                        address: TokenAddress::Evm(e.address),
                        decimals: e.decimals,
                        chain,
                    })
            }
            Chain::Solana => SOLANA_TOKENS
                .iter()
                .find(|e| e.symbol.eq_ignore_ascii_case(resolved))
                .map(|e| Self {
                    symbol: e.symbol.to_string(),
                    name: e.name.to_string(),
                    address: TokenAddress::Solana(e.mint),
                    decimals: e.decimals,
                    chain,
                }),
        }
    }

    /// Return up to `limit` registry symbols that most closely match the
    /// given input. Used to power "did you mean `WETH`?" suggestions when
    /// an unknown symbol is passed. Empty if the match quality is too low
    /// to be useful.
    ///
    /// Free function [`suggest_symbols`] is an alias for this method that
    /// can be called without the `Token::` qualifier.
    pub fn suggest_symbols(input: &str, chain: Chain, limit: usize) -> Vec<String> {
        let needle = input.to_ascii_lowercase();

        // Collect (score, symbol) pairs from whichever registry applies
        // to this chain. We use two branches instead of a unified
        // iterator because EVM Entry and SolEntry are distinct types.
        let mut scored: Vec<(usize, &'static str)> = match chain {
            Chain::Arbitrum | Chain::Base | Chain::Ethereum => entries_for_evm(chain)
                .iter()
                .map(|e| {
                    let sym = e.symbol.to_ascii_lowercase();
                    let score = score_match(&needle, &sym);
                    (score, e.symbol)
                })
                .filter(|(s, _)| *s > 0)
                .collect(),
            Chain::Solana => SOLANA_TOKENS
                .iter()
                .map(|e| {
                    let sym = e.symbol.to_ascii_lowercase();
                    let score = score_match(&needle, &sym);
                    (score, e.symbol)
                })
                .filter(|(s, _)| *s > 0)
                .collect(),
        };

        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored
            .into_iter()
            .take(limit)
            .map(|(_, s)| s.to_string())
            .collect()
    }

    fn lookup_address_evm(address: &Address, chain: Chain) -> Option<Self> {
        entries_for_evm(chain)
            .iter()
            .find(|e| e.address == *address)
            .map(|e| Self {
                symbol: e.symbol.to_string(),
                name: e.name.to_string(),
                address: TokenAddress::Evm(e.address),
                decimals: e.decimals,
                chain,
            })
    }

    fn lookup_address_solana(mint: &[u8; 32]) -> Option<Self> {
        SOLANA_TOKENS.iter().find(|e| &e.mint == mint).map(|e| Self {
            symbol: e.symbol.to_string(),
            name: e.name.to_string(),
            address: TokenAddress::Solana(e.mint),
            decimals: e.decimals,
            chain: Chain::Solana,
        })
    }
}

/// EVM-only registry dispatch. Panics on Solana — callers must branch
/// on chain before calling this.
fn entries_for_evm(chain: Chain) -> &'static [Entry] {
    match chain {
        Chain::Arbitrum => ARBITRUM_TOKENS,
        Chain::Base => BASE_TOKENS,
        Chain::Ethereum => ETHEREUM_TOKENS,
        Chain::Solana => &[],
    }
}

/// Map a handful of common aliases to their canonical registry symbols.
/// Everyone says "ETH" when they mean WETH on an EVM chain. Same for BTC
/// and a handful of lowercase-convention LSTs. Returns the input unchanged
/// when no alias applies, so callers can unconditionally pipe through.
fn resolve_alias(input: &str) -> &str {
    match input.to_ascii_lowercase().as_str() {
        "eth" | "ether" => "WETH",
        "btc" | "bitcoin" => "WBTC",
        "steth" => "wstETH",
        "usd" => "USDC",
        _ => input,
    }
}

/// Lightweight similarity heuristic for symbol suggestions. Higher is
/// better. Returns 0 when the candidate is almost certainly not what the
/// user meant.
///
///   - exact (case-insensitive) match             → 1000
///   - candidate starts with input                → 500 + shared length
///   - input starts with candidate                → 400 + shared length
///   - shared-character count (unordered)         → raw overlap
fn score_match(needle: &str, candidate: &str) -> usize {
    if needle == candidate {
        return 1000;
    }
    if candidate.starts_with(needle) {
        return 500 + needle.len();
    }
    if needle.starts_with(candidate) {
        return 400 + candidate.len();
    }
    // Character overlap — cheap "edit distance proxy".
    let mut overlap = 0usize;
    for c in needle.chars() {
        if candidate.contains(c) {
            overlap += 1;
        }
    }
    // Need at least half the characters to match to count as a suggestion.
    if overlap * 2 >= needle.len().max(2) {
        overlap
    } else {
        0
    }
}

fn short_addr(address: &Address) -> String {
    let hex = format!("{:?}", address);
    format!("{}…{}", &hex[..6], &hex[hex.len() - 4..])
}

/// Parse a human-readable amount like `"1.5"` (or `"1.5k"`, `"1e6"`, `"10m"`)
/// into base units using `decimals`. Suffix multipliers supported:
///
///   - `k` / `K` → ×10³
///   - `m` / `M` → ×10⁶
///   - `b` / `B` → ×10⁹
///   - `eN`      → ×10ᴺ  (scientific notation)
///
/// The suffix is expanded to plain decimal via string manipulation — no
/// f64 involved, so precision is preserved for any amount that fits in a
/// `U256`.
pub fn parse_amount(input: &str, decimals: u8) -> Result<U256, DexQuoteError> {
    let normalized = normalize_amount_input(input)?;
    parse_plain_amount(&normalized, input, decimals)
}

/// Expand suffixes and scientific notation into a plain decimal string.
/// Returns the normalized string on success or an error with the original
/// input preserved.
fn normalize_amount_input(input: &str) -> Result<String, DexQuoteError> {
    let trimmed = input.trim().replace('_', "");
    if trimmed.is_empty() {
        return Err(DexQuoteError::InvalidAmount(
            input.to_string(),
            "empty".into(),
        ));
    }

    // Scientific notation: "1e6", "1.5e3". Split on 'e'/'E', parse the
    // exponent as u32, shift the decimal point.
    if let Some((mantissa, exp_part)) = trimmed.split_once(['e', 'E']) {
        let exp: u32 = exp_part.parse().map_err(|_| {
            DexQuoteError::InvalidAmount(
                input.to_string(),
                format!("invalid scientific exponent `{exp_part}`"),
            )
        })?;
        return Ok(shift_decimal_right(mantissa, exp));
    }

    // Suffix multiplier: last character is k/m/b and the rest is a
    // plain decimal.
    if let Some(last) = trimmed.chars().last() {
        let exp: Option<u32> = match last {
            'k' | 'K' => Some(3),
            'm' | 'M' => Some(6),
            'b' | 'B' => Some(9),
            _ => None,
        };
        if let Some(exp) = exp {
            let mantissa = &trimmed[..trimmed.len() - 1];
            return Ok(shift_decimal_right(mantissa, exp));
        }
    }

    Ok(trimmed)
}

/// Shift a plain decimal number right by `exp` digits (i.e. multiply by
/// 10^exp) using string manipulation only. `"1.5"`, exp=3 → `"1500"`.
fn shift_decimal_right(mantissa: &str, exp: u32) -> String {
    let (int_part, frac_part) = match mantissa.split_once('.') {
        Some((i, f)) => (i, f),
        None => (mantissa, ""),
    };
    let int_part = if int_part.is_empty() { "0" } else { int_part };

    let exp = exp as usize;
    if frac_part.len() <= exp {
        // Whole shift falls within or past the fractional part: all of
        // frac becomes int, remaining zeros are padded on the right.
        let padding = exp - frac_part.len();
        format!("{int_part}{frac_part}{}", "0".repeat(padding))
    } else {
        // Partial shift: only the first `exp` fractional digits move to
        // the integer side; the rest remain fractional.
        let (move_to_int, stay_frac) = frac_part.split_at(exp);
        format!("{int_part}{move_to_int}.{stay_frac}")
    }
}

fn parse_plain_amount(trimmed: &str, original: &str, decimals: u8) -> Result<U256, DexQuoteError> {
    if trimmed.is_empty() {
        return Err(DexQuoteError::InvalidAmount(
            original.to_string(),
            "empty".into(),
        ));
    }
    if trimmed.chars().any(|c| !(c.is_ascii_digit() || c == '.')) {
        return Err(DexQuoteError::InvalidAmount(
            original.to_string(),
            "only digits, one '.', one suffix (k/m/b), or scientific notation allowed"
                .into(),
        ));
    }
    let input = original;

    let (int_part, frac_part) = match trimmed.split_once('.') {
        Some((i, f)) => {
            if f.contains('.') {
                return Err(DexQuoteError::InvalidAmount(
                    input.to_string(),
                    "multiple decimal points".into(),
                ));
            }
            (i, f)
        }
        None => (trimmed, ""),
    };

    if frac_part.len() > decimals as usize {
        return Err(DexQuoteError::InvalidAmount(
            input.to_string(),
            format!("too many decimals (max {})", decimals),
        ));
    }

    let int_part = if int_part.is_empty() { "0" } else { int_part };
    let padded_frac = format!("{:0<width$}", frac_part, width = decimals as usize);
    let combined = format!("{int_part}{padded_frac}");
    U256::from_str_radix(&combined, 10)
        .map_err(|e| DexQuoteError::InvalidAmount(input.to_string(), e.to_string()))
}

/// Format a base-unit amount as a human-readable string with comma grouping
/// in the integer part and up to `max_frac_digits` after the decimal point
/// (trailing zeros in the fraction are stripped).
pub fn format_amount(amount: U256, decimals: u8, max_frac_digits: u8) -> String {
    let amount_str = amount.to_string();
    let decimals = decimals as usize;
    let max_frac_digits = max_frac_digits as usize;

    let (int_part, frac_part) = if amount_str.len() <= decimals {
        let padded = format!("{:0>width$}", amount_str, width = decimals);
        ("0".to_string(), padded)
    } else {
        let split = amount_str.len() - decimals;
        (
            amount_str[..split].to_string(),
            amount_str[split..].to_string(),
        )
    };

    let int_grouped = group_int(&int_part);

    let frac_trimmed = {
        let take = frac_part.len().min(max_frac_digits);
        let slice = &frac_part[..take];
        slice.trim_end_matches('0').to_string()
    };

    if frac_trimmed.is_empty() {
        int_grouped
    } else {
        format!("{int_grouped}.{frac_trimmed}")
    }
}

fn group_int(s: &str) -> String {
    let bytes = s.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i != 0 && (len - i) % 3 == 0 {
            out.push(',');
        }
        out.push(b as char);
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn resolves_weth_symbol() {
        let t = Token::resolve_static("weth", Chain::Arbitrum)
            .unwrap()
            .unwrap();
        assert_eq!(t.symbol, "WETH");
        assert_eq!(t.decimals, 18);
    }

    #[test]
    fn resolves_usdc_is_native_circle() {
        let t = Token::resolve_static("USDC", Chain::Arbitrum)
            .unwrap()
            .unwrap();
        assert_eq!(
            t.address,
            TokenAddress::Evm(address!("af88d065e77c8cC2239327C5EDb3A432268e5831"))
        );
        assert_eq!(t.decimals, 6);
    }

    #[test]
    fn resolves_usdc_e_is_bridged() {
        let t = Token::resolve_static("USDC.e", Chain::Arbitrum)
            .unwrap()
            .unwrap();
        assert_eq!(
            t.address,
            TokenAddress::Evm(address!("FF970A61A04b1cA14834A43f5dE4533eBDDB5CC8"))
        );
    }

    #[test]
    fn resolves_raw_known_address() {
        let t = Token::resolve_static(
            "0xaf88d065e77c8cC2239327C5EDb3A432268e5831",
            Chain::Arbitrum,
        )
        .unwrap()
        .unwrap();
        assert_eq!(t.symbol, "USDC");
        assert_eq!(t.decimals, 6);
    }

    #[test]
    fn raw_unknown_address_needs_chain_lookup() {
        let result = Token::resolve_static(
            "0x1111111111111111111111111111111111111111",
            Chain::Arbitrum,
        )
        .unwrap();
        assert!(result.is_none(), "should defer to on-chain lookup");
    }

    #[test]
    fn rejects_unknown_symbol() {
        assert!(Token::resolve_static("FOOBAR", Chain::Arbitrum).is_err());
    }

    #[test]
    fn eth_alias_resolves_to_weth() {
        let t = Token::resolve_static("eth", Chain::Arbitrum)
            .unwrap()
            .unwrap();
        assert_eq!(t.symbol, "WETH");
    }

    #[test]
    fn btc_alias_resolves_to_wbtc() {
        let t = Token::resolve_static("btc", Chain::Arbitrum)
            .unwrap()
            .unwrap();
        assert_eq!(t.symbol, "WBTC");
    }

    #[test]
    fn suggest_finds_typos() {
        let suggestions = Token::suggest_symbols("WTH", Chain::Arbitrum, 3);
        assert!(
            suggestions.iter().any(|s| s == "WETH"),
            "WTH should suggest WETH: {suggestions:?}"
        );
    }

    #[test]
    fn suggest_finds_prefix() {
        let suggestions = Token::suggest_symbols("pen", Chain::Arbitrum, 3);
        assert!(
            suggestions.iter().any(|s| s == "PENDLE"),
            "pen should suggest PENDLE: {suggestions:?}"
        );
    }

    #[tokio::test]
    async fn async_resolve_errors_on_unknown_address_without_rpc() {
        let err = Token::resolve(
            "0x1111111111111111111111111111111111111111",
            Chain::Arbitrum,
            None,
        )
        .await
        .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("RPC"), "got: {msg}");
    }

    #[test]
    fn parse_amount_whole() {
        assert_eq!(parse_amount("1", 18).unwrap(), U256::from(10u128.pow(18)));
    }

    #[test]
    fn parse_amount_fractional() {
        let expected = U256::from(1_500_000u128);
        assert_eq!(parse_amount("1.5", 6).unwrap(), expected);
    }

    #[test]
    fn parse_amount_dot_leading() {
        let expected = U256::from(500_000u128);
        assert_eq!(parse_amount(".5", 6).unwrap(), expected);
    }

    #[test]
    fn parse_amount_rejects_too_many_decimals() {
        assert!(parse_amount("1.1234567", 6).is_err());
    }

    #[test]
    fn parse_amount_rejects_garbage() {
        assert!(parse_amount("abc", 6).is_err());
        assert!(parse_amount("1.2.3", 6).is_err());
    }

    #[test]
    fn parse_amount_k_suffix() {
        assert_eq!(
            parse_amount("1k", 6).unwrap(),
            U256::from(1_000_000_000u128)
        );
        assert_eq!(
            parse_amount("1.5k", 6).unwrap(),
            U256::from(1_500_000_000u128)
        );
    }

    #[test]
    fn parse_amount_m_suffix() {
        assert_eq!(
            parse_amount("2m", 18).unwrap(),
            U256::from(2_000_000u128) * U256::from(10u128.pow(18))
        );
    }

    #[test]
    fn parse_amount_scientific() {
        assert_eq!(
            parse_amount("1e6", 6).unwrap(),
            U256::from(1_000_000_000_000u128)
        );
        assert_eq!(
            parse_amount("1.5e3", 6).unwrap(),
            U256::from(1_500_000_000u128)
        );
    }

    #[test]
    fn parse_amount_suffix_preserves_fractional() {
        // "1.5k" → 1500, which at decimals=6 means 1_500_000_000 base units.
        assert_eq!(
            parse_amount("1.5k", 6).unwrap(),
            U256::from(1_500_000_000u128)
        );
    }

    #[test]
    fn format_amount_integer() {
        let amount = U256::from(3_501_230_000u128);
        assert_eq!(format_amount(amount, 6, 2), "3,501.23");
    }

    #[test]
    fn format_amount_strips_trailing_zeros() {
        let amount = U256::from(1_000_000u128);
        assert_eq!(format_amount(amount, 6, 6), "1");
    }

    #[test]
    fn format_amount_small_value() {
        let amount = U256::from(123u128);
        assert_eq!(format_amount(amount, 6, 6), "0.000123");
    }

    #[test]
    fn format_amount_groups_large_ints() {
        let amount = U256::from(1_234_567u128) * U256::from(10u128.pow(6));
        assert_eq!(format_amount(amount, 6, 2), "1,234,567");
    }

    #[test]
    fn round_trip_parse_format() {
        let parsed = parse_amount("12345.678", 18).unwrap();
        assert_eq!(format_amount(parsed, 18, 3), "12,345.678");
    }
}
