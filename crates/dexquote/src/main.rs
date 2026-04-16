mod benchmark;
mod cli;
mod config;
mod depth;
mod doctor;
mod error;
mod history;
mod render;
mod route;
mod theme;
mod tui;

use cli::{Cli, Command, CompletionShell, ConfigCmd};
use clap::{CommandFactory, Parser};
use config::{Config, Loaded};
use dexquote_core::token::parse_amount;
use alloy::network::Ethereum;
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use dexquote_core::{
    list_tokens, quote_all, AerodromeBackend, BalancerV2Backend, CamelotV3Backend, Chain,
    CowSwapBackend, CurveBackend, DexBackend, DodoV2Backend, FraxSwapBackend, GasPricer,
    JupiterSwapBackend, JupiterUltraBackend, KyberSwapBackend, LiFiBackend, LiFiSolanaBackend,
    MaverickV2Backend, OdosBackend, OnChainContext, OpenOceanBackend, OpenOceanSolanaBackend,
    PancakeV3Backend, ParaswapBackend, QuoteRequest, RaydiumBackend, SlipstreamBackend,
    SushiV2Backend, Token, TraderJoeBackend, UniswapV2Backend, UniswapV3Backend, UniswapV4Backend,
};
use error::{CliError, CliResult};
use render::stream::{self, StreamConfig};
use render::table::PriorQuoteRef;
use render::{render_human, render_json, render_minimal, render_token_list, RenderInput};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use theme::{ColorMode, Theme};

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let theme = Theme::resolve(ColorMode::Auto);
            eprintln!("\n{}\n", err.render(theme.color));
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> CliResult<()> {
    // Shell completion is a special case — it doesn't touch the config file,
    // doesn't need RPC, doesn't need anything at all. Handle before load.
    if let Some(Command::Completions { shell }) = &cli.command {
        emit_completions(*shell);
        return Ok(());
    }

    let Loaded {
        mut config,
        path: config_path,
        was_created,
    } = Config::load_or_init()?;

    if was_created {
        print_welcome_banner(&config_path, &config);
    }

    // `last` is sugar for re-running the most recent history entry; we
    // rewrite the CLI in memory to have that entry's positional args and
    // fall through to the normal direct-mode path.
    if matches!(cli.command, Some(Command::Last)) {
        return handle_last(&config, cli).await;
    }

    if let Some(Command::History { filter, limit }) = &cli.command {
        return handle_history(filter.as_deref(), *limit);
    }

    // Doctor is a long-running diagnostic that uses its own dispatch path.
    if matches!(cli.command, Some(Command::Doctor)) {
        return doctor::run(&config, &config_path).await;
    }

    // Benchmark mode runs an internal pair sweep across all backends and
    // chains. Like Doctor it bypasses the positional-args path entirely.
    if let Some(Command::Benchmark { chain_filter, json }) = &cli.command {
        return benchmark::run(&config, chain_filter.as_deref(), *json).await;
    }

    // Depth mode runs the same pair at multiple notionals.
    if let Some(Command::Depth {
        sell_token,
        buy_token,
        amount,
    }) = &cli.command
    {
        return depth::run(&config, sell_token, buy_token, amount, cli.chain.as_deref()).await;
    }

    // Route mode shows the multi-hop path each backend used.
    if let Some(Command::Route {
        sell_token,
        buy_token,
        amount,
    }) = &cli.command
    {
        return route::run(&config, sell_token, buy_token, amount, cli.chain.as_deref()).await;
    }

    // Handle config/tokens subcommands up-front — these never need to quote.
    if let Some(cmd) = &cli.command {
        return handle_subcommand(cmd, &mut config, &config_path, cli.chain.clone()).await;
    }

    // Resolve effective settings: CLI flag > env var (already merged by clap) > config > builtin.
    let chain_str = cli.chain.clone().unwrap_or_else(|| config.defaults.chain.clone());
    let chain = Chain::parse(&chain_str).map_err(CliError::from)?;
    let chain_overridden = cli.chain.is_some()
        && !cli.chain.as_deref().map(|c| c.eq_ignore_ascii_case(&config.defaults.chain)).unwrap_or(false);

    // RPC resolution:
    //   1. explicit `--rpc` wins
    //   2. otherwise, if `--chain` overrode the config chain, pick that
    //      chain's public default so a one-off `--chain base` works without
    //      the user also passing `--rpc`
    //   3. otherwise inherit the config's RPC
    //   4. otherwise fall back to the chain's public RPC
    let rpc_url = if let Some(url) = cli.rpc.clone() {
        Some(url)
    } else if chain_overridden {
        Some(chain.default_public_rpc().to_string())
    } else if !config.defaults.rpc.is_empty() {
        Some(config.defaults.rpc.clone())
    } else {
        Some(chain.default_public_rpc().to_string())
    };

    let timeout_ms = cli.timeout.unwrap_or(config.defaults.timeout_ms);
    let color_mode = cli
        .color
        .as_deref()
        .map(ColorMode::parse)
        .unwrap_or_else(|| ColorMode::parse(&config.display.color));
    let theme = Theme::resolve(color_mode);

    let backend_names = cli.backends.clone().unwrap_or_else(|| config.backends.enabled.clone());
    let selection = parse_backend_names(&backend_names)?;

    // Decide: interactive (TUI) or direct mode?
    let force_interactive = cli.interactive;
    let any_positional = cli.sell_token.is_some() || cli.buy_token.is_some() || cli.amount.is_some();

    if (force_interactive || !any_positional) && Theme::is_tty() {
        // v1.2 unified TUI flow:
        //
        //  1. `dexquote --chain foo` (explicit chain, no positional args)
        //     → skip the main menu, go straight to the chain-pinned quote
        //     form. User already told us what they want. Esc from the
        //     form quits the TUI directly.
        //
        //  2. bare `dexquote` (no chain, no positional args)
        //     → enter the unified TUI which starts in Phase::MainMenu.
        //     The user picks an action, then a chain, all in one terminal
        //     session. When they exit the main menu, the TUI tears down
        //     and returns `TuiOutcome::Exit`. For actions that still use
        //     the v1.2 scaffolding's deferred dispatch (Depth, Route,
        //     Benchmark, Doctor, Tokens, History), the TUI returns
        //     `TuiOutcome::Deferred(action)` and we dispatch the matching
        //     subcommand below.
        if cli.chain.is_some() {
            run_tui(
                &config,
                chain,
                rpc_url,
                timeout_ms,
                &selection,
                tui::app::QuoteMode::Normal,
            )
            .await?;
            return Ok(());
        }

        let outcome = tui::run_unified(&config, &selection, chain, rpc_url, timeout_ms).await?;
        return dispatch_tui_outcome(outcome, &mut config, &config_path).await;
    }

    // Direct mode requires all three positionals.
    let sell_input = cli.sell_token.clone().ok_or_else(|| {
        CliError::input(
            "missing SELL_TOKEN".to_string(),
            "pass positional args or run `dexquote` with no args for interactive mode".to_string(),
        )
    })?;
    let buy_input = cli.buy_token.clone().ok_or_else(|| {
        CliError::input(
            "missing BUY_TOKEN".to_string(),
            "pass positional args or run `dexquote` with no args for interactive mode".to_string(),
        )
    })?;
    let amount_input = cli.amount.clone().ok_or_else(|| {
        CliError::input(
            "missing AMOUNT".to_string(),
            "pass positional args or run `dexquote` with no args for interactive mode".to_string(),
        )
    })?;

    let (sell, buy) = tokio::try_join!(
        resolve_token(&sell_input, chain, rpc_url.as_deref(), "sell token"),
        resolve_token(&buy_input, chain, rpc_url.as_deref(), "buy token"),
    )?;

    let amount_in = parse_amount(&amount_input, sell.decimals).map_err(CliError::from)?;

    validate_selection(&selection, rpc_url.is_some())?;

    let built = build_backends_with(
        &selection,
        chain,
        rpc_url.as_deref(),
        Duration::from_millis(timeout_ms),
        cli.at_block.is_some(),
    )
    .await?;
    let backends = built.backends;
    if backends.is_empty() {
        return Err(CliError::setup(
            "no backends selected".to_string(),
            "check your `--backends` flag or `backends.enabled` in config".to_string(),
        ));
    }

    let block_id = cli
        .at_block
        .map(|n| alloy::eips::BlockId::Number(alloy::eips::BlockNumberOrTag::Number(n)));

    let request = QuoteRequest {
        chain,
        token_in: sell,
        token_out: buy,
        amount_in,
        block_id,
    };

    let timeout = Duration::from_millis(timeout_ms);

    // --watch turns a one-shot into a loop with Ctrl-C cancellation.
    if let Some(spec) = &cli.watch {
        let interval = parse_duration(spec)?;
        return run_watch_loop(&request, &backends, timeout, theme, cli.json, cli.minimal, interval)
            .await;
    }

    run_single_quote(&request, &backends, timeout, theme, cli.json, cli.minimal).await
}

