//! App state for the interactive TUI.
//!
//! The app is a state machine driven by the `Phase` enum. v1.2 added
//! `MainMenu` and `ChainPicker` as first-class in-TUI phases — the
//! user lands in `MainMenu` on launch, picks an action, picks a chain,
//! then transitions into the action-specific phase. `Esc` from any
//! action phase returns to `MainMenu`; `Esc` from `MainMenu` quits.
//!
//! Existing phases (kept from v1.1):
//!   - `EditingFields`: user is filling in sell/buy/amount. Arrow keys move
//!     focus between fields; pressing Enter on a token field opens the
//!     picker; pressing Enter on the amount field triggers a quote.
//!   - `TokenPicker`: fuzzy-searchable list overlay for the currently
//!     focused token field.
//!   - `CustomAddressEntry`: raw 0x… address entry for tokens not in
//!     the bundled registry.
//!   - `Quoting` / `ShowingResults`: in-flight and completed quote
//!     states; results render in the body panel.

use crate::tui::event::Intent;
use crate::tui::gas_tracker::GasSnapshot;
use crate::tui::quote_stream::PendingRx;
use crate::tui::solana_gas_tracker::SolanaGasSnapshot;
use dexquote_core::{list_tokens, BackendResult, Chain, QuoteRequest, Token};
use tokio::sync::mpsc;
use nucleo_matcher::{
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
    Matcher,
};
use std::time::Instant;

/// What the user wants dexquote to do. Selected in `Phase::MainMenu`;
/// stashed as `app.pending_action` while the chain picker runs, then
/// dispatched once the chain is known.
///
/// v1.1 moved this out of a standalone `tui::main_menu` module; v1.2
/// folds it into the main App state alongside the menu cursor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    Quote,
    Depth,
    Route,
    Benchmark,
    Doctor,
    Tokens,
    History,
    Quit,
}

impl MenuAction {
    pub const ALL: &'static [MenuAction] = &[
        MenuAction::Quote,
        MenuAction::Depth,
        MenuAction::Route,
        MenuAction::Benchmark,
        MenuAction::Doctor,
        MenuAction::Tokens,
        MenuAction::History,
        MenuAction::Quit,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MenuAction::Quote => "Quote",
            MenuAction::Depth => "Depth",
            MenuAction::Route => "Route",
            MenuAction::Benchmark => "Benchmark",
            MenuAction::Doctor => "Doctor",
            MenuAction::Tokens => "Tokens",
            MenuAction::History => "History",
            MenuAction::Quit => "Quit",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            MenuAction::Quote => "interactive quote builder",
            MenuAction::Depth => "price impact across 5 notionals",
            MenuAction::Route => "multi-hop path each backend took",
            MenuAction::Benchmark => "per-backend leaderboard",
            MenuAction::Doctor => "self-test RPC + every backend",
            MenuAction::Tokens => "browse the bundled registry",
            MenuAction::History => "list recent quotes",
            MenuAction::Quit => "exit dexquote",
        }
    }

    /// Which `QuoteMode` does the quote-form land in for this action?
    /// Only meaningful for `Quote` / `Depth` / `Route` — the others
    /// bypass the form entirely.
    pub fn quote_mode(self) -> QuoteMode {
        match self {
            MenuAction::Quote => QuoteMode::Normal,
            MenuAction::Depth => QuoteMode::Depth,
            MenuAction::Route => QuoteMode::Route,
            _ => QuoteMode::Normal,
        }
    }

    /// Does this action's chain picker include the "All chains" row?
    pub fn picker_includes_all(self) -> bool {
        matches!(self, MenuAction::Benchmark)
    }
}

/// In-flight quote state. One row per backend: `Some` once the backend
/// has reported, `None` while it's still fetching. The `backend_names`
/// vector is captured at spawn-time so the UI can render the row labels
/// without touching the (moved) backend trait objects.
pub struct PendingQuote {
    pub request: QuoteRequest,
    pub backend_names: Vec<&'static str>,
    pub received: Vec<Option<BackendResult>>,
    pub started_at: Instant,
    pub rx: PendingRx,
}

impl PendingQuote {
    pub fn is_done(&self) -> bool {
        self.received.iter().all(|s| s.is_some())
    }

    pub fn progress(&self) -> (usize, usize) {
        let done = self.received.iter().filter(|s| s.is_some()).count();
        (done, self.received.len())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Sell,
    Buy,
    Amount,
    FindQuote,
}

/// Discriminator for the shared list-view handler used by
/// `Phase::ShowingTokens` and `Phase::ShowingHistory`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListView {
    Tokens,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// v1.2 landing phase. User picks an action from the menu. Esc
    /// here quits the TUI — it's the only exit point.
    MainMenu,
    /// v1.2 chain selection after an action is picked. Esc here
    /// returns to `MainMenu`.
    ChainPicker,
    EditingFields,
    TokenPicker,
    CustomAddressEntry,
    Quoting,
    ShowingResults,
    /// v1.2 Route results — same data as `ShowingResults` but
    /// rendered with the multi-hop path column instead of the
    /// gas/net columns. Transitioned to from `Quoting` when the
    /// active mode is `QuoteMode::Route`.
    ShowingRoute,
    /// v1.2 Depth sweep in flight — 5 sequential quote_all rounds,
    /// one per notional. Each completed level lands in
    /// `app.depth_levels[idx]` and the UI renders with a spinner on
    /// the current level.
    Depthing,
    /// v1.2 Depth sweep complete — full price-impact table rendered
    /// from `app.depth_report`.
    ShowingDepth,
    /// v1.2 Benchmark sweep in flight — iterates the hardcoded pair
    /// set, emitting progress events. UI shows an inline progress
    /// bar per chain plus a scrollable "recent pairs" buffer.
    Benchmarking,
    /// v1.2 Benchmark complete — per-backend leaderboard rendered
    /// from `app.benchmark_stats`.
    ShowingBenchmark,
    /// v1.2 Doctor self-test in flight — streams check results
    /// into `app.doctor_sections` as they land.
    Doctoring,
    /// v1.2 Doctor complete — full section/item report rendered
    /// from `app.doctor_sections`.
    ShowingDoctor,
    /// v1.2 Tokens action — scrollable list of the bundled registry
    /// for the chain the user picked. Esc returns to `MainMenu`.
    ShowingTokens,
    /// v1.2 History action — scrollable list of recent quotes from
    /// the JSONL log, newest first. Chain-global. Esc returns to
    /// `MainMenu`.
    ShowingHistory,
}

