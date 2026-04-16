//! Full-screen interactive TUI built on ratatui + crossterm.
//!
//! Architecture:
//!   - `app.rs` holds the state machine (fields, picker mode, results).
//!   - `ui.rs` renders the state into ratatui widgets each frame.
//!   - `event.rs` maps keyboard events into state transitions.
//!   - `mod.rs` (this file) owns the terminal setup/teardown and the main
//!     loop. It's the only place that touches the real `stdout`.
//!
//! The quoting engine is called through the exact same path the non-TUI
//! code uses, so the TUI and direct-mode CLI can't drift.

pub mod app;
mod benchmark_stream;
mod depth_stream;
mod doctor_stream;
mod event;
mod gas_tracker;
mod quote_stream;
mod solana_gas_tracker;
mod ui;

use crate::config::Config;
use crate::error::{CliError, CliResult};
use crate::BackendKind;
use alloy::network::Ethereum;
use alloy::providers::DynProvider;
use app::{App, Field, MenuAction, PendingQuote, Phase, QuoteMode};
use crossterm::event::{self as xt_event, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use dexquote_core::{Chain, DexBackend, QuoteRequest};
use ratatui::prelude::*;
use ratatui::Terminal;
use std::io::{self, Stdout};
use std::sync::Arc;
use std::time::Duration;

pub struct TuiContext {
    pub chain: Chain,
    /// Kept for a future v0.2 that resolves custom addresses asynchronously
    /// inside the TUI loop.
    #[allow(dead_code)]
    pub rpc_url: Option<String>,
    pub timeout_ms: u64,
    pub backends: Vec<Arc<dyn DexBackend>>,
    /// Optional shared provider for the live gas tracker. `None` when the
    /// user is running ODOS-only (no RPC), in which case the header shows
    /// a single "no RPC" placeholder instead of live gas data.
    pub provider: Option<DynProvider<Ethereum>>,
    /// What the Quote TUI should do on Find Quote. `Normal` is the
    /// existing streaming quote flow; `Depth` / `Route` stash a
    /// deferred action and exit so the dispatcher can run the matching
    /// subcommand with the collected inputs.
    pub mode: QuoteMode,
}

/// How the Quote TUI exited. The dispatcher in `main.rs` matches on
/// this to decide whether to simply exit or to run a deferred
/// v1.2: the TUI is a self-contained session — every action runs
/// fully inside the window. The only exit path is the user
/// quitting from the main menu, so this enum has a single variant.
/// Kept as an enum rather than unit `()` to leave room for a
/// future "return-a-result-to-the-shell" variant without another
/// API break.
pub enum TuiOutcome {
    Exit,
}

/// Launch the full-screen TUI and drive it until the user quits (Esc or
/// Ctrl-C). Returns a `TuiOutcome` describing how the session ended —
/// `Exit` for a clean quit, or a `Deferred*` variant carrying the
/// user's collected inputs when the Quote TUI was running in Depth or
/// Route mode and the user pressed Find Quote. The dispatcher in
/// `main.rs` runs the matching subcommand against the deferred inputs
/// AFTER this function returns (i.e. after terminal teardown), so the
/// subcommand's output lands on a clean terminal.
pub async fn run(config: &Config, ctx: TuiContext) -> CliResult<TuiOutcome> {
    let mut terminal = setup_terminal()?;
    let result = run_inner(&mut terminal, config, ctx).await;
    restore_terminal(&mut terminal).ok();
    result
}

/// v1.2 unified entry point. Bare `dexquote` (no `--chain`, no
/// positional args) calls this. Starts in `Phase::MainMenu` and
/// drives the whole TUI lifecycle in one terminal session. When
/// the user picks a chain from the in-TUI chain picker, this
/// function rebuilds `TuiContext.backends` for the new chain and
/// respawns the gas tracker before transitioning to the next phase.
///
/// Returns `TuiOutcome::Exit` when the user quits from the main
/// menu, or `TuiOutcome::Deferred(action)` when the user picks a
/// non-Quote action the v1.2 scaffolding doesn't yet render
/// in-TUI. Phases 2-6 of the v1.2 plan progressively shrink the
/// deferred set until only `Exit` remains.
pub async fn run_unified(
    config: &Config,
    selection: &[BackendKind],
    default_chain: Chain,
    initial_rpc_url: Option<String>,
    timeout_ms: u64,
) -> CliResult<TuiOutcome> {
    let mut terminal = setup_terminal()?;
    let result = run_unified_inner(
        &mut terminal,
        config,
        selection,
        default_chain,
        initial_rpc_url,
        timeout_ms,
    )
    .await;
    restore_terminal(&mut terminal).ok();
    result
}

async fn run_unified_inner(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    config: &Config,
    selection: &[BackendKind],
    default_chain: Chain,
    initial_rpc_url: Option<String>,
    timeout_ms: u64,
) -> CliResult<TuiOutcome> {
    let mut app = App::new(default_chain);
    // `ctx` is `None` until the user picks a chain from the in-TUI
    // chain picker. Once picked, we build the backends + gas tracker
    // for that chain and stash them here for the remainder of the
    // session (or until the user returns to the menu and picks a
    // different chain, in which case we tear down and rebuild).
    let mut ctx: Option<TuiContext> = None;
    let mut effective_rpc = initial_rpc_url;

    loop {
        app.drain_pending();
        app.drain_depth();
        app.drain_benchmark();
        app.drain_doctor();
        app.drain_custom_token();
        app.drain_gas();

        terminal
            .draw(|frame| ui::draw(frame, &app))
            .map_err(|e| CliError::bug(format!("tui draw failed: {e}")))?;

        // Spinner animates during any in-flight async phase.
        if matches!(
            app.phase,
            Phase::Quoting | Phase::Depthing | Phase::Benchmarking | Phase::Doctoring
        ) {
            app.spinner_frame = app.spinner_frame.wrapping_add(1);
        }

        let poll_timeout = if matches!(
            app.phase,
            Phase::Quoting | Phase::Depthing | Phase::Benchmarking | Phase::Doctoring
        ) {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(80)
        };

        if xt_event::poll(poll_timeout)
            .map_err(|e| CliError::bug(format!("tui event poll failed: {e}")))?
        {
            if let Event::Key(key) = xt_event::read()
                .map_err(|e| CliError::bug(format!("tui event read failed: {e}")))?
            {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let intent = event::map_key(key, &app);
                let should_quote = app.handle(intent);

                // Chain picker just fired: rebuild backends + gas
                // tracker, then transition to the action's next
                // phase (Form for Quote/Depth/Route, or deferred
                // for Benchmark/Doctor/Tokens).
                if app.pending_transition {
                    app.pending_transition = false;
                    if let Err(e) = handle_chain_picked(
                        &mut app,
                        &mut ctx,
                        &mut effective_rpc,
                        selection,
                        default_chain,
                        timeout_ms,
                        config,
                    )
                    .await
                    {
                        // Setup error (e.g. RPC unreachable). Bounce
                        // back to the main menu with the error in the
                        // status bar.
                        app.status = format!("Setup failed: {e}");
                        app.phase = Phase::MainMenu;
                        app.pending_action = None;
                        app.chain_picker_include_all = false;
                    }
                }

                if app.should_quit {
                    return Ok(TuiOutcome::Exit);
                }

                if should_quote {
                    if let Some(c) = ctx.as_ref() {
                        start_quote(&mut app, c);
                    } else {
                        app.status = "Internal error: no backends loaded yet".into();
                    }
                }
            }
        }
    }
}

/// Handle the chain-picker Activate event: either rebuild the
/// `TuiContext` for the newly picked chain (Quote/Depth/Route) or
/// set a `DeferredAction` and exit the TUI (Benchmark/Doctor/Tokens).
/// Called from `run_unified_inner` after `App::handle` sets
/// `pending_transition`.
#[allow(clippy::too_many_arguments)]
async fn handle_chain_picked(
    app: &mut App,
    ctx: &mut Option<TuiContext>,
    effective_rpc: &mut Option<String>,
    selection: &[BackendKind],
    default_chain: Chain,
    timeout_ms: u64,
    config: &Config,
) -> CliResult<()> {
    let Some(action) = app.pending_action else {
        return Ok(());
    };
    let picked = app.chain_picker_selected();

    match action {
        MenuAction::Benchmark => {
            // v1.2 Phase 5: run the benchmark sweep inside the
            // TUI, streaming per-pair progress through an mpsc
            // channel. Esc during the run drops the receiver and
            // bounces back to the menu.
            let rx = benchmark_stream::spawn(
                selection.to_vec(),
                picked,
                Duration::from_millis(timeout_ms),
            );
            // Count the pairs up front so the progress bar has a
            // known denominator before the first event lands.
            let total: usize = crate::benchmark::BENCHMARK_PAIRS
                .iter()
                .filter(|(c, _, _, _)| picked.map(|f| f == *c).unwrap_or(true))
                .count();
            app.bench_rx = Some(rx);
            app.bench_total = total;
            app.bench_done = 0;
            app.bench_scroll.clear();
            app.bench_skipped.clear();
            app.bench_stats = None;
            app.bench_started_at = Some(std::time::Instant::now());
            app.bench_chain_filter = picked;
            app.phase = Phase::Benchmarking;
            app.pending_action = None;
            app.chain_picker_include_all = false;
            app.status = "Benchmark in flight · Esc to cancel".into();
            return Ok(());
        }
        MenuAction::Doctor => {
            let Some(chain) = picked else {
                return Ok(());
            };
            // v1.2 Phase 6b: run doctor inside the TUI, streaming
            // per-check progress through an mpsc channel. Esc drops
            // the receiver and bounces back to the menu. The
            // sections vec builds up incrementally as items land.
            // Clone the config so we can override the chain
            // without mutating the persisted default.
            let mut cfg = config.clone();
            cfg.defaults.chain = chain.name().to_ascii_lowercase();
            // Resolve the RPC URL for the picked chain. If the
            // user passed an explicit --rpc, trust it; otherwise
            // fall back to the chain's public default so the
            // probes can run without further config.
            if effective_rpc.is_none() && cfg.defaults.rpc.is_empty() {
                cfg.defaults.rpc = chain.default_public_rpc().to_string();
            } else if let Some(url) = effective_rpc.as_ref() {
                cfg.defaults.rpc = url.clone();
            }
            let rx = doctor_stream::spawn(
                cfg,
                std::path::PathBuf::new(),
                chain,
            );
            app.doctor_rx = Some(rx);
            app.doctor_sections.clear();
            app.doctor_current_section = None;
            app.doctor_started_at = Some(std::time::Instant::now());
            app.doctor_total_elapsed_ms = 0;
            app.chain = chain;
            app.phase = Phase::Doctoring;
            app.pending_action = None;
            app.chain_picker_include_all = false;
            app.status = "Doctor running · Esc to cancel".into();
            return Ok(());
        }
        MenuAction::Tokens => {
            let Some(chain) = picked else {
                return Ok(());
            };
            // v1.2 Phase 2: load the token registry for the picked
            // chain and transition straight into the in-TUI list
            // view. No backend build, no gas tracker — Tokens is a
            // pure data browser.
            let tokens = dexquote_core::list_tokens(chain);
            if tokens.is_empty() {
                app.status = format!("No tokens bundled for {}", chain.name());
                app.phase = Phase::MainMenu;
                app.pending_action = None;
                app.chain_picker_include_all = false;
                return Ok(());
            }
            let count = tokens.len();
            app.chain = chain;
            app.tokens_view = tokens;
            app.tokens_cursor = 0;
            app.tokens_scroll = 0;
            app.phase = Phase::ShowingTokens;
            app.pending_action = None;
            app.chain_picker_include_all = false;
            app.status = format!(
                "{} · {count} tokens · ↑↓ PgUp/PgDn g/G · Esc back",
                chain.name()
            );
            return Ok(());
        }
        MenuAction::History | MenuAction::Quit => {
            // These bypass the chain picker entirely — shouldn't
            // reach this branch, but handle defensively.
            return Ok(());
        }
        MenuAction::Quote | MenuAction::Depth | MenuAction::Route => {
            // Chain is required for the form-based actions.
            let Some(chain) = picked else {
                return Ok(());
            };

            // Pick the RPC URL for this chain. If the user explicitly
            // passed `--rpc` at launch, trust them (it might be a
            // local fork). Otherwise, if they picked a chain that
            // differs from the default, swap in that chain's public
            // default so the user gets a working RPC without extra
            // configuration.
            let new_rpc = if effective_rpc.is_some() && chain == default_chain {
                effective_rpc.clone()
            } else if effective_rpc.is_some() {
                // Explicit --rpc for a different chain — honour it.
                effective_rpc.clone()
            } else {
                Some(chain.default_public_rpc().to_string())
            };

            let built = crate::build_backends(
                selection,
                chain,
                new_rpc.as_deref(),
                Duration::from_millis(timeout_ms),
            )
            .await?;

            if built.backends.is_empty() {
                return Err(CliError::setup(
                    "no backends selected for this chain".to_string(),
                    "check `backends.enabled` in config".to_string(),
                ));
            }

            // Tear down any old gas tracker and spawn a fresh one
            // for the new chain. Dropping the receivers cleanly
            // terminates the old spawn tasks at their next send.
            app.gas_rx = None;
            app.sol_gas_rx = None;
            app.gas_snapshot = None;
            app.sol_gas_snapshot = None;

            match chain {
                Chain::Solana => {
                    app.sol_gas_rx = Some(solana_gas_tracker::spawn());
                }
                _ => {
                    if let Some(provider) = &built.provider {
                        app.gas_rx = Some(gas_tracker::spawn(provider.clone(), chain));
                    }
                }
            }

            // Update the app's chain + mode, reset form state so the
            // user starts with blank fields, and transition to the
            // form.
            let mode = action.quote_mode();
            app.chain = chain;
            app.mode = mode;
            app.phase = Phase::EditingFields;
            app.focus = Field::Sell;
            app.pending_action = None;
            app.chain_picker_include_all = false;
            app.sell = None;
            app.buy = None;
            app.amount_input.clear();
            app.status = match action {
                MenuAction::Quote => {
                    "Tab to move · Enter to pick token or quote · Esc back to menu".into()
                }
                MenuAction::Depth => {
                    "Tab to move · Enter to pick · Find Quote runs Depth · Esc back to menu"
                        .into()
                }
                MenuAction::Route => {
                    "Tab to move · Enter to pick · Find Quote shows Route · Esc back to menu"
                        .into()
                }
                _ => String::new(),
            };

            // Stash the new context.
            *ctx = Some(TuiContext {
                chain,
                rpc_url: new_rpc.clone(),
                timeout_ms,
                backends: built.backends,
                provider: built.provider,
                mode,
            });
            *effective_rpc = new_rpc;
        }
    }
    Ok(())
}

async fn run_inner(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    _config: &Config,
    ctx: TuiContext,
) -> CliResult<TuiOutcome> {
    let mut app = App::with_mode(ctx.chain, ctx.mode);

    // Spawn the chain-appropriate live gas tracker. EVM chains use
    // `eth_gasPrice` + Chainlink via the shared alloy provider; Solana
    // uses Pyth SOL/USD + Solana RPC `getRecentPrioritizationFees`
    // with its own HTTP client. Each runs for the lifetime of the TUI
    // and exits cleanly when the receiver is dropped on function return.
    match ctx.chain {
        Chain::Solana => {
            app.sol_gas_rx = Some(solana_gas_tracker::spawn());
        }
        _ => {
            if let Some(provider) = &ctx.provider {
                app.gas_rx = Some(gas_tracker::spawn(provider.clone(), ctx.chain));
            }
        }
    }

    loop {
        // Pull any finished backend results off the mpsc channel BEFORE
        // drawing so per-backend rows update the same frame they arrive on.
        app.drain_pending();
        app.drain_gas();

        terminal
            .draw(|frame| ui::draw(frame, &app))
            .map_err(|e| CliError::bug(format!("tui draw failed: {e}")))?;

        // While a quote is in flight, advance the spinner frame each draw
        // so the "fetching..." rows actually animate.
        if matches!(app.phase, Phase::Quoting) {
            app.spinner_frame = app.spinner_frame.wrapping_add(1);
        }

        // Shorter poll interval while quoting so the spinner looks smooth
        // and results propagate within ~40ms of landing in the channel.
        let poll_timeout = if matches!(app.phase, Phase::Quoting) {
            Duration::from_millis(40)
        } else {
            Duration::from_millis(80)
        };

        if xt_event::poll(poll_timeout)
            .map_err(|e| CliError::bug(format!("tui event poll failed: {e}")))?
        {
            if let Event::Key(key) = xt_event::read()
                .map_err(|e| CliError::bug(format!("tui event read failed: {e}")))?
            {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                let intent = event::map_key(key, &app);
                let should_quote = app.handle(intent);

                if app.should_quit {
                    return Ok(TuiOutcome::Exit);
                }

                if should_quote {
                    start_quote(&mut app, &ctx);
                }
            }
        }
    }
}

/// Kick off a new quote. Non-blocking — spawns one tokio task per backend
/// into an mpsc channel, stashes the receiver on the app, and returns
/// immediately so the event loop can draw the pending state. The actual
/// result population happens in `app.drain_pending()` on subsequent ticks.
///
/// **Mode dispatch:** in Depth / Route mode this function doesn't
/// actually start a quote — it stashes a `DeferredAction` on the app
/// and sets `should_quit`, so the event loop tears down cleanly and
/// returns the deferred outcome to the dispatcher. The dispatcher
/// then runs `depth::run` or `route::run` with the collected inputs
/// against the restored terminal.
fn start_quote(app: &mut App, ctx: &TuiContext) {
    let Some(sell) = app.sell.clone() else {
        app.status = "Pick a sell token first".into();
        return;
    };
    let Some(buy) = app.buy.clone() else {
        app.status = "Pick a buy token first".into();
        return;
    };

    // Parse the amount here so Depth/Route modes fail the same way
    // Normal mode does — invalid input stays in the form instead of
    // exiting the TUI.
    let amount_in = match dexquote_core::token::parse_amount(&app.amount_input, sell.decimals) {
        Ok(v) => v,
        Err(e) => {
            app.status = format!("Invalid amount: {e}");
            app.phase = Phase::EditingFields;
            app.focus = Field::Amount;
            return;
        }
    };

    // Route (v1.2 Phase 3): runs the same quote_all as Normal but
    // `drain_pending` flips to `Phase::ShowingRoute` on completion
    // so the UI renders with the route-path column.
    //
    // Depth (v1.2 Phase 4): spawns `depth_stream::spawn` which
    // runs 5 sequential quote_all rounds on scaled notionals and
    // streams `DepthProgress` events. Transitions to
    // `Phase::Depthing` — `drain_depth` pulls events, the UI
    // renders the progress table, and `Finished` flips to
    // `Phase::ShowingDepth`.
    if matches!(app.mode, QuoteMode::Depth) {
        let request = QuoteRequest {
            chain: ctx.chain,
            token_in: sell.clone(),
            token_out: buy.clone(),
            amount_in,
            block_id: None,
        };
        let rx = depth_stream::spawn(
            ctx.backends.clone(),
            request,
            app.amount_input.clone(),
            Duration::from_millis(ctx.timeout_ms),
        );
        // Initialize the per-level slot buffer so `drain_depth`
        // can assign each `LevelDone` into its index.
        app.depth_levels = (0..crate::depth::NOTIONALS.len()).map(|_| None).collect();
        app.depth_current = 0;
        app.depth_report = None;
        app.depth_rx = Some(rx);
        app.depth_started_at = Some(std::time::Instant::now());
        app.depth_base_amount_human = app.amount_input.clone();
        app.depth_sell_symbol = sell.symbol.clone();
        app.depth_buy_symbol = buy.symbol.clone();
        app.phase = Phase::Depthing;
        app.status = format!(
            "Depth sweep 1/{} · Esc to cancel",
            crate::depth::NOTIONALS.len()
        );
        return;
    }

    let request = QuoteRequest {
        chain: ctx.chain,
        token_in: sell,
        token_out: buy,
        amount_in,
        block_id: None,
    };

    let rx = quote_stream::spawn(
        ctx.backends.clone(),
        request.clone(),
        Duration::from_millis(ctx.timeout_ms),
    );

    let backend_names: Vec<&'static str> =
        ctx.backends.iter().map(|b| b.name()).collect();
    // `BackendResult` doesn't implement Clone (the inner Box<dyn Error> in
    // the Rpc variant isn't Clone), so `vec![None; n]` won't compile.
    let received: Vec<Option<dexquote_core::BackendResult>> =
        std::iter::repeat_with(|| None).take(ctx.backends.len()).collect();

    app.pending = Some(PendingQuote {
        request,
        backend_names,
        received,
        started_at: std::time::Instant::now(),
        rx,
    });
    app.phase = Phase::Quoting;
    app.results = None;
    app.spinner_frame = 0;
    app.status = format!(
        "Fetching {} backends · Esc to cancel",
        ctx.backends.len()
    );
}

fn setup_terminal() -> CliResult<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()
        .map_err(|e| CliError::bug(format!("failed to enable raw mode: {e}")))?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen)
        .map_err(|e| CliError::bug(format!("failed to enter alternate screen: {e}")))?;
    let backend = CrosstermBackend::new(out);
    Terminal::new(backend)
        .map_err(|e| CliError::bug(format!("failed to build terminal: {e}")))
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