async fn run_single_quote(
    request: &QuoteRequest,
    backends: &[Arc<dyn DexBackend>],
    timeout: Duration,
    theme: Theme,
    json: bool,
    minimal: bool,
) -> CliResult<()> {
    // Look up the most recent history entry for this exact pair BEFORE
    // running and BEFORE writing the new record. That way the delta line
    // compares against the last quote, not against itself.
    let prior = load_prior_quote(request);

    // JSON / minimal output always uses the batch path — no spinners, no color.
    if json || minimal {
        let start = Instant::now();
        let results = quote_all(backends, request, timeout).await;
        let elapsed = start.elapsed().as_millis();
        history::record(&history::HistoryEntry::from_quote(request, &results, elapsed));
        if json {
            println!("{}", render_json(&results));
        } else {
            let line = render_minimal(&results, request);
            if !line.is_empty() {
                println!("{line}");
            }
        }
        return Ok(());
    }

    // Human mode on a TTY streams; piped mode batches.
    if theme.color || Theme::is_tty() {
        let outcome = stream::run(StreamConfig {
            request,
            backends,
            per_backend_timeout: timeout,
            theme,
        })
        .await;

        history::record(&history::HistoryEntry::from_quote(
            request,
            &outcome.results,
            outcome.total_elapsed_ms,
        ));

        let input = RenderInput {
            request,
            results: &outcome.results,
            total_elapsed_ms: outcome.total_elapsed_ms,
            theme,
            prior,
        };
        print!("{}", render::table::render_footer_only(&input));
    } else {
        let start = Instant::now();
        let results = quote_all(backends, request, timeout).await;
        let elapsed = start.elapsed().as_millis();
        history::record(&history::HistoryEntry::from_quote(request, &results, elapsed));
        let input = RenderInput {
            request,
            results: &results,
            total_elapsed_ms: elapsed,
            theme,
            prior,
        };
        print!("{}", render_human(&input));
    }

    Ok(())
}

