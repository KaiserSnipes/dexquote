use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

#[derive(Parser, Debug)]
#[command(
    name = "dexquote",
    version,
    about = "Compare DEX quotes from the terminal",
    long_about = "dexquote fetches swap quotes for a token pair from multiple DEXs in \
                  parallel. Run with no arguments for an interactive full-screen picker, \
                  or pass positional args for a scriptable one-shot quote. Read-only — \
                  no trades are executed."
)]
pub struct Cli {
    /// Token to sell (symbol like WETH or a 0x… address)
    pub sell_token: Option<String>,

    /// Token to buy (symbol or 0x… address)
    pub buy_token: Option<String>,

    /// Amount of sell token in human units (e.g. 1.0)
    pub amount: Option<String>,

    /// Chain to quote on
    #[arg(long, global = true)]
    pub chain: Option<String>,

    /// RPC endpoint (also: DEXQUOTE_RPC env var, or defaults.rpc in config)
    #[arg(long, env = "DEXQUOTE_RPC", global = true)]
    pub rpc: Option<String>,

    /// Output format override
    #[arg(long, global = true)]
    pub json: bool,

    /// Single-line scripting output (prints the best quote, tab-separated)
    #[arg(long, global = true, conflicts_with = "json")]
    pub minimal: bool,

    /// Comma-separated list of backends to query
    #[arg(long, value_delimiter = ',', global = true)]
    pub backends: Option<Vec<String>>,

    /// Per-backend timeout in milliseconds
    #[arg(long, global = true)]
    pub timeout: Option<u64>,

    /// Colorization: auto / always / never
    #[arg(long, global = true)]
    pub color: Option<String>,

    /// Force interactive full-screen mode even if positional args are given
    #[arg(short = 'i', long, global = true)]
    pub interactive: bool,

    /// Re-run the quote on an interval (e.g. 30s, 1m, 5m). Ctrl-C quits.
    #[arg(long, value_name = "DURATION", global = true)]
    pub watch: Option<String>,

    /// Replay the quote against a historical block height (on-chain
    /// backends only). HTTP aggregators are filtered silently because
    /// they can't replay arbitrary blocks. Useful for backtesting and
    /// for understanding what dexquote would have shown at a past moment.
    #[arg(long, value_name = "BLOCK", global = true)]
    pub at_block: Option<u64>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Show, set, or locate the config file
    #[command(subcommand)]
    Config(ConfigCmd),

    /// Print the bundled token registry
    Tokens {
        /// Optional substring to filter by symbol or name
        filter: Option<String>,
    },

    /// Re-run the most recent quote
    Last,

    /// Browse recent quotes
    History {
        /// Optional substring to filter by token symbol
        filter: Option<String>,
        /// Limit to the N most recent entries
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },

    /// Print a shell completion script
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: CompletionShell,
    },

    /// Self-test: probe the RPC, Chainlink feed, and every backend.
    Doctor,

    /// Run a fixed benchmark suite across every backend on every chain.
    /// Outputs a leaderboard of per-backend win count, success rate,
    /// median latency, and average spread vs the per-pair median.
    Benchmark {
        /// Restrict the sweep to a single chain (default: all chains).
        #[arg(long, value_name = "CHAIN")]
        chain_filter: Option<String>,
        /// Output JSON instead of the human-readable leaderboard.
        #[arg(long)]
        json: bool,
    },

    /// Quote the same pair at multiple notionals (0.1× → 1000×) and
    /// render the price-impact curve. Helps see how much of an order
    /// a venue can fill before slippage eats it.
    Depth {
        /// Token to sell (symbol or 0x… address)
        sell_token: String,
        /// Token to buy (symbol or 0x… address)
        buy_token: String,
        /// Base amount in human units (e.g. `1.0`). The 0.1× / 1× / 10× /
        /// 100× / 1000× multipliers are applied to this number.
        amount: String,
    },

    /// Show the multi-hop path each backend used for the quote.
    /// Surfaces the underlying venues (UniV3, Curve, Balancer, ...) that
    /// aggregators actually route through — information you can't get
    /// from any other public DEX comparison tool.
    Route {
        /// Token to sell (symbol or 0x… address)
        sell_token: String,
        /// Token to buy (symbol or 0x… address)
        buy_token: String,
        /// Amount in human units (e.g. `1.0`).
        amount: String,
    },
}

/// Thin wrapper around `clap_complete::Shell` that implements `ValueEnum`
/// for the derive API.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum CompletionShell {
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}

impl CompletionShell {
    pub fn to_clap(self) -> Shell {
        match self {
            CompletionShell::Bash => Shell::Bash,
            CompletionShell::Zsh => Shell::Zsh,
            CompletionShell::Fish => Shell::Fish,
            CompletionShell::Powershell => Shell::PowerShell,
            CompletionShell::Elvish => Shell::Elvish,
        }
    }
}

#[derive(Subcommand, Debug)]
pub enum ConfigCmd {
    /// Print the current config (with the file path)
    Show,
    /// Set a config value by dotted key, e.g. `defaults.rpc https://…`
    Set { key: String, value: String },
    /// Print the config file path
    Path,
    /// Open the config file in $EDITOR (falls back to notepad/nano)
    Edit,
    /// Reset the config file to built-in defaults
    Reset,
}
