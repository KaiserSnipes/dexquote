//! Persistent configuration stored as JSON.
//!
//! Default locations:
//!   - Linux/macOS: `$XDG_CONFIG_HOME/dexquote/config.json` (or `~/.config/dexquote/config.json`)
//!   - Windows:     `%APPDATA%\dexquote\config.json`
//!
//! Precedence for every setting: CLI flag > env var > config file > built-in default.

use crate::error::{CliError, CliResult};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Public Arbitrum One RPC. Rate-limited and meant as a convenience default.
/// The first-run wizard tells the user how to override it with their own.
pub const DEFAULT_ARBITRUM_RPC: &str = "https://arb1.arbitrum.io/rpc";
pub const DEFAULT_CHAIN: &str = "arbitrum";
pub const DEFAULT_TIMEOUT_MS: u64 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub defaults: Defaults,
    pub backends: BackendsConfig,
    pub display: DisplayConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Defaults {
    pub chain: String,
    pub rpc: String,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackendsConfig {
    /// Backend identifiers, e.g. ["uniswap-v3", "sushi-v2", "trader-joe", "odos"].
    pub enabled: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayConfig {
    /// "auto" | "always" | "never"
    pub color: String,
    /// "table" | "json" | "minimal"
    pub format: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults {
                chain: DEFAULT_CHAIN.to_string(),
                rpc: DEFAULT_ARBITRUM_RPC.to_string(),
                timeout_ms: DEFAULT_TIMEOUT_MS,
            },
            backends: BackendsConfig {
                enabled: vec![
                    "uniswap-v2".to_string(),
                    "uniswap-v3".to_string(),
                    "uniswap-v4".to_string(),
                    "sushi-v2".to_string(),
                    "fraxswap".to_string(),
                    "trader-joe".to_string(),
                    "pancake-v3".to_string(),
                    "camelot-v3".to_string(),
                    "curve".to_string(),
                    "aerodrome".to_string(),
                    "slipstream".to_string(),
                    "balancer-v2".to_string(),
                    "maverick-v2".to_string(),
                    "dodo-v2".to_string(),
                    "odos".to_string(),
                    "paraswap".to_string(),
                    "kyberswap".to_string(),
                    "openocean".to_string(),
                    "lifi".to_string(),
                    "cowswap".to_string(),
                    "jupiter".to_string(),
                    "jupiter-ultra".to_string(),
                    "raydium".to_string(),
                    "openocean-sol".to_string(),
                    "lifi-sol".to_string(),
                ],
            },
            display: DisplayConfig {
                color: "auto".to_string(),
                format: "table".to_string(),
            },
        }
    }
}

/// Result of loading (or initialising) the config file. When this is the
/// first run — no file existed yet — the caller is responsible for showing
/// the welcome message; see `Config::load_or_init`.
pub struct Loaded {
    pub config: Config,
    pub path: PathBuf,
    pub was_created: bool,
}

impl Config {
    /// Load the config from disk, creating it with defaults on first run.
    /// Returns the loaded config along with a flag indicating whether this
    /// was a first-time creation (so `main` can print the welcome banner).
    pub fn load_or_init() -> CliResult<Loaded> {
        let path = config_path()?;

        if !path.exists() {
            let config = Config::default();
            save_to(&path, &config)?;
            return Ok(Loaded {
                config,
                path,
                was_created: true,
            });
        }

        let bytes = fs::read(&path).map_err(|e| {
            CliError::setup(
                format!("could not read config file at {}", path.display()),
                format!("check file permissions or delete it to regenerate: {e}"),
            )
        })?;

        let config: Config = serde_json::from_slice(&bytes).map_err(|e| {
            CliError::setup(
                format!("config file at {} is not valid JSON", path.display()),
                format!(
                    "delete the file to regenerate defaults, or fix the syntax manually: {e}"
                ),
            )
        })?;

        Ok(Loaded {
            config,
            path,
            was_created: false,
        })
    }

    pub fn save(&self, path: &PathBuf) -> CliResult<()> {
        save_to(path, self)
    }

    /// Mutate a single setting by dotted key (`defaults.rpc`, `display.color`).
    /// Returns an error describing the valid keys when the key is unknown.
    pub fn set(&mut self, key: &str, value: &str) -> CliResult<()> {
        match key {
            "defaults.chain" => {
                // Validate the value is a recognized chain; auto-pick the
                // matching public RPC iff the current one is still one of
                // the built-in defaults. This saves new users from having
                // to set both keys manually when switching chains.
                let chain = dexquote_core::Chain::parse(value).map_err(|_| {
                    CliError::input(
                        format!("unknown chain `{value}`"),
                        "supported: arbitrum, base, ethereum".to_string(),
                    )
                })?;
                self.defaults.chain = value.to_ascii_lowercase();
                let default_rpcs = [
                    dexquote_core::Chain::Arbitrum.default_public_rpc(),
                    dexquote_core::Chain::Base.default_public_rpc(),
                    dexquote_core::Chain::Ethereum.default_public_rpc(),
                ];
                if default_rpcs.contains(&self.defaults.rpc.as_str()) {
                    self.defaults.rpc = chain.default_public_rpc().to_string();
                }
            }
            "defaults.rpc" => self.defaults.rpc = value.to_string(),
            "defaults.timeout_ms" => {
                self.defaults.timeout_ms = value.parse().map_err(|e| {
                    CliError::input(
                        format!("timeout_ms must be an integer, got `{value}`"),
                        format!("parse error: {e}"),
                    )
                })?;
            }
            "display.color" => {
                if !["auto", "always", "never"].contains(&value) {
                    return Err(CliError::input(
                        format!("display.color must be auto/always/never, got `{value}`"),
                        "try: dexquote config set display.color auto".to_string(),
                    ));
                }
                self.display.color = value.into();
            }
            "display.format" => {
                if !["table", "json", "minimal"].contains(&value) {
                    return Err(CliError::input(
                        format!("display.format must be table/json/minimal, got `{value}`"),
                        "try: dexquote config set display.format table".to_string(),
                    ));
                }
                self.display.format = value.into();
            }
            "backends.enabled" => {
                self.backends.enabled = value
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
            other => {
                return Err(CliError::input(
                    format!("unknown config key `{other}`"),
                    "valid keys: defaults.chain, defaults.rpc, defaults.timeout_ms, \
                     display.color, display.format, backends.enabled"
                        .to_string(),
                ));
            }
        }
        Ok(())
    }
}

fn save_to(path: &PathBuf, config: &Config) -> CliResult<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            CliError::setup(
                format!("could not create config directory {}", parent.display()),
                format!("check permissions: {e}"),
            )
        })?;
    }
    let body = serde_json::to_string_pretty(config).unwrap_or_else(|_| "{}".into());
    fs::write(path, body).map_err(|e| {
        CliError::setup(
            format!("could not write config file {}", path.display()),
            format!("check permissions: {e}"),
        )
    })?;
    Ok(())
}

pub fn config_path() -> CliResult<PathBuf> {
    let base = dirs::config_dir().ok_or_else(|| {
        CliError::setup(
            "could not determine the user config directory".to_string(),
            "set the XDG_CONFIG_HOME environment variable or use --rpc instead".to_string(),
        )
    })?;
    Ok(base.join("dexquote").join("config.json"))
}