fn load_prior_quote(request: &QuoteRequest) -> Option<PriorQuoteRef> {
    let entry = history::find_last_matching(
        request.chain.name(),
        &format!("{:?}", request.token_in.address),
        &format!("{:?}", request.token_out.address),
    )?;
    let best_out = entry.best_amount_out.as_ref()?.clone();
    Some(PriorQuoteRef {
        ts: entry.ts,
        sell_decimals: entry.sell_decimals,
        buy_decimals: entry.buy_decimals,
        amount_in_base_units: entry.amount_in,
        best_amount_out_base_units: best_out,
    })
}

/// Run `run_single_quote` in a loop until Ctrl-C. Between iterations we
/// clear the alternate screen via ANSI and redraw, so the terminal stays
/// tidy. History is recorded only for the first run of a watch session
/// (subsequent redraws would bloat the file).
async fn run_watch_loop(
    request: &QuoteRequest,
    backends: &[Arc<dyn DexBackend>],
    timeout: Duration,
    theme: Theme,
    json: bool,
    minimal: bool,
    interval: Duration,
) -> CliResult<()> {
    use tokio::signal::ctrl_c;

    eprintln!(
        " watching every {} · Ctrl-C to stop",
        format_watch_interval(interval)
    );

    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        // Fire once immediately then wait.
        ticker.tick().await;

        // Clear screen between iterations when we're drawing to a TTY.
        if !json && !minimal && Theme::is_tty() {
            print!("\x1b[2J\x1b[H"); // clear + cursor home
            std::io::Write::flush(&mut std::io::stdout()).ok();
        }

        let single = run_single_quote(request, backends, timeout, theme, json, minimal);
        tokio::select! {
            res = single => { res?; }
            _ = ctrl_c() => {
                eprintln!("\n stopping watch.");
                return Ok(());
            }
        }
    }
}

fn parse_duration(spec: &str) -> CliResult<Duration> {
    humantime::parse_duration(spec.trim()).map_err(|e| {
        CliError::input(
            format!("invalid --watch duration `{spec}`: {e}"),
            "try `30s`, `1m`, or `5m`".to_string(),
        )
    })
}

fn format_watch_interval(d: Duration) -> String {
    let secs = d.as_secs();
    if secs >= 60 && secs % 60 == 0 {
        format!("{}m", secs / 60)
    } else {
        format!("{secs}s")
    }
}

fn emit_completions(shell: CompletionShell) {
    let mut cmd = Cli::command();
    let name = cmd.get_name().to_string();
    clap_complete::generate(shell.to_clap(), &mut cmd, name, &mut std::io::stdout());
}

/// Open `path` in the user's preferred editor. Picks `$VISUAL`, then
/// `$EDITOR`, then falls back to a platform-appropriate default. After
/// the editor exits, re-reads the file and surfaces any JSON parse
/// errors so the user knows their edit broke the config.
fn open_config_in_editor(path: &std::path::Path) -> CliResult<()> {
    use std::process::Command as ProcCommand;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| {
            if cfg!(windows) {
                "notepad".to_string()
            } else {
                "nano".to_string()
            }
        });

    eprintln!("Opening {} in {editor}…", path.display());

    let status = ProcCommand::new(&editor)
        .arg(path)
        .status()
        .map_err(|e| {
            CliError::setup(
                format!("failed to launch editor `{editor}`: {e}"),
                "set $EDITOR to a different program, or edit the file manually".to_string(),
            )
        })?;

    if !status.success() {
        return Err(CliError::setup(
            format!("editor `{editor}` exited with status {status}"),
            "no changes were validated; open the file manually to inspect".to_string(),
        ));
    }

    // Validate the edited file by re-loading it. If parsing fails, the
    // error message includes the exact JSON error and points the user
    // back at the file.
    match Config::load_or_init() {
        Ok(_) => {
            println!("✓ config saved");
            Ok(())
        }
        Err(e) => Err(e),
    }
}