/// What the Quote TUI should DO when the user presses Find Quote.
/// Set at TUI launch based on which action the user picked from the
/// main menu. `Normal` is the v1.0 streaming quote; `Depth` / `Route`
/// stash a deferred action and exit so the main dispatch layer can
/// run the corresponding subcommand with the collected inputs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuoteMode {
    Normal,
    Depth,
    Route,
}

pub struct App {
    pub chain: Chain,
    pub focus: Field,
    pub phase: Phase,
    /// What the Quote TUI should do on Find Quote. Set from the main
    /// menu selection + chain picker.
    pub mode: QuoteMode,

    // v1.2 main menu state
    pub menu_cursor: usize,
    /// Set when the user selects an action from the main menu and
    /// transitions to `ChainPicker`. Consumed once the chain is picked.
    pub pending_action: Option<MenuAction>,

    // v1.2 chain picker state. `include_all` is set when the pending
    // action is `Benchmark` — its picker gets an extra "All chains"
    // row at the top.
    pub chain_picker_cursor: usize,
    pub chain_picker_include_all: bool,

    pub sell: Option<Token>,
    pub buy: Option<Token>,
    pub amount_input: String,

    // Picker state
    pub picker_filter: String,
    pub picker_cursor: usize,
    pub picker_source: Vec<Token>,
    pub picker_target: Field,

    // Custom address state
    pub custom_address_input: String,
    pub custom_address_error: Option<String>,
    /// v1.2: async token resolve in flight. The custom-address
    /// Activate handler spawns a tokio task that calls
    /// `Token::resolve` and posts the result (or error) here.
    /// `drain_custom_token` picks it up on the next tick.
    pub custom_token_rx:
        Option<tokio::sync::oneshot::Receiver<Result<Token, String>>>,
    /// Which field the custom token should land in once resolved.
    pub custom_token_target: Field,

    // In-flight quote state (Phase::Quoting). `None` in all other phases.
    pub pending: Option<PendingQuote>,
    /// Monotonically-incremented on every draw while a quote is in flight.
    /// Used to pick which spinner frame to render.
    pub spinner_frame: usize,

    // Live gas tracker state — one pair per chain family. The EVM
    // tracker polls `eth_gasPrice` + Chainlink ETH/USD; the Solana
    // tracker polls Pyth + `getRecentPrioritizationFees`. Only one is
    // populated at a time (the chain-native one). The sibling pair is
    // `None` and gets drained as a no-op.
    pub gas_rx: Option<mpsc::Receiver<GasSnapshot>>,
    pub gas_snapshot: Option<GasSnapshot>,
    pub sol_gas_rx: Option<mpsc::Receiver<SolanaGasSnapshot>>,
    pub sol_gas_snapshot: Option<SolanaGasSnapshot>,

    /// When true, the help overlay is drawn over everything else. Dismissed
    /// by pressing `?` again or any navigation key.
    pub show_help: bool,

    // Results state
    pub results: Option<Vec<BackendResult>>,
    pub last_request: Option<QuoteRequest>,
    pub total_elapsed_ms: u128,

    // v1.2 tokens view state: scrollable list of the bundled token
    // registry for `app.chain`. Populated when the user picks Tokens
    // from the main menu and a chain from the picker.
    pub tokens_view: Vec<Token>,
    pub tokens_cursor: usize,
    pub tokens_scroll: usize,

    // v1.2 history view state: scrollable list of `HistoryEntry`s
    // from the JSONL log, newest-first. No chain dimension — the
    // log is global.
    pub history_view: Vec<crate::history::HistoryEntry>,
    pub history_cursor: usize,
    pub history_scroll: usize,

    // v1.2 depth sweep state. `depth_rx` is the receiver end of
    // the `depth_stream::spawn` channel; dropping it orphans the
    // spawned task cleanly. `depth_levels` is a slot-per-level
    // buffer that fills in as `LevelDone` events land. `depth_current`
    // tracks the row that should render a spinner. `depth_report`
    // is only set once the sweep is fully finished — the UI uses
    // its presence to decide whether to render the progress view
    // or the final table.
    pub depth_rx: Option<crate::tui::depth_stream::DepthRx>,
    pub depth_levels: Vec<Option<crate::render::depth::DepthLevel>>,
    pub depth_current: usize,
    pub depth_report: Option<crate::render::depth::DepthReport>,
    pub depth_started_at: Option<Instant>,
    pub depth_base_amount_human: String,
    pub depth_sell_symbol: String,
    pub depth_buy_symbol: String,

    // v1.2 benchmark sweep state. `bench_rx` is the receiver end
    // of `benchmark_stream::spawn`; dropping it orphans the task
    // cleanly (in-flight backends bounded by per-backend timeout).
    // `bench_scroll` is a ring buffer of the most recent N
    // completed pairs for the scroll-buffer UI. `bench_stats`
    // is only set once the sweep finishes.
    pub bench_rx: Option<crate::tui::benchmark_stream::BenchmarkRx>,
    pub bench_total: usize,
    pub bench_done: usize,
    pub bench_current_chain: Option<Chain>,
    pub bench_current_pair: Option<(String, String)>,
    pub bench_scroll: std::collections::VecDeque<crate::tui::benchmark_stream::PairSummary>,
    pub bench_skipped: Vec<(Chain, String)>,
    pub bench_stats: Option<crate::benchmark::BenchmarkStats>,
    pub bench_started_at: Option<Instant>,
    pub bench_chain_filter: Option<Chain>,

