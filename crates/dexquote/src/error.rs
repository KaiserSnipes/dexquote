//! User-facing error model.
//!
//! Every CLI error is classified into a category so rendering can show a
//! colored icon + a one-line action hint instead of a stack trace. `main`
//! converts library errors (`DexQuoteError`) into this type near the top of
//! each flow.

use colored::Colorize;
use dexquote_core::{suggest_symbols, Chain, DexQuoteError};
use std::fmt;

pub type CliResult<T> = Result<T, CliError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    /// Missing setup (config, RPC, chain selection).
    Setup,
    /// User input error (bad token, bad amount, typo).
    Input,
    /// Network / RPC failure reaching the outside world.
    Network,
    /// Unexpected internal failure; shown with a bug-report hint.
    Bug,
}

impl Category {
    fn icon(self, colored: bool) -> String {
        let text = "●";
        if !colored {
            return text.to_string();
        }
        match self {
            Category::Setup => text.yellow().bold().to_string(),
            Category::Input => text.cyan().bold().to_string(),
            Category::Network => text.magenta().bold().to_string(),
            Category::Bug => text.red().bold().to_string(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Category::Setup => "setup",
            Category::Input => "input",
            Category::Network => "network",
            Category::Bug => "bug",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CliError {
    pub category: Category,
    pub message: String,
    /// One-line "what do I do next" hint. Always present.
    pub hint: String,
}

impl CliError {
    pub fn setup(message: String, hint: String) -> Self {
        Self {
            category: Category::Setup,
            message,
            hint,
        }
    }

    pub fn input(message: String, hint: String) -> Self {
        Self {
            category: Category::Input,
            message,
            hint,
        }
    }

    pub fn network(message: String, hint: String) -> Self {
        Self {
            category: Category::Network,
            message,
            hint,
        }
    }

    pub fn bug(message: String) -> Self {
        Self {
            category: Category::Bug,
            message,
            hint: "please file an issue at https://github.com/dexquote/dexquote/issues".into(),
        }
    }

    /// Render to stderr with color if `colored` is true.
    pub fn render(&self, colored: bool) -> String {
        let icon = self.category.icon(colored);
        let label = if colored {
            self.category.label().bold().to_string()
        } else {
            self.category.label().to_string()
        };
        let hint_line = if colored {
            format!("  {} {}", "→".dimmed(), self.hint.dimmed())
        } else {
            format!("  → {}", self.hint)
        };
        format!("{icon} {label}: {}\n{hint_line}", self.message)
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.category.label(), self.message)
    }
}

impl std::error::Error for CliError {}

impl From<DexQuoteError> for CliError {
    fn from(err: DexQuoteError) -> Self {
        use DexQuoteError::*;
        match &err {
            UnknownSymbol(sym, chain) => {
                // Try to fuzzy-match against the registry so the hint can
                // say "did you mean WETH?" instead of a generic pointer.
                let suggestions = match Chain::parse(&chain.to_ascii_lowercase()) {
                    Ok(c) => suggest_symbols(sym, c, 3),
                    Err(_) => Vec::new(),
                };
                let hint = if suggestions.is_empty() {
                    "run `dexquote tokens` to see available symbols, or pass a 0x… address"
                        .to_string()
                } else {
                    format!(
                        "did you mean {}? (or run `dexquote tokens`)",
                        suggestions
                            .iter()
                            .map(|s| format!("`{s}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                CliError::input(format!("unknown token symbol `{sym}` on {chain}"), hint)
            }
            InvalidTokenInput(_, msg) if msg.contains("RPC") => CliError::setup(
                err.to_string(),
                "set a default with `dexquote config set defaults.rpc <URL>`, or pass --rpc".to_string(),
            ),
            InvalidTokenInput(input, msg) => CliError::input(
                format!("invalid token `{input}`: {msg}"),
                "use a registered symbol or a checksummed 0x… address".to_string(),
            ),
            InvalidAmount(amt, msg) => CliError::input(
                format!("invalid amount `{amt}`: {msg}"),
                "try a decimal like `1.0` or `0.25`".to_string(),
            ),
            UnsupportedChain(name) => CliError::setup(
                format!("unsupported chain `{name}`"),
                "v0.1 only supports Arbitrum; omit --chain or set it to `arbitrum`".to_string(),
            ),
            RpcRequired(backend) => CliError::setup(
                format!("{backend} requires an RPC endpoint"),
                "set DEXQUOTE_RPC, pass --rpc <URL>, or `dexquote config set defaults.rpc <URL>`".to_string(),
            ),
            Http { backend, source } if source.is_timeout() => CliError::network(
                format!("{backend} timed out"),
                "try --timeout 20000 or check your network connection".to_string(),
            ),
            Http { backend, .. } => CliError::network(
                format!("{backend} HTTP error"),
                "check the backend's status or retry in a moment".to_string(),
            ),
            Rpc { backend, .. } => CliError::network(
                format!("{backend} RPC call failed"),
                "your RPC may be rate-limited or down; try a different --rpc".to_string(),
            ),
            NoRoute { backend } => CliError::input(
                format!("{backend} has no route for this pair"),
                "this is normal — not every DEX has every pair; try a different backend list".to_string(),
            ),
            Timeout { backend, ms } => CliError::network(
                format!("{backend} didn't return within {ms}ms"),
                "try --timeout 20000 or check network latency".to_string(),
            ),
            Decode { backend, message } => CliError::bug(format!("{backend} decode error: {message}")),
        }
    }
}