fn handle_history(filter: Option<&str>, limit: usize) -> CliResult<()> {
    let entries = history::read_all()?;
    if entries.is_empty() {
        println!(" no history yet — run a quote first");
        return Ok(());
    }

    let needle = filter.map(|s| s.to_ascii_lowercase());
    let filtered: Vec<&history::HistoryEntry> = entries
        .iter()
        .rev()
        .filter(|e| match &needle {
            None => true,
            Some(n) => {
                e.sell_symbol.to_ascii_lowercase().contains(n)
                    || e.buy_symbol.to_ascii_lowercase().contains(n)
            }
        })
        .take(limit)
        .collect();

    if filtered.is_empty() {
        println!(" no matching history entries");
        return Ok(());
    }

    println!();
    println!(" Recent quotes ({} shown):\n", filtered.len());
    for entry in &filtered {
        let ts_label = history::format_relative_ts(entry.ts);
        let best = entry.best_backend.as_deref().unwrap_or("—");
        let spread = entry
            .spread_pct
            .map(|p| format!("{:.2}%", p))
            .unwrap_or_else(|| "—".into());
        println!(
            " {:>14}  {:>6} {:<8} → {:<8}  best {:<10}  spread {}",
            ts_label,
            entry.amount_in_human(),
            entry.sell_symbol,
            entry.buy_symbol,
            best,
            spread,
        );
    }
    println!();
    Ok(())
}

async fn handle_last(_config: &Config, mut cli: Cli) -> CliResult<()> {
    let entries = history::read_all()?;
    let last = entries.last().ok_or_else(|| {
        CliError::input(
            "no history yet".to_string(),
            "run a quote first, then `dexquote last` replays it".to_string(),
        )
    })?;
    let replay = history::entry_to_request(last).ok_or_else(|| {
        CliError::bug("last history entry references an unknown chain".to_string())
    })?;

    // Rewrite the CLI args and strip the subcommand so the normal direct-mode
    // path takes over from here.
    cli.sell_token = Some(replay.sell_input);
    cli.buy_token = Some(replay.buy_input);
    cli.amount = Some(replay.amount_human);
    cli.chain = Some(replay.chain.name().to_ascii_lowercase());
    cli.command = None;
    Box::pin(run(cli)).await
}

async fn run_tui(
    config: &Config,
    chain: Chain,
    rpc_url: Option<String>,
    timeout_ms: u64,
    selection: &[BackendKind],
    mode: tui::app::QuoteMode,
) -> CliResult<tui::TuiOutcome> {
    let built = build_backends(
        selection,
        chain,
        rpc_url.as_deref(),
        Duration::from_millis(timeout_ms),
    )
    .await?;
    if built.backends.is_empty() {
        return Err(CliError::setup(
            "no backends selected".to_string(),
            "check `backends.enabled` in config".to_string(),
        ));
    }
    let ctx = tui::TuiContext {
        chain,
        rpc_url,
        timeout_ms,
        backends: built.backends,
        provider: built.provider,
        mode,
    };
    tui::run(config, ctx).await
}

/// Dispatch a main-menu selection into the appropriate flow. Called
/// Dispatch a `TuiOutcome` returned by the unified TUI. `Exit` is a
/// clean shutdown; `Deferred(action)` means the user picked a non-
/// Quote action that the v1.2 scaffolding doesn't yet render in-TUI
/// — the TUI has torn down, and we run the matching subcommand on
/// the now-restored terminal.
///
/// Phases 2-6 of the v1.2 plan will progressively shrink the
/// `Deferred` cases: once all actions render in-TUI, this function
/// reduces to a no-op (the TUI returns `Exit` for every path).
async fn dispatch_tui_outcome(
    outcome: tui::TuiOutcome,
    config: &mut Config,
    config_path: &std::path::Path,
) -> CliResult<()> {
    // v1.2 complete: every menu action runs fully inside the TUI,
    // so the only outcome the TUI returns is `Exit`. The
    // `config`/`config_path` params are kept as dispatcher inputs
    // in case a future v1.3+ adds a new deferred shape.
    let _ = (config, config_path);
    match outcome {
        tui::TuiOutcome::Exit => Ok(()),
    }
}