    // v1.2 doctor state. Sections build up progressively as the
    // stream emits items; complete on the `Finished` event.
    pub doctor_rx: Option<crate::tui::doctor_stream::DoctorRx>,
    pub doctor_sections: Vec<crate::doctor::DoctorSection>,
    pub doctor_current_section: Option<&'static str>,
    pub doctor_started_at: Option<Instant>,
    pub doctor_total_elapsed_ms: u128,

    // Status line shown under the form
    pub status: String,
    pub should_quit: bool,

    /// v1.2: set by the ChainPicker's Activate handler. The main
    /// event loop reads this after `handle()` returns, reads
    /// `pending_action` + `chain_picker_selected()`, rebuilds the
    /// `TuiContext` for the new chain (or sets a deferred action for
    /// non-Quote flows), and transitions the phase accordingly.
    pub pending_transition: bool,
}

impl App {
    /// v1.2 default constructor: lands in `Phase::MainMenu`. The
    /// `chain` is a placeholder — the real chain is chosen by the
    /// user in `ChainPicker` before any action runs. Calling code
    /// that wants to skip the menu (explicit `--chain` flag) should
    /// use `with_mode` instead, which lands directly in the quote
    /// form.
    pub fn new(default_chain: Chain) -> Self {
        Self {
            chain: default_chain,
            focus: Field::Sell,
            phase: Phase::MainMenu,
            mode: QuoteMode::Normal,
            menu_cursor: 0,
            pending_action: None,
            chain_picker_cursor: Chain::ALL
                .iter()
                .position(|&c| c == default_chain)
                .unwrap_or(0),
            chain_picker_include_all: false,
            sell: None,
            buy: None,
            amount_input: String::new(),
            picker_filter: String::new(),
            picker_cursor: 0,
            picker_source: Vec::new(),
            picker_target: Field::Sell,
            custom_address_input: String::new(),
            custom_address_error: None,
            custom_token_rx: None,
            custom_token_target: Field::Sell,
            pending: None,
            spinner_frame: 0,
            gas_rx: None,
            gas_snapshot: None,
            sol_gas_rx: None,
            sol_gas_snapshot: None,
            show_help: false,
            results: None,
            last_request: None,
            total_elapsed_ms: 0,
            tokens_view: Vec::new(),
            tokens_cursor: 0,
            tokens_scroll: 0,
            history_view: Vec::new(),
            history_cursor: 0,
            history_scroll: 0,
            depth_rx: None,
            depth_levels: Vec::new(),
            depth_current: 0,
            depth_report: None,
            depth_started_at: None,
            depth_base_amount_human: String::new(),
            depth_sell_symbol: String::new(),
            depth_buy_symbol: String::new(),
            bench_rx: None,
            bench_total: 0,
            bench_done: 0,
            bench_current_chain: None,
            bench_current_pair: None,
            bench_scroll: std::collections::VecDeque::new(),
            bench_skipped: Vec::new(),
            bench_stats: None,
            bench_started_at: None,
            bench_chain_filter: None,
            doctor_rx: None,
            doctor_sections: Vec::new(),
            doctor_current_section: None,
            doctor_started_at: None,
            doctor_total_elapsed_ms: 0,
            status: "↑↓ navigate · Enter select · 1-8 jump · Esc quit".into(),
            should_quit: false,
            pending_transition: false,
        }
    }

    /// Legacy constructor used by the "explicit `--chain`" path that
    /// skips the menu entirely and lands directly in the quote form.
    /// Kept for back-compat with `dexquote --chain ethereum`.
    pub fn with_mode(chain: Chain, mode: QuoteMode) -> Self {
        Self {
            chain,
            focus: Field::Sell,
            phase: Phase::EditingFields,
            mode,
            menu_cursor: 0,
            pending_action: None,
            chain_picker_cursor: 0,
            chain_picker_include_all: false,
            sell: None,
            buy: None,
            amount_input: String::new(),
            picker_filter: String::new(),
            picker_cursor: 0,
            picker_source: Vec::new(),
            picker_target: Field::Sell,
            custom_address_input: String::new(),
            custom_address_error: None,
            custom_token_rx: None,
            custom_token_target: Field::Sell,
            pending: None,
            spinner_frame: 0,
            gas_rx: None,
            gas_snapshot: None,
            sol_gas_rx: None,
            sol_gas_snapshot: None,
            show_help: false,
            results: None,
            last_request: None,
            total_elapsed_ms: 0,
            tokens_view: Vec::new(),
            tokens_cursor: 0,
            tokens_scroll: 0,
            history_view: Vec::new(),
            history_cursor: 0,
            history_scroll: 0,
            depth_rx: None,
            depth_levels: Vec::new(),
            depth_current: 0,
            depth_report: None,
            depth_started_at: None,
            depth_base_amount_human: String::new(),
            depth_sell_symbol: String::new(),
            depth_buy_symbol: String::new(),
            bench_rx: None,
            bench_total: 0,
            bench_done: 0,
            bench_current_chain: None,
            bench_current_pair: None,
            bench_scroll: std::collections::VecDeque::new(),
            bench_skipped: Vec::new(),
            bench_stats: None,
            bench_started_at: None,
            bench_chain_filter: None,
            doctor_rx: None,
            doctor_sections: Vec::new(),
            doctor_current_section: None,
            doctor_started_at: None,
            doctor_total_elapsed_ms: 0,
            status: "Tab to move between fields · Enter to pick a token or quote · Esc to quit"
                .into(),
            should_quit: false,
            pending_transition: false,
        }
    }