async fn handle_subcommand(
    cmd: &Command,
    config: &mut Config,
    path: &std::path::Path,
    cli_chain_hint: Option<String>,
) -> CliResult<()> {
    let theme = Theme::resolve(ColorMode::Auto);
    match cmd {
        Command::Config(ConfigCmd::Show) => {
            println!("config path: {}\n", path.display());
            let body =
                serde_json::to_string_pretty(&*config).unwrap_or_else(|_| "{}".to_string());
            println!("{body}");
        }
        Command::Config(ConfigCmd::Set { key, value }) => {
            config.set(key, value)?;
            config.save(&path.to_path_buf())?;
            println!("✓ set {key} = {value}");
        }
        Command::Config(ConfigCmd::Path) => {
            println!("{}", path.display());
        }
        Command::Config(ConfigCmd::Edit) => {
            open_config_in_editor(path)?;
        }
        Command::Config(ConfigCmd::Reset) => {
            let defaults = Config::default();
            defaults.save(&path.to_path_buf())?;
            println!("✓ config reset to defaults at {}", path.display());
        }
        Command::Tokens { filter } => {
            // Prefer --chain override if present, fall back to config default.
            // Without this, `dexquote tokens --chain solana` would silently
            // use the config's default chain, which is confusing.
            let chain_str_owned;
            let chain_str: &str = if let Some(c) = cli_chain_hint.as_deref() {
                c
            } else {
                chain_str_owned = config.defaults.chain.clone();
                &chain_str_owned
            };
            let chain = Chain::parse(chain_str).map_err(CliError::from)?;
            let mut tokens: Vec<Token> = list_tokens(chain);
            if let Some(f) = filter {
                let needle = f.to_ascii_lowercase();
                tokens.retain(|t| {
                    t.symbol.to_ascii_lowercase().contains(&needle)
                        || t.name.to_ascii_lowercase().contains(&needle)
                });
            }
            print!("{}", render_token_list(&tokens, theme));
        }
        // These subcommands are handled earlier in `run`:
        //   Completions → emit_completions before config load
        //   Last        → rewrites the Cli and recurses into run()
        //   History     → handle_history
        //   Doctor      → doctor::run
        //   Benchmark   → benchmark::run
        // Reaching them here means the dispatch got crossed wires.
        Command::Last
        | Command::History { .. }
        | Command::Completions { .. }
        | Command::Doctor
        | Command::Benchmark { .. }
        | Command::Depth { .. }
        | Command::Route { .. } => {
            return Err(CliError::bug(
                "subcommand reached the wrong dispatch arm".to_string(),
            ));
        }
    }
    Ok(())
}

fn print_welcome_banner(path: &std::path::Path, config: &Config) {
    let theme = Theme::resolve(ColorMode::Auto);
    let title = if theme.color {
        use colored::Colorize;
        "Welcome to dexquote.".bold().to_string()
    } else {
        "Welcome to dexquote.".to_string()
    };
    eprintln!();
    eprintln!("  {title}");
    eprintln!();
    eprintln!("  I wrote a default config at:");
    eprintln!("    {}", path.display());
    eprintln!();
    eprintln!("  It's set up to quote on {} using the public RPC:", config.defaults.chain);
    eprintln!("    {}", config.defaults.rpc);
    eprintln!();
    eprintln!("  Supported chains: arbitrum, base, ethereum. Switch with:");
    eprintln!("    dexquote config set defaults.chain base");
    eprintln!("    dexquote config set defaults.rpc https://mainnet.base.org");
    eprintln!();
    eprintln!("  The public endpoint is rate-limited. To use your own (Alchemy, Infura, etc.):");
    eprintln!("    dexquote config set defaults.rpc https://your-rpc-url");
    eprintln!();
    eprintln!("  Try:");
    eprintln!("    dexquote WETH USDC 1.0             # direct quote on default chain");
    eprintln!("    dexquote --chain base WETH USDC 1  # one-off chain override");
    eprintln!("    dexquote                           # interactive TUI");
    eprintln!("    dexquote tokens                    # browse bundled tokens");
    eprintln!("    dexquote --help                    # full help");
    eprintln!();
}

async fn resolve_token(
    input: &str,
    chain: Chain,
    rpc_url: Option<&str>,
    label: &str,
) -> CliResult<Token> {
    Token::resolve(input, chain, rpc_url).await.map_err(|e| {
        let base: CliError = e.into();
        CliError {
            category: base.category,
            message: format!("{}: {}", label, base.message),
            hint: base.hint,
        }
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    UniswapV2,
    UniswapV3,
    UniswapV4,
    SushiV2,
    FraxSwap,
    TraderJoe,
    PancakeV3,
    CamelotV3,
    Curve,
    Aerodrome,
    Slipstream,
    BalancerV2,
    MaverickV2,
    DodoV2,
    Odos,
    Paraswap,
    KyberSwap,
    OpenOcean,
    LiFi,
    CowSwap,
    // Solana-native HTTP aggregators (v1.0).
    JupiterSwap,
    JupiterUltra,
    Raydium,
    OpenOceanSolana,
    LiFiSolana,
}

impl BackendKind {
    fn is_on_chain(self) -> bool {
        matches!(
            self,
            BackendKind::UniswapV2
                | BackendKind::UniswapV3
                | BackendKind::UniswapV4
                | BackendKind::SushiV2
                | BackendKind::FraxSwap
                | BackendKind::TraderJoe
                | BackendKind::PancakeV3
                | BackendKind::CamelotV3
                | BackendKind::Curve
                | BackendKind::Aerodrome
                | BackendKind::Slipstream
                | BackendKind::BalancerV2
                | BackendKind::MaverickV2
                | BackendKind::DodoV2
        )
    }

    /// Whether this backend has any deployment / live support on the given
    /// chain. HTTP aggregators return `true` for every chain because they
    /// just need a chain-id parameter; on-chain backends delegate to their
    /// per-backend `supports()` helper which checks the address lookup.
    fn supports_chain(self, chain: Chain) -> bool {
        match self {
            BackendKind::UniswapV2 => UniswapV2Backend::supports(chain),
            BackendKind::UniswapV3 => UniswapV3Backend::supports(chain),
            BackendKind::UniswapV4 => UniswapV4Backend::supports(chain),
            BackendKind::SushiV2 => SushiV2Backend::supports(chain),
            BackendKind::FraxSwap => FraxSwapBackend::supports(chain),
            BackendKind::TraderJoe => TraderJoeBackend::supports(chain),
            BackendKind::PancakeV3 => PancakeV3Backend::supports(chain),
            BackendKind::CamelotV3 => CamelotV3Backend::supports(chain),
            BackendKind::Curve => CurveBackend::supports(chain),
            BackendKind::Aerodrome => AerodromeBackend::supports(chain),
            BackendKind::Slipstream => SlipstreamBackend::supports(chain),
            BackendKind::BalancerV2 => BalancerV2Backend::supports(chain),
            BackendKind::MaverickV2 => MaverickV2Backend::supports(chain),
            BackendKind::DodoV2 => DodoV2Backend::supports(chain),
            // EVM aggregators: true for every EVM chain, false for Solana.
            BackendKind::Odos
            | BackendKind::Paraswap
            | BackendKind::KyberSwap
            | BackendKind::OpenOcean
            | BackendKind::LiFi
            | BackendKind::CowSwap => !matches!(chain, Chain::Solana),
            // Solana aggregators: delegate to each backend's supports().
            BackendKind::JupiterSwap => JupiterSwapBackend::supports(chain),
            BackendKind::JupiterUltra => JupiterUltraBackend::supports(chain),
            BackendKind::Raydium => RaydiumBackend::supports(chain),
            BackendKind::OpenOceanSolana => OpenOceanSolanaBackend::supports(chain),
            BackendKind::LiFiSolana => LiFiSolanaBackend::supports(chain),
        }
    }

    fn parse(input: &str) -> Option<Self> {
        match input.to_ascii_lowercase().replace(['_', '-'], "").as_str() {
            "uniswapv2" | "univ2" | "uni2" => Some(Self::UniswapV2),
            "uniswapv3" | "univ3" | "uniswap" => Some(Self::UniswapV3),
            "uniswapv4" | "univ4" | "uni4" => Some(Self::UniswapV4),
            "sushiv2" | "sushiswap" | "sushi" => Some(Self::SushiV2),
            "fraxswap" | "frax" => Some(Self::FraxSwap),
            "traderjoe" | "tj" | "lb" | "lfj" => Some(Self::TraderJoe),
            "pancakev3" | "pancakeswap" | "pancake" | "cake" => Some(Self::PancakeV3),
            "camelotv3" | "camelot" => Some(Self::CamelotV3),
            "curve" | "crv" => Some(Self::Curve),
            "aerodrome" | "aero" => Some(Self::Aerodrome),
            "slipstream" | "aeroslipstream" | "aerodromeslipstream" => Some(Self::Slipstream),
            "balancerv2" | "balancer" | "bal" => Some(Self::BalancerV2),
            "maverickv2" | "maverick" | "mav" => Some(Self::MaverickV2),
            "dodov2" | "dodo" => Some(Self::DodoV2),
            "jupiter" | "jup" | "jupiterswap" | "jupswap" => Some(Self::JupiterSwap),
            "jupiterultra" | "jupultra" | "ultra" => Some(Self::JupiterUltra),
            "raydium" | "ray" => Some(Self::Raydium),
            "openoceansol" | "openoceansolana" | "oo-sol" => Some(Self::OpenOceanSolana),
            "lifisol" | "lifisolana" | "lifi-sol" => Some(Self::LiFiSolana),
            "odos" => Some(Self::Odos),
            "paraswap" | "velora" => Some(Self::Paraswap),
            "kyberswap" | "kyber" => Some(Self::KyberSwap),
            "openocean" | "oo" => Some(Self::OpenOcean),
            "lifi" | "li.fi" | "liquest" => Some(Self::LiFi),
            "cowswap" | "cow" | "cowprotocol" => Some(Self::CowSwap),
            _ => None,
        }
    }

    fn all_default_order() -> [BackendKind; 25] {
        [
            BackendKind::UniswapV2,
            BackendKind::UniswapV3,
            BackendKind::UniswapV4,
            BackendKind::SushiV2,
            BackendKind::FraxSwap,
            BackendKind::TraderJoe,
            BackendKind::PancakeV3,
            BackendKind::CamelotV3,
            BackendKind::Curve,
            BackendKind::Aerodrome,
            BackendKind::Slipstream,
            BackendKind::BalancerV2,
            BackendKind::MaverickV2,
            BackendKind::DodoV2,
            BackendKind::Odos,
            BackendKind::Paraswap,
            BackendKind::KyberSwap,
            BackendKind::OpenOcean,
            BackendKind::LiFi,
            BackendKind::CowSwap,
            BackendKind::JupiterSwap,
            BackendKind::JupiterUltra,
            BackendKind::Raydium,
            BackendKind::OpenOceanSolana,
            BackendKind::LiFiSolana,
        ]
    }
}

fn parse_backend_names(raw: &[String]) -> CliResult<Vec<BackendKind>> {
    let parsed: Vec<BackendKind> = raw
        .iter()
        .filter_map(|r| BackendKind::parse(r.trim()))
        .collect();
    if parsed.is_empty() && !raw.is_empty() {
        return Err(CliError::input(
            format!("no recognized backends in `{}`", raw.join(",")),
            "valid: uniswap-v3, sushi-v2, trader-joe, pancake-v3, camelot-v3, \
             odos, paraswap, kyberswap, openocean, lifi"
                .to_string(),
        ));
    }
    if parsed.is_empty() {
        return Ok(BackendKind::all_default_order().to_vec());
    }
    Ok(parsed)
}

fn validate_selection(selection: &[BackendKind], has_rpc: bool) -> CliResult<()> {
    if !has_rpc && selection.iter().any(|b| b.is_on_chain()) {
        return Err(CliError::setup(
            "on-chain backends need an RPC endpoint".to_string(),
            "pass --rpc <URL>, set DEXQUOTE_RPC, or `dexquote config set defaults.rpc <URL>`"
                .to_string(),
        ));
    }
    Ok(())
}

/// Collected output of `build_backends`: the list of ready-to-call backends
/// plus the shared provider (when an on-chain backend was selected). The
/// TUI borrows the provider to spawn its live gas tracker; direct-mode
/// callers ignore it.
pub struct BuiltBackends {
    pub backends: Vec<Arc<dyn DexBackend>>,
    pub provider: Option<DynProvider<Ethereum>>,
}

/// Connect to the RPC exactly once (if any on-chain backend is selected),
/// build a single `reqwest::Client` shared across all HTTP backends, then
/// hand them out to every selected backend. A 10-backend quote now makes
/// 1 TCP handshake to the RPC (not 5) and reuses a single connection pool
/// for the aggregators — trimming ~200–500ms off every cold run.
///
/// Filters the selection by `supports_chain(chain)` so backends that have
/// no deployment on the active chain (Trader Joe on Base, Aerodrome on
/// Arbitrum, etc.) are silently dropped instead of surfacing as NoRoute
/// rows on every quote.
pub(crate) async fn build_backends(
    selection: &[BackendKind],
    chain: Chain,
    rpc_url: Option<&str>,
    timeout: Duration,
) -> CliResult<BuiltBackends> {
    build_backends_with(selection, chain, rpc_url, timeout, false).await
}

/// Same as `build_backends` but lets the caller force aggregator
/// filtering when replaying historical blocks. Aggregators can't replay
/// against arbitrary block heights, so we silently drop them when the
/// user passes `--at-block`.
pub(crate) async fn build_backends_with(
    selection: &[BackendKind],
    chain: Chain,
    rpc_url: Option<&str>,
    timeout: Duration,
    historical_replay: bool,
) -> CliResult<BuiltBackends> {
    // Drop backends that have nothing to say on this chain. When
    // historical replay is active, also drop HTTP aggregators because
    // they can't quote against past blocks.
    let selection: Vec<BackendKind> = selection
        .iter()
        .copied()
        .filter(|k| k.supports_chain(chain))
        .filter(|k| !historical_replay || k.is_on_chain())
        .collect();
    let selection = &selection;
    let needs_provider = selection.iter().any(|k| k.is_on_chain());

    let provider: Option<DynProvider<Ethereum>> = if needs_provider {
        let url = rpc_url.ok_or_else(|| {
            CliError::setup(
                "on-chain backends need an RPC endpoint".to_string(),
                "pass --rpc <URL>, set DEXQUOTE_RPC, or `dexquote config set defaults.rpc <URL>`"
                    .to_string(),
            )
        })?;
        let p = ProviderBuilder::new()
            .connect(url)
            .await
            .map_err(|e| {
                CliError::network(
                    format!("could not connect to RPC: {e}"),
                    format!("check that {url} is reachable and returns JSON-RPC"),
                )
            })?
            .erased();
        Some(p)
    } else {
        None
    };

    let gas_pricer = GasPricer::new(chain, provider.clone());

    let ctx = provider.as_ref().map(|p| OnChainContext {
        provider: p.clone(),
        gas_pricer: gas_pricer.clone(),
    });

    // One HTTP client shared across every aggregator backend. reqwest pools
    // connections per-client, so sharing gives us keep-alive reuse across
    // Paraswap → KyberSwap → OpenOcean → LiFi → ODOS when they happen to
    // hit compatible hosts, and a consistent user-agent/timeout policy.
    //
    // Per-stage timeouts: the connect timeout is capped at 1/4 of the
    // overall budget (or 2s, whichever is smaller) so a single misbehaving
    // host that refuses TCP can't gobble the entire request budget. The
    // overall `.timeout(...)` then bounds total request lifetime including
    // the body read. `pool_idle_timeout` keeps connections warm long
    // enough to benefit from reuse across consecutive runs but not so
    // long that the OS reaps them.
    let connect_budget = std::cmp::min(timeout / 4, Duration::from_secs(2));
    let http_client = reqwest::Client::builder()
        .timeout(timeout)
        .connect_timeout(connect_budget)
        .pool_idle_timeout(Duration::from_secs(30))
        .user_agent(concat!("dexquote/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let mut backends: Vec<Arc<dyn DexBackend>> = Vec::with_capacity(selection.len());
    for kind in selection {
        let b: Option<Arc<dyn DexBackend>> = match kind {
            BackendKind::Odos => Some(Arc::new(OdosBackend::with_client(http_client.clone()))),
            BackendKind::Paraswap => Some(Arc::new(ParaswapBackend::with_client(http_client.clone()))),
            BackendKind::KyberSwap => Some(Arc::new(KyberSwapBackend::with_client(http_client.clone()))),
            BackendKind::OpenOcean => Some(Arc::new(OpenOceanBackend::with_client(http_client.clone()))),
            BackendKind::LiFi => Some(Arc::new(LiFiBackend::with_client(http_client.clone()))),
            BackendKind::CowSwap => Some(Arc::new(CowSwapBackend::with_client(http_client.clone()))),
            BackendKind::UniswapV2 => ctx.clone().map(|c| Arc::new(UniswapV2Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::UniswapV3 => ctx.clone().map(|c| Arc::new(UniswapV3Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::UniswapV4 => ctx.clone().map(|c| Arc::new(UniswapV4Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::SushiV2 => ctx.clone().map(|c| Arc::new(SushiV2Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::FraxSwap => ctx.clone().map(|c| Arc::new(FraxSwapBackend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::TraderJoe => ctx.clone().map(|c| Arc::new(TraderJoeBackend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::PancakeV3 => ctx.clone().map(|c| Arc::new(PancakeV3Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::CamelotV3 => ctx.clone().map(|c| Arc::new(CamelotV3Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::Curve => ctx.clone().map(|c| Arc::new(CurveBackend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::Aerodrome => ctx.clone().map(|c| Arc::new(AerodromeBackend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::Slipstream => ctx.clone().map(|c| Arc::new(SlipstreamBackend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::BalancerV2 => ctx.clone().map(|c| Arc::new(BalancerV2Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::MaverickV2 => ctx.clone().map(|c| Arc::new(MaverickV2Backend::new(c)) as Arc<dyn DexBackend>),
            BackendKind::DodoV2 => ctx.clone().map(|c| Arc::new(DodoV2Backend::new(c)) as Arc<dyn DexBackend>),
            // Solana HTTP aggregators — no ctx needed, just the shared client.
            BackendKind::JupiterSwap => Some(Arc::new(JupiterSwapBackend::with_client(http_client.clone()))),
            BackendKind::JupiterUltra => Some(Arc::new(JupiterUltraBackend::with_client(http_client.clone()))),
            BackendKind::Raydium => Some(Arc::new(RaydiumBackend::with_client(http_client.clone()))),
            BackendKind::OpenOceanSolana => Some(Arc::new(OpenOceanSolanaBackend::with_client(http_client.clone()))),
            BackendKind::LiFiSolana => Some(Arc::new(LiFiSolanaBackend::with_client(http_client.clone()))),
        };
        if let Some(b) = b {
            backends.push(b);
        }
    }

    Ok(BuiltBackends { backends, provider })
}