    /// Reset all action-specific state and return to the main menu.
    /// Called when the user presses Esc from an action phase, or
    /// when an action completes and wants to offer another action.
    ///
    /// Clears the quote form, the pending quote channel, the results,
    /// and the deferred action. Does NOT touch the gas tracker or
    /// the chain field — those stay valid across menu round trips.
    pub fn return_to_menu(&mut self) {
        self.phase = Phase::MainMenu;
        self.focus = Field::Sell;
        self.pending_action = None;
        self.sell = None;
        self.buy = None;
        self.amount_input.clear();
        self.picker_filter.clear();
        self.picker_cursor = 0;
        self.picker_source.clear();
        self.custom_address_input.clear();
        self.custom_address_error = None;
        self.custom_token_rx = None;
        self.pending = None;
        self.results = None;
        self.last_request = None;
        self.total_elapsed_ms = 0;
        self.spinner_frame = 0;
        self.mode = QuoteMode::Normal;
        self.pending_transition = false;
        self.tokens_view.clear();
        self.tokens_cursor = 0;
        self.tokens_scroll = 0;
        self.history_view.clear();
        self.history_cursor = 0;
        self.history_scroll = 0;
        self.depth_rx = None;
        self.depth_levels.clear();
        self.depth_current = 0;
        self.depth_report = None;
        self.depth_started_at = None;
        self.depth_base_amount_human.clear();
        self.depth_sell_symbol.clear();
        self.depth_buy_symbol.clear();
        self.bench_rx = None;
        self.bench_total = 0;
        self.bench_done = 0;
        self.bench_current_chain = None;
        self.bench_current_pair = None;
        self.bench_scroll.clear();
        self.bench_skipped.clear();
        self.bench_stats = None;
        self.bench_started_at = None;
        self.bench_chain_filter = None;
        self.doctor_rx = None;
        self.doctor_sections.clear();
        self.doctor_current_section = None;
        self.doctor_started_at = None;
        self.doctor_total_elapsed_ms = 0;
        self.status = "↑↓ navigate · Enter select · 1-8 jump · Esc quit".into();
    }

    /// Check whether the async custom-token resolve has completed.
    /// If so, stash the result as the sell or buy token and update
    /// the status line. Non-blocking — oneshot::try_recv returns
    /// immediately.
    pub fn drain_custom_token(&mut self) {
        let Some(rx) = self.custom_token_rx.as_mut() else {
            return;
        };
        match rx.try_recv() {
            Ok(Ok(token)) => {
                let sym = token.symbol.clone();
                match self.custom_token_target {
                    Field::Sell => self.sell = Some(token),
                    Field::Buy => self.buy = Some(token),
                    _ => {}
                }
                self.custom_token_rx = None;
                self.status = format!("Resolved: {sym}");
                self.advance_focus();
            }
            Ok(Err(e)) => {
                self.custom_token_rx = None;
                self.status = format!("Token resolve failed: {e}");
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {
                // Still in flight — nothing to do yet.
            }
            Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                self.custom_token_rx = None;
                self.status = "Token resolve: task dropped unexpectedly".into();
            }
        }
    }

    /// Drain whichever live gas tracker channel is active. Keeps the
    /// most recent snapshot of whichever flavour is flowing — EVM or
    /// Solana — so the header strip renders fresh data. Cheap no-op
    /// when both receivers are `None`.
    pub fn drain_gas(&mut self) {
        if let Some(rx) = self.gas_rx.as_mut() {
            while let Ok(snap) = rx.try_recv() {
                self.gas_snapshot = Some(snap);
            }
        }
        if let Some(rx) = self.sol_gas_rx.as_mut() {
            while let Ok(snap) = rx.try_recv() {
                self.sol_gas_snapshot = Some(snap);
            }
        }
    }

    /// Pull every ready message off the pending channel into the per-backend
    /// slots. Called once per event loop iteration, right before drawing.
    /// When every slot is filled (or the channel has closed with holes, which
    /// means some task was dropped), finalise the quote and transition to
    /// `ShowingResults`.
    pub fn drain_pending(&mut self) {
        let Some(pending) = self.pending.as_mut() else {
            return;
        };

        while let Ok((idx, result)) = pending.rx.try_recv() {
            if let Some(slot) = pending.received.get_mut(idx) {
                *slot = Some(result);
            }
        }

        // If every sender hung up but we still have empty slots (e.g. a
        // spawned task panicked before emitting), treat the missing rows
        // as generic errors and finalise anyway — better than waiting
        // forever.
        let rx_closed = pending.rx.is_closed();
        if rx_closed && !pending.is_done() {
            for (i, slot) in pending.received.iter_mut().enumerate() {
                if slot.is_none() {
                    let name = pending.backend_names.get(i).copied().unwrap_or("?");
                    *slot = Some(BackendResult {
                        name,
                        quote: Err(dexquote_core::DexQuoteError::decode(
                            name,
                            "backend task dropped before sending a result",
                        )),
                    });
                }
            }
        }

        if pending.is_done() {
            let pending = self.pending.take().expect("checked above");
            let total_elapsed_ms = pending.started_at.elapsed().as_millis();
            let results: Vec<BackendResult> =
                pending.received.into_iter().flatten().collect();
            self.results = Some(results);
            self.last_request = Some(pending.request);
            self.total_elapsed_ms = total_elapsed_ms;
            self.phase = match self.mode {
                QuoteMode::Route => Phase::ShowingRoute,
                _ => Phase::ShowingResults,
            };
            self.focus = Field::FindQuote;
            self.status = match self.mode {
                QuoteMode::Route => {
                    "Route complete · R re-run · Tab edit · Esc return to menu".into()
                }
                _ => "Quote complete · R re-run · Tab edit · Esc return to menu".into(),
            };
        }
    }

    /// Apply an intent to the state. Returns `true` when the caller should
    /// kick off an actual quote (i.e. all fields are valid and the user
    /// pressed Enter on the amount field).
    pub fn handle(&mut self, intent: Intent) -> bool {
        match self.phase {
            Phase::MainMenu => {
                self.handle_main_menu(intent);
                false
            }
            Phase::ChainPicker => {
                self.handle_chain_picker(intent);
                false
            }
            Phase::EditingFields | Phase::ShowingResults | Phase::ShowingRoute => {
                self.handle_editing(intent)
            }
            Phase::TokenPicker => {
                self.handle_picker(intent);
                false
            }
            Phase::CustomAddressEntry => {
                self.handle_custom_address(intent);
                false
            }
            Phase::Quoting => {
                self.handle_quoting(intent);
                false
            }
            Phase::ShowingTokens => {
                self.handle_list_view(intent, ListView::Tokens);
                false
            }
            Phase::ShowingHistory => {
                self.handle_list_view(intent, ListView::History);
                false
            }
            Phase::Depthing => {
                self.handle_depthing(intent);
                false
            }
            Phase::ShowingDepth => {
                self.handle_showing_depth(intent);
                false
            }
            Phase::Benchmarking => {
                self.handle_benchmarking(intent);
                false
            }
            Phase::ShowingBenchmark => {
                self.handle_showing_benchmark(intent);
                false
            }
            Phase::Doctoring => {
                self.handle_doctoring(intent);
                false
            }
            Phase::ShowingDoctor => {
                self.handle_showing_doctor(intent);
                false
            }
        }
    }

    /// Key handler for the in-flight doctor self-test.
    fn handle_doctoring(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.doctor_rx = None;
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Key handler for the completed doctor report.
    fn handle_showing_doctor(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Drain ready doctor progress events. Builds up the sections
    /// vec progressively; transitions to `ShowingDoctor` on Finished.
    pub fn drain_doctor(&mut self) {
        if self.doctor_rx.is_none() {
            return;
        }
        let mut events: Vec<crate::tui::doctor_stream::DoctorProgress> = Vec::new();
        if let Some(rx) = self.doctor_rx.as_mut() {
            while let Ok(progress) = rx.try_recv() {
                events.push(progress);
            }
        }
        for progress in events {
            use crate::tui::doctor_stream::DoctorProgress as DP;
            match progress {
                DP::SectionStarted { name } => {
                    self.doctor_current_section = Some(name);
                }
                DP::Item { section, item } => {
                    crate::tui::doctor_stream::apply_item(
                        &mut self.doctor_sections,
                        section,
                        item,
                    );
                }
                DP::Finished { total_elapsed_ms } => {
                    self.doctor_total_elapsed_ms = total_elapsed_ms;
                    self.phase = Phase::ShowingDoctor;
                    self.status = "Doctor complete · Esc return to menu".into();
                    self.doctor_rx = None;
                    self.doctor_current_section = None;
                }
            }
        }
    }

    /// Key handler for the in-flight benchmark sweep. Esc drops the
    /// receiver (orphaning in-flight work) and returns to menu.
    fn handle_benchmarking(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.bench_rx = None;
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Key handler for the completed benchmark leaderboard.
    fn handle_showing_benchmark(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Drain ready benchmark progress events. Updates `bench_done`,
    /// `bench_current_*`, appends to the scroll buffer, and on
    /// `Finished` transitions to `ShowingBenchmark`.
    pub fn drain_benchmark(&mut self) {
        if self.bench_rx.is_none() {
            return;
        }
        let mut events: Vec<crate::tui::benchmark_stream::BenchmarkProgress> = Vec::new();
        if let Some(rx) = self.bench_rx.as_mut() {
            while let Ok(progress) = rx.try_recv() {
                events.push(progress);
            }
        }
        for progress in events {
            use crate::tui::benchmark_stream::BenchmarkProgress as BP;
            match progress {
                BP::PairStarted {
                    chain,
                    sell,
                    buy,
                    idx,
                    total,
                    ..
                } => {
                    self.bench_current_chain = Some(chain);
                    self.bench_current_pair = Some((sell, buy));
                    self.bench_total = total;
                    self.bench_done = idx;
                }
                BP::PairDone { summary } => {
                    // Cap the scroll buffer at 14 rows — newer at
                    // the bottom, older drop off the top.
                    if self.bench_scroll.len() >= 14 {
                        self.bench_scroll.pop_front();
                    }
                    self.bench_scroll.push_back(summary);
                    self.bench_done = self.bench_done.saturating_add(1);
                }
                BP::ChainSkipped { chain, reason } => {
                    self.bench_skipped.push((chain, reason));
                }
                BP::Finished { stats } => {
                    self.bench_stats = Some(stats);
                    self.phase = Phase::ShowingBenchmark;
                    self.status = "Benchmark complete · Esc return to menu".into();
                    self.bench_rx = None;
                    self.bench_current_chain = None;
                    self.bench_current_pair = None;
                }
            }
        }
    }

    /// Key handler for the in-flight depth sweep. Only Esc does
    /// anything — it drops the receiver, orphans the spawned task
    /// (bounded by per-backend timeout), and bounces back to the
    /// main menu.
    fn handle_depthing(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.depth_rx = None;
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Key handler for the completed depth view. Esc returns to
    /// menu; R re-runs with the same inputs.
    fn handle_showing_depth(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.return_to_menu();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            _ => {}
        }
    }

    /// Drain all ready progress events off the depth receiver into
    /// the per-level slots. Called once per event loop tick, right
    /// before drawing. When the `Finished` event lands, stash the
    /// report and transition to `Phase::ShowingDepth`.
    pub fn drain_depth(&mut self) {
        if self.depth_rx.is_none() {
            return;
        }
        // Collect events into a staging vec so we can finish the
        // `depth_rx` borrow before mutating other fields.
        let mut events: Vec<crate::tui::depth_stream::DepthProgress> = Vec::new();
        if let Some(rx) = self.depth_rx.as_mut() {
            while let Ok(progress) = rx.try_recv() {
                events.push(progress);
            }
        }
        for progress in events {
            match progress {
                crate::tui::depth_stream::DepthProgress::LevelStarted { idx } => {
                    self.depth_current = idx;
                }
                crate::tui::depth_stream::DepthProgress::LevelDone { idx, row } => {
                    if let Some(slot) = self.depth_levels.get_mut(idx) {
                        *slot = Some(row);
                    }
                }
                crate::tui::depth_stream::DepthProgress::Finished { report } => {
                    self.depth_report = Some(report);
                    self.phase = Phase::ShowingDepth;
                    self.status = "Depth complete · Esc return to menu".into();
                    self.depth_rx = None;
                }
            }
        }
    }

    /// Shared scrollable-list handler used by `ShowingTokens` and
    /// `ShowingHistory`. Arrow keys / j/k move the cursor, PgUp/PgDn
    /// page, Home/End jump to the ends, Esc returns to menu.
    fn handle_list_view(&mut self, intent: Intent, view: ListView) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        // Handle self-mutating intents first so there's no conflict
        // with the &mut borrow on the cursor below.
        match intent {
            Intent::Quit => {
                self.return_to_menu();
                return;
            }
            Intent::ToggleHelp => {
                self.show_help = true;
                return;
            }
            _ => {}
        }
        let total = match view {
            ListView::Tokens => self.tokens_view.len(),
            ListView::History => self.history_view.len(),
        };
        if total == 0 {
            return;
        }

        let cursor = match view {
            ListView::Tokens => &mut self.tokens_cursor,
            ListView::History => &mut self.history_cursor,
        };

        match intent {
            Intent::Up | Intent::Prev => {
                *cursor = cursor.saturating_sub(1);
            }
            Intent::Down | Intent::Next => {
                if *cursor + 1 < total {
                    *cursor += 1;
                }
            }
            Intent::PageUp => {
                *cursor = cursor.saturating_sub(10);
            }
            Intent::PageDown => {
                *cursor = (*cursor + 10).min(total.saturating_sub(1));
            }
            Intent::Home => {
                *cursor = 0;
            }
            Intent::End => {
                *cursor = total.saturating_sub(1);
            }
            _ => {}
        }
    }

    /// Main menu key handler. Arrow keys move the cursor, Enter
    /// selects the current row, digits 1-8 jump directly to an
    /// action, Esc/q quits the TUI entirely (this is the only exit
    /// point under v1.2).
    fn handle_main_menu(&mut self, intent: Intent) {
        // Help overlay can be toggled from the menu too.
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        match intent {
            Intent::Quit => {
                self.should_quit = true;
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            Intent::Up | Intent::Prev => {
                self.menu_cursor = self.menu_cursor.saturating_sub(1);
            }
            Intent::Down | Intent::Next => {
                if self.menu_cursor + 1 < MenuAction::ALL.len() {
                    self.menu_cursor += 1;
                }
            }
            Intent::Activate => {
                self.select_menu_action(MenuAction::ALL[self.menu_cursor]);
            }
            Intent::Character(c) if c.is_ascii_digit() => {
                let idx = c.to_digit(10).unwrap_or(0) as usize;
                if idx >= 1 && idx <= MenuAction::ALL.len() {
                    self.menu_cursor = idx - 1;
                    self.select_menu_action(MenuAction::ALL[self.menu_cursor]);
                }
            }
            _ => {}
        }
    }

    /// Act on a picked menu action. For actions that need a chain
    /// picker, stash the action in `pending_action` and transition
    /// to `ChainPicker`. For `Quit`, set `should_quit`. For
    /// `History`, set a deferred action and ask the loop to exit.
    fn select_menu_action(&mut self, action: MenuAction) {
        match action {
            MenuAction::Quit => {
                self.should_quit = true;
            }
            MenuAction::History => {
                // v1.2 Phase 2: History is global (no chain) — load
                // entries synchronously and transition straight to
                // the in-TUI list view. Errors bounce back to the
                // main menu with a status message.
                match crate::history::read_all() {
                    Ok(entries) => {
                        // Newest-first to match the CLI `history` subcommand.
                        let mut entries: Vec<_> = entries.into_iter().rev().collect();
                        entries.truncate(500);
                        if entries.is_empty() {
                            self.status = "No history yet · run a quote first".into();
                        } else {
                            let count = entries.len();
                            self.history_view = entries;
                            self.history_cursor = 0;
                            self.history_scroll = 0;
                            self.phase = Phase::ShowingHistory;
                            self.status = format!(
                                "History · {count} entries · ↑↓ PgUp/PgDn g/G · Esc back"
                            );
                        }
                    }
                    Err(e) => {
                        self.status = format!("History error: {e}");
                    }
                }
            }
            a => {
                self.pending_action = Some(a);
                // Reset the chain picker cursor to the current chain.
                // Benchmark gets the "All chains" row prepended.
                self.chain_picker_include_all = a.picker_includes_all();
                let base_cursor = Chain::ALL
                    .iter()
                    .position(|&c| c == self.chain)
                    .unwrap_or(0);
                self.chain_picker_cursor = if self.chain_picker_include_all {
                    base_cursor + 1
                } else {
                    base_cursor
                };
                self.phase = Phase::ChainPicker;
                self.status = format!(
                    "Pick a chain for {} · ↑↓ navigate · Enter select · Esc back",
                    a.label()
                );
            }
        }
    }

    /// Chain picker key handler. Arrow keys move the cursor, Enter
    /// selects. Esc returns to the main menu. Digits 0-4 (or 1-4,
    /// depending on `include_all`) jump-select.
    fn handle_chain_picker(&mut self, intent: Intent) {
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return;
        }
        let total = self.chain_picker_total();
        match intent {
            Intent::Quit => {
                // Back to the main menu, preserve menu cursor.
                self.phase = Phase::MainMenu;
                self.pending_action = None;
                self.chain_picker_include_all = false;
                self.status = "↑↓ navigate · Enter select · 1-8 jump · Esc quit".into();
            }
            Intent::ToggleHelp => {
                self.show_help = true;
            }
            Intent::Up | Intent::Prev => {
                self.chain_picker_cursor = self.chain_picker_cursor.saturating_sub(1);
            }
            Intent::Down | Intent::Next => {
                if self.chain_picker_cursor + 1 < total {
                    self.chain_picker_cursor += 1;
                }
            }
            Intent::Activate => {
                // Selection flows through `tui::run` — it reads
                // `pending_action` and `chain_picker_selected()` after
                // this call returns, rebuilds backends for the
                // selected chain (or sets a deferred action for
                // non-Quote flows), and transitions the phase.
                self.pending_transition = true;
            }
            Intent::Character(c) if c.is_ascii_digit() => {
                let idx = c.to_digit(10).unwrap_or(99) as usize;
                if self.chain_picker_include_all && idx == 0 {
                    self.chain_picker_cursor = 0;
                    self.pending_transition = true;
                } else if idx >= 1 && idx <= Chain::ALL.len() {
                    self.chain_picker_cursor = if self.chain_picker_include_all {
                        idx
                    } else {
                        idx - 1
                    };
                    self.pending_transition = true;
                }
            }
            _ => {}
        }
    }

    /// Row count for the chain picker — chains plus optional
    /// "All chains" row at the top.
    pub fn chain_picker_total(&self) -> usize {
        if self.chain_picker_include_all {
            Chain::ALL.len() + 1
        } else {
            Chain::ALL.len()
        }
    }

    /// The chain currently under the cursor in the picker, or
    /// `None` if the cursor is on the "All chains" row.
    pub fn chain_picker_selected(&self) -> Option<Chain> {
        if self.chain_picker_include_all {
            if self.chain_picker_cursor == 0 {
                None
            } else {
                Chain::ALL.get(self.chain_picker_cursor - 1).copied()
            }
        } else {
            Chain::ALL.get(self.chain_picker_cursor).copied()
        }
    }

    /// During an in-flight quote, the only keys that do anything are Esc
    /// (cancel) and Ctrl-C (quit). Everything else — including Tab, arrows,
    /// and text input — is swallowed so the user can't partially edit the
    /// form mid-quote.
    fn handle_quoting(&mut self, intent: Intent) {
        if matches!(intent, Intent::Quit) {
            // Drop the pending channel — orphans the tokio tasks but that's
            // fine because their sends into a closed channel are no-ops.
            self.pending = None;
            self.phase = Phase::EditingFields;
            self.focus = Field::FindQuote;
            self.status = "Quote cancelled · Enter to retry · Esc to return to menu".into();
        }
    }

    fn handle_editing(&mut self, intent: Intent) -> bool {
        // The help overlay intercepts key handling until it's dismissed.
        if self.show_help {
            if matches!(intent, Intent::ToggleHelp | Intent::Quit) {
                self.show_help = false;
            }
            return false;
        }

        match intent {
            Intent::Quit => {
                // v1.2: Esc from the form returns to the main menu
                // rather than quitting the TUI. Only Phase::MainMenu's
                // Esc handler can set `should_quit`.
                self.return_to_menu();
                false
            }
            Intent::QuoteNow => self.try_quote(),
            Intent::ToggleHelp => {
                self.show_help = true;
                false
            }
            Intent::SwapTokens => {
                std::mem::swap(&mut self.sell, &mut self.buy);
                self.status = "Swapped sell ↔ buy · press R or Enter to quote".into();
                false
            }
            Intent::YankBest => {
                self.yank_best_to_clipboard();
                false
            }
            Intent::Next => {
                self.advance_focus();
                false
            }
            Intent::Prev => {
                self.retreat_focus();
                false
            }
            Intent::Activate => match self.focus {
                Field::Sell => {
                    self.open_picker(Field::Sell);
                    false
                }
                Field::Buy => {
                    self.open_picker(Field::Buy);
                    false
                }
                Field::Amount => {
                    // Enter on Amount advances focus to the button so the
                    // user can see what they're about to do before firing
                    // the quote. Pressing Enter again on the button fires.
                    if self.try_quote_validates() {
                        self.focus = Field::FindQuote;
                    }
                    false
                }
                Field::FindQuote => self.try_quote(),
            },
            Intent::Character(c) if self.focus == Field::Amount => {
                if c.is_ascii_digit() || c == '.' {
                    self.amount_input.push(c);
                    self.phase = Phase::EditingFields;
                }
                false
            }
            Intent::Backspace if self.focus == Field::Amount => {
                self.amount_input.pop();
                self.phase = Phase::EditingFields;
                false
            }
            _ => false,
        }
    }

    fn handle_picker(&mut self, intent: Intent) {
        match intent {
            Intent::Quit => {
                self.phase = Phase::EditingFields;
                self.picker_filter.clear();
                self.picker_cursor = 0;
            }
            Intent::Up => {
                self.picker_cursor = self.picker_cursor.saturating_sub(1);
            }
            Intent::Down => {
                let total = self.picker_matches().len() + 1;
                if self.picker_cursor + 1 < total {
                    self.picker_cursor += 1;
                }
            }
            Intent::Activate => {
                let matches = self.picker_matches();
                // Position 0 is always "custom address".
                if self.picker_cursor == 0 {
                    self.phase = Phase::CustomAddressEntry;
                    self.custom_address_input.clear();
                    self.custom_address_error = None;
                } else if let Some(token) = matches.get(self.picker_cursor - 1).cloned() {
                    match self.picker_target {
                        Field::Sell => self.sell = Some(token),
                        Field::Buy => self.buy = Some(token),
                        Field::Amount | Field::FindQuote => {}
                    }
                    self.phase = Phase::EditingFields;
                    self.picker_filter.clear();
                    self.picker_cursor = 0;
                    self.advance_focus();
                }
            }
            Intent::Character(c) => {
                self.picker_filter.push(c);
                self.picker_cursor = 0;
            }
            Intent::Backspace => {
                self.picker_filter.pop();
                self.picker_cursor = 0;
            }
            _ => {}
        }
    }

    fn handle_custom_address(&mut self, intent: Intent) {
        match intent {
            Intent::Quit => {
                self.phase = Phase::TokenPicker;
                self.custom_address_error = None;
            }
            Intent::Activate => {
                let addr = self.custom_address_input.trim().to_string();
                if addr.is_empty() {
                    self.custom_address_error = Some("enter an address".into());
                    return;
                }
                // Basic client-side validation. Token::resolve does the
                // real validation and on-chain fetch.
                let is_solana = self.chain == Chain::Solana;
                if !is_solana && (addr.len() != 42 || !addr.starts_with("0x")) {
                    self.custom_address_error =
                        Some("address must be a 42-char 0x… hex string".into());
                    return;
                }
                if is_solana && addr.len() < 32 {
                    self.custom_address_error =
                        Some("address must be a valid base58 Solana mint".into());
                    return;
                }
                // Spawn async Token::resolve. The result lands in
                // `custom_token_rx` and `drain_custom_token` picks
                // it up on the next event loop tick.
                let (tx, rx) = tokio::sync::oneshot::channel();
                let chain = self.chain;
                let rpc_url: Option<String> = None; // RPC from ctx not available here; Token::resolve uses chain default
                tokio::spawn(async move {
                    let result = Token::resolve(&addr, chain, rpc_url.as_deref())
                        .await
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
                self.custom_token_rx = Some(rx);
                self.custom_token_target = self.picker_target;
                self.custom_address_input.clear();
                self.custom_address_error = None;
                self.phase = Phase::EditingFields;
                self.status = "Resolving custom token…".into();
            }
            Intent::Character(c) => {
                if self.custom_address_input.len() < 64 {
                    self.custom_address_input.push(c);
                }
            }
            Intent::Backspace => {
                self.custom_address_input.pop();
            }
            _ => {}
        }
    }

    pub fn picker_matches(&self) -> Vec<Token> {
        if self.picker_source.is_empty() {
            return Vec::new();
        }
        if self.picker_filter.is_empty() {
            return self.picker_source.clone();
        }
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = Pattern::new(
            &self.picker_filter,
            CaseMatching::Ignore,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        let mut scored: Vec<(u32, Token)> = self
            .picker_source
            .iter()
            .filter_map(|t| {
                let haystack = format!("{} {}", t.symbol, t.name);
                let mut buf = Vec::new();
                let hay = nucleo_matcher::Utf32Str::new(&haystack, &mut buf);
                pattern.score(hay, &mut matcher).map(|s| (s, t.clone()))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, t)| t).collect()
    }

    fn open_picker(&mut self, target: Field) {
        self.picker_target = target;
        self.picker_source = list_tokens(self.chain);
        self.picker_filter.clear();
        self.picker_cursor = 0;
        self.phase = Phase::TokenPicker;
    }

    /// Format and copy the best current result into the system clipboard.
    /// Falls back to updating the status line with an error message if
    /// the platform clipboard is unavailable (remote SSH session without
    /// X forwarding, WSL without wslg, etc).
    fn yank_best_to_clipboard(&mut self) {
        let Some(results) = &self.results else {
            self.status = "No quote to copy yet — run a quote first".into();
            return;
        };
        let request = match &self.last_request {
            Some(req) => req,
            None => {
                self.status = "No quote to copy yet".into();
                return;
            }
        };
        let best = results
            .iter()
            .filter_map(|r| r.quote.as_ref().ok())
            .max_by_key(|q| q.amount_out);
        let Some(best) = best else {
            self.status = "No successful quote to copy".into();
            return;
        };

        let line = format!(
            "{}: {} {} (from {} {})",
            best.backend,
            dexquote_core::token::format_amount(best.amount_out, request.token_out.decimals, 6),
            request.token_out.symbol,
            dexquote_core::token::format_amount(request.amount_in, request.token_in.decimals, 6),
            request.token_in.symbol,
        );

        match arboard::Clipboard::new().and_then(|mut c| c.set_text(line.clone())) {
            Ok(()) => {
                self.status = format!("Copied to clipboard: {line}");
            }
            Err(e) => {
                self.status = format!("Clipboard unavailable: {e}");
            }
        }
    }

    /// Non-mutating version used by the form-editing flow so we can see if
    /// the fields are complete WITHOUT asking the caller to kick off a
    /// quote. Mirror of `try_quote` minus the side effect.
    fn try_quote_validates(&mut self) -> bool {
        if self.sell.is_none() || self.buy.is_none() || self.amount_input.is_empty() {
            self.status = "Fill in all three fields first".into();
            return false;
        }
        self.status =
            "Press Enter on [Find Quote] to run · Tab to edit fields · Esc to quit".into();
        true
    }

    fn try_quote(&mut self) -> bool {
        if self.sell.is_none() || self.buy.is_none() || self.amount_input.is_empty() {
            self.status = "Fill in all three fields first".into();
            return false;
        }
        // Validation happens inside the parent run_quote — if amount is bad
        // we'll surface the error there.
        true
    }

    fn advance_focus(&mut self) {
        self.focus = match self.focus {
            Field::Sell => Field::Buy,
            Field::Buy => Field::Amount,
            Field::Amount => Field::FindQuote,
            Field::FindQuote => Field::Sell,
        };
    }

    fn retreat_focus(&mut self) {
        self.focus = match self.focus {
            Field::Sell => Field::FindQuote,
            Field::Buy => Field::Sell,
            Field::Amount => Field::Buy,
            Field::FindQuote => Field::Amount,
        };
    }
}
