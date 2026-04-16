//! ratatui rendering for the TUI.
//!
//! Layout:
//!   ┌────────── dexquote · Arbitrum ──────────┐
//!   │ Sell   › WETH · Wrapped Ether           │
//!   │ Buy      USDC · USD Coin                │
//!   │ Amount   1.0                            │
//!   │                                          │
//!   │ Tab: next field · Enter: select · Esc…  │
//!   ├──────────────────────────────────────────┤
//!   │ Results panel                            │
//!   └──────────────────────────────────────────┘
//!
//! When the token picker is open, a centered overlay is drawn over the
//! form. When results are present, they replace the help panel below.

use super::app::{App, Field, MenuAction, Phase};
use dexquote_core::token::format_amount;
use dexquote_core::{BackendResult, Chain, QuoteRequest, U256};

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap};

const PRIMARY: Color = Color::Cyan;
const ACCENT: Color = Color::Green;
const DIM: Color = Color::DarkGray;

pub fn draw(frame: &mut Frame, app: &App) {
    let size = frame.area();

    // v1.2 pre-action phases: the user is in the main menu or chain
    // picker. These render as centered overlays on top of a cleared
    // background — no gas tracker, no form, no results. The main
    // menu is the TUI's landing and exit point.
    if matches!(app.phase, Phase::MainMenu | Phase::ChainPicker) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // gas tracker strip (blank until a chain is picked)
                Constraint::Min(10),   // centered menu/picker overlay background
                Constraint::Length(1), // status line
            ])
            .split(size);
        draw_gas_tracker(frame, chunks[0], app);
        frame.render_widget(Clear, chunks[1]);
        if app.phase == Phase::MainMenu {
            draw_main_menu(frame, chunks[1], app);
        } else {
            draw_chain_picker(frame, chunks[1], app);
        }
        draw_status(frame, chunks[2], app);
        if app.show_help {
            draw_help_overlay(frame, size);
        }
        return;
    }

    // v1.2 Phase 2 list views: Tokens and History. Both render as
    // full-body scrollable lists with the gas tracker strip above
    // and the status line below. Shared key handling (PgUp/Dn,
    // g/G) lives in `app::handle_list_view`.
    if matches!(app.phase, Phase::ShowingTokens | Phase::ShowingHistory) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // gas tracker strip
                Constraint::Min(10),   // list body
                Constraint::Length(1), // status line
            ])
            .split(size);
        draw_gas_tracker(frame, chunks[0], app);
        if app.phase == Phase::ShowingTokens {
            draw_tokens_list(frame, chunks[1], app);
        } else {
            draw_history_list(frame, chunks[1], app);
        }
        draw_status(frame, chunks[2], app);
        if app.show_help {
            draw_help_overlay(frame, size);
        }
        return;
    }

    // v1.2 Phase 4: Depth sweep in flight, or completed depth
    // report. Full-body panel with gas tracker above and status
    // line below. Progressive rendering while `Phase::Depthing`;
    // final table once `Phase::ShowingDepth`.
    if matches!(app.phase, Phase::Depthing | Phase::ShowingDepth) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(size);
        draw_gas_tracker(frame, chunks[0], app);
        draw_depth(frame, chunks[1], app);
        draw_status(frame, chunks[2], app);
        if app.show_help {
            draw_help_overlay(frame, size);
        }
        return;
    }

    // v1.2 Phase 5: Benchmark sweep in flight, or completed
    // leaderboard. No gas tracker strip — benchmark runs across
    // chains so a single chain's gas doesn't make sense here.
    if matches!(app.phase, Phase::Benchmarking | Phase::ShowingBenchmark) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // (blank) reserved top strip
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(size);
        draw_benchmark(frame, chunks[1], app);
        draw_status(frame, chunks[2], app);
        if app.show_help {
            draw_help_overlay(frame, size);
        }
        return;
    }

    // v1.2 Phase 6: Doctor self-test. Live progress view while
    // Doctoring; complete report once ShowingDoctor.
    if matches!(app.phase, Phase::Doctoring | Phase::ShowingDoctor) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(10),
                Constraint::Length(1),
            ])
            .split(size);
        draw_gas_tracker(frame, chunks[0], app);
        draw_doctor(frame, chunks[1], app);
        draw_status(frame, chunks[2], app);
        if app.show_help {
            draw_help_overlay(frame, size);
        }
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),   // gas tracker strip
            Constraint::Length(11),  // form
            Constraint::Min(10),     // results / help
            Constraint::Length(1),   // status
        ])
        .split(size);

    draw_gas_tracker(frame, chunks[0], app);
    draw_form(frame, chunks[1], app);
    draw_results(frame, chunks[2], app);
    draw_status(frame, chunks[3], app);

    if app.phase == Phase::TokenPicker {
        draw_picker_overlay(frame, size, app);
    }
    if app.phase == Phase::CustomAddressEntry {
        draw_custom_address_overlay(frame, size, app);
    }
    if app.show_help {
        draw_help_overlay(frame, size);
    }
}

/// v1.2 main menu: the landing and exit point of the TUI.
/// Renders a centered box with the 8 MenuAction rows. Selection is
/// driven by `app.menu_cursor`.
fn draw_main_menu(frame: &mut Frame, area: Rect, app: &App) {
    let inner = center_rect(62, 16, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // title
            Constraint::Min(8),    // action list
            Constraint::Length(2), // footer hint
        ])
        .margin(1)
        .split(inner);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(" dexquote ")
        .title_alignment(Alignment::Center);
    frame.render_widget(block, inner);

    let title = Paragraph::new("What would you like to do?")
        .alignment(Alignment::Center)
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(title, chunks[0]);

    let items: Vec<ListItem> = MenuAction::ALL
        .iter()
        .enumerate()
        .map(|(i, action)| {
            let num = format!("{}.", i + 1);
            let label = action.label();
            let desc = action.description();
            let line = format!("  {:<3} {:<10}  — {}", num, label, desc);
            ListItem::new(line)
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(PRIMARY)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.menu_cursor));
    frame.render_stateful_widget(list, chunks[1], &mut list_state);

    let hint = Paragraph::new("↑↓ navigate · Enter select · 1-8 jump · ? help · q/Esc quit")
        .alignment(Alignment::Center)
        .style(Style::default().fg(DIM));
    frame.render_widget(hint, chunks[2]);
}

/// v1.2 chain picker: shown after the user selects an action from
/// the main menu. Optionally includes an "All chains" row at the
/// top when the pending action is Benchmark.
fn draw_chain_picker(frame: &mut Frame, area: Rect, app: &App) {
    let include_all = app.chain_picker_include_all;
    let width = if include_all { 54 } else { 48 };
    let height = if include_all { 14 } else { 12 };
    let inner = center_rect(width, height, area);
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(4),
            Constraint::Length(2),
        ])
        .margin(1)
        .split(inner);

    let action_label = app
        .pending_action
        .map(|a| a.label())
        .unwrap_or("dexquote");
    let title = format!(" {} ", action_label);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(title)
        .title_alignment(Alignment::Center);
    frame.render_widget(block, inner);

    let header = Paragraph::new("Which chain?")
        .alignment(Alignment::Center)
        .style(Style::default().add_modifier(Modifier::BOLD));
    frame.render_widget(header, chunks[0]);

    let mut items: Vec<ListItem> = Vec::new();
    if include_all {
        items.push(ListItem::new(
            "  0.  All chains            (every supported chain)".to_string(),
        ));
    }
    for (i, chain) in Chain::ALL.iter().enumerate() {
        let num = format!("{}.", i + 1);
        items.push(ListItem::new(format!(
            "  {:<3} {:<20} {}",
            num,
            chain.name(),
            chain_backend_tag(*chain)
        )));
    }

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(PRIMARY)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.chain_picker_cursor));
    frame.render_stateful_widget(list, chunks[1], &mut list_state);

    let hint_text = if include_all {
        "↑↓ navigate · Enter select · 0-4 jump · Esc back"
    } else {
        "↑↓ navigate · Enter select · 1-4 jump · Esc back"
    };
    let hint = Paragraph::new(hint_text)
        .alignment(Alignment::Center)
        .style(Style::default().fg(DIM));
    frame.render_widget(hint, chunks[2]);
}

/// Per-chain coverage hint shown in the chain picker rows.
fn chain_backend_tag(chain: Chain) -> &'static str {
    match chain {
        Chain::Arbitrum => "14 backends",
        Chain::Base => "11 backends",
        Chain::Ethereum => "16 backends",
        Chain::Solana => "5 backends",
    }
}

/// v1.2 tokens list view. Renders the bundled registry for
/// `app.chain` as a scrollable ratatui `List` with the same column
/// layout as the CLI `dexquote tokens` output (symbol, name,
/// address, decimals).
fn draw_tokens_list(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" {} tokens · {} ", app.chain.name(), app.tokens_view.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(title)
        .title_alignment(Alignment::Left);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Column widths tuned for the widest symbols/names in the
    // bundled registry.
    let items: Vec<ListItem> = app
        .tokens_view
        .iter()
        .map(|t| {
            let addr = t.address.display_string();
            // Shorten addresses to fit narrower terminals:
            let addr_short = if addr.len() > 44 {
                format!("{}…{}", &addr[..22], &addr[addr.len() - 8..])
            } else {
                addr
            };
            let line = format!(
                "  {:<10} {:<26} {:<44}  {:>2}d",
                t.symbol,
                truncate(&t.name, 26),
                addr_short,
                t.decimals
            );
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(PRIMARY)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.tokens_cursor));
    frame.render_stateful_widget(list, inner, &mut list_state);
}

/// v1.2 history list view. Renders the JSONL log as a scrollable
/// list with the same column layout as the CLI `dexquote history`
/// output (ts, amount+pair, best backend, spread).
fn draw_history_list(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" Recent quotes · {} ", app.history_view.len());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(title)
        .title_alignment(Alignment::Left);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items: Vec<ListItem> = app
        .history_view
        .iter()
        .map(|entry| {
            let ts = crate::history::format_relative_ts(entry.ts);
            let best = entry.best_backend.as_deref().unwrap_or("—");
            let spread = entry
                .spread_pct
                .map(|p| format!("{:.2}%", p))
                .unwrap_or_else(|| "—".into());
            let line = format!(
                " {:>14}  {:>6} {:<8} → {:<8}  best {:<10}  spread {}",
                ts,
                entry.amount_in_human(),
                entry.sell_symbol,
                entry.buy_symbol,
                best,
                spread,
            );
            ListItem::new(line)
        })
        .collect();

    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(PRIMARY)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.history_cursor));
    frame.render_stateful_widget(list, inner, &mut list_state);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// v1.2 Phase 4: depth sweep view. Renders either the in-flight
/// progress table (spinner on the current level, empty/queued
/// rows) or the completed price-impact table from `depth_report`.
fn draw_depth(frame: &mut Frame, area: Rect, app: &App) {
    let (title, border_color) = match app.phase {
        Phase::Depthing => {
            let done = app
                .depth_levels
                .iter()
                .filter(|s| s.is_some())
                .count();
            let total = app.depth_levels.len();
            (format!(" Depth sweep {done}/{total} "), PRIMARY)
        }
        Phase::ShowingDepth => (" Depth complete ".to_string(), ACCENT),
        _ => (" Depth ".to_string(), DIM),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.phase == Phase::ShowingDepth {
        if let Some(report) = &app.depth_report {
            draw_depth_report(frame, inner, report);
            return;
        }
    }

    // In-flight: render a row per notional. Completed rows show
    // the best venue; the current row shows a spinner; queued
    // rows show a "." bullet.
    let spinner = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
    let mut lines = Vec::with_capacity(app.depth_levels.len() + 3);
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " {} {} → {}",
            app.depth_base_amount_human, app.depth_sell_symbol, app.depth_buy_symbol
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    for (idx, slot) in app.depth_levels.iter().enumerate() {
        let mult = crate::depth::NOTIONALS.get(idx).copied().unwrap_or(0.0);
        let mult_label = if mult >= 1.0 {
            format!("{}×", mult as u64)
        } else {
            format!("{}×", mult)
        };
        let row = match slot {
            Some(level) => match (level.amount_out, &level.best_venue) {
                (Some(out), Some(venue)) => {
                    let amount = format_amount(
                        out,
                        8, /* best-effort; exact decimals unknown at this layer */
                        4,
                    );
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled("✓", Style::default().fg(ACCENT)),
                        Span::raw("  "),
                        Span::styled(format!("{:<8}", mult_label), Style::default()),
                        Span::raw(" "),
                        Span::styled(format!("{:>16}", amount), Style::default()),
                        Span::raw("   via "),
                        Span::styled(venue.to_string(), Style::default().fg(PRIMARY)),
                    ])
                }
                _ => Line::from(vec![
                    Span::raw(" "),
                    Span::styled("✗", Style::default().fg(Color::Yellow)),
                    Span::raw("  "),
                    Span::styled(format!("{:<8}", mult_label), Style::default()),
                    Span::raw(" "),
                    Span::styled("no route", Style::default().fg(DIM)),
                ]),
            },
            None if idx == app.depth_current && app.phase == Phase::Depthing => Line::from(vec![
                Span::raw(" "),
                Span::styled(spinner.to_string(), Style::default().fg(PRIMARY)),
                Span::raw("  "),
                Span::styled(
                    format!("{:<8}", mult_label),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled("fetching…", Style::default().fg(DIM)),
            ]),
            None => Line::from(vec![
                Span::raw(" "),
                Span::styled(".", Style::default().fg(DIM)),
                Span::raw("  "),
                Span::styled(format!("{:<8}", mult_label), Style::default().fg(DIM)),
                Span::raw(" "),
                Span::styled("queued", Style::default().fg(DIM)),
            ]),
        };
        lines.push(row);
    }

    lines.push(Line::from(""));
    if let Some(started) = app.depth_started_at {
        lines.push(Line::from(Span::styled(
            format!(" Elapsed {}", format_elapsed(started.elapsed().as_millis())),
            Style::default().fg(DIM),
        )));
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

/// Render the completed `DepthReport` as a price-impact table
/// inside the depth panel. Structure mirrors the CLI
/// `render::depth::render_depth` output: one row per notional,
/// showing effective rate + price impact vs baseline.
fn draw_depth_report(
    frame: &mut Frame,
    area: Rect,
    report: &crate::render::depth::DepthReport,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " {} {} → {} depth on {}",
            report.base_amount_human,
            report.sell.symbol,
            report.buy.symbol,
            report.chain.name()
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Baseline effective rate: first level with a non-zero
    // amount_out. Used to compute per-level price impact.
    let baseline_rate: Option<f64> = report
        .levels
        .iter()
        .find(|l| l.amount_out.is_some() && !l.amount_in.is_zero())
        .and_then(|l| {
            let out = u256_to_f64_scaled(l.amount_out?, report.buy.decimals);
            let input = u256_to_f64_scaled(l.amount_in, report.sell.decimals);
            if input == 0.0 {
                None
            } else {
                Some(out / input)
            }
        });

    for (idx, level) in report.levels.iter().enumerate() {
        let mult_label = if level.multiplier >= 1.0 {
            format!("{}×", level.multiplier as u64)
        } else {
            format!("{}×", level.multiplier)
        };
        let input_human = format_amount(level.amount_in, report.sell.decimals, 4);
        let row = match (level.amount_out, &level.best_venue) {
            (Some(out), Some(venue)) => {
                let amount = format_amount(out, report.buy.decimals, 4);
                let rate = {
                    let o = u256_to_f64_scaled(out, report.buy.decimals);
                    let i = u256_to_f64_scaled(level.amount_in, report.sell.decimals);
                    if i > 0.0 {
                        Some(o / i)
                    } else {
                        None
                    }
                };
                let impact_label = match (rate, baseline_rate) {
                    (Some(r), Some(b)) if b > 0.0 => {
                        if idx == 0 {
                            "(baseline)".to_string()
                        } else {
                            let pct = ((r - b) / b) * 100.0;
                            format!("{:+.3}%", pct)
                        }
                    }
                    _ => "—".to_string(),
                };
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<6}", mult_label),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                    Span::raw(" ("),
                    Span::styled(
                        format!("{:>12} {}", input_human, report.sell.symbol),
                        Style::default(),
                    ),
                    Span::raw(")   "),
                    Span::styled(
                        format!("{:>16} {}", amount, report.buy.symbol),
                        Style::default(),
                    ),
                    Span::raw("   "),
                    Span::styled(format!("{:>10}", impact_label), Style::default().fg(DIM)),
                    Span::raw("   "),
                    Span::styled(venue.to_string(), Style::default().fg(PRIMARY)),
                ])
            }
            _ => Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:<6}", mult_label),
                    Style::default().fg(DIM),
                ),
                Span::raw(" ("),
                Span::styled(
                    format!("{:>12} {}", input_human, report.sell.symbol),
                    Style::default().fg(DIM),
                ),
                Span::raw(")   "),
                Span::styled("no route".to_string(), Style::default().fg(Color::Yellow)),
            ]),
        };
        lines.push(row);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Price impact measured vs the 0.1× baseline · Esc to return",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

/// v1.2 Phase 5: benchmark sweep view. In-flight renders an
/// overall progress bar + the scroll buffer of recent pairs.
/// Completed renders the leaderboard table.
fn draw_benchmark(frame: &mut Frame, area: Rect, app: &App) {
    let (title, border_color) = match app.phase {
        Phase::Benchmarking => (
            format!(" Benchmark {}/{} ", app.bench_done, app.bench_total),
            PRIMARY,
        ),
        Phase::ShowingBenchmark => (" Benchmark complete ".to_string(), ACCENT),
        _ => (" Benchmark ".to_string(), DIM),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.phase == Phase::ShowingBenchmark {
        if let Some(stats) = &app.bench_stats {
            draw_benchmark_stats(frame, inner, stats);
            return;
        }
    }

    // In-flight progress view: header with elapsed + count, a
    // horizontal progress bar, the current chain/pair, the scroll
    // buffer of recent completed pairs, and any skipped chains.
    let elapsed = app
        .bench_started_at
        .map(|s| s.elapsed().as_secs())
        .unwrap_or(0);
    let filter_label = app
        .bench_chain_filter
        .map(|c| c.name().to_string())
        .unwrap_or_else(|| "all chains".to_string());

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " Running benchmark across {} pairs ({}) · {:02}:{:02} elapsed",
            app.bench_total,
            filter_label,
            elapsed / 60,
            elapsed % 60
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));

    // Horizontal progress bar. 40 cells wide, █ for done, ░ for
    // remaining. Rounded by percentage, not per-pair, to keep the
    // visual smooth on large pair counts.
    let pct = if app.bench_total > 0 {
        (app.bench_done as f64 / app.bench_total as f64).min(1.0)
    } else {
        0.0
    };
    let bar_w = 40usize;
    let filled = (pct * bar_w as f64).round() as usize;
    let bar: String = format!(
        "  [{}{}]  {:>3}%",
        "█".repeat(filled),
        "░".repeat(bar_w - filled),
        (pct * 100.0).round() as u32
    );
    lines.push(Line::from(Span::styled(bar, Style::default().fg(PRIMARY))));
    lines.push(Line::from(""));

    // Current chain / pair row with spinner.
    let spinner = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
    if let Some(chain) = app.bench_current_chain {
        let pair = app
            .bench_current_pair
            .as_ref()
            .map(|(s, b)| format!("{} → {}", s, b))
            .unwrap_or_else(|| "preparing…".into());
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(spinner.to_string(), Style::default().fg(PRIMARY)),
            Span::raw("  "),
            Span::styled(
                format!("{:<10}", chain.name()),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(pair, Style::default().fg(DIM)),
        ]));
    }
    lines.push(Line::from(""));

    // Scroll buffer: recent completed pairs, newest at the bottom.
    lines.push(Line::from(Span::styled(
        " Recent pairs:",
        Style::default().fg(DIM),
    )));
    for summary in app.bench_scroll.iter() {
        let ok_label = format!("{}/{}", summary.ok_count, summary.total_count);
        let best_label = summary.best_backend.as_deref().unwrap_or("—");
        lines.push(Line::from(vec![
            Span::raw("   "),
            Span::styled("✓", Style::default().fg(ACCENT)),
            Span::raw("  "),
            Span::styled(
                format!("{:<10}", summary.chain.name()),
                Style::default().fg(DIM),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{:>5} {:<6} → {:<6}", summary.amount, summary.sell, summary.buy),
                Style::default(),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{:>5} ok", ok_label),
                Style::default().fg(DIM),
            ),
            Span::raw("  "),
            Span::styled(
                format!("{:>5}", format_elapsed(summary.elapsed_ms)),
                Style::default().fg(DIM),
            ),
            Span::raw("  best "),
            Span::styled(best_label.to_string(), Style::default().fg(PRIMARY)),
        ]));
    }

    // Skipped chains footer.
    if !app.bench_skipped.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(" Skipped {} chain(s):", app.bench_skipped.len()),
            Style::default().fg(Color::Yellow),
        )));
        for (chain, reason) in &app.bench_skipped {
            lines.push(Line::from(Span::styled(
                format!("   ! {}: {}", chain.name(), reason),
                Style::default().fg(Color::Yellow),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Render the completed per-backend leaderboard as a styled
/// column table. Top 3 rows highlighted in green; success rate
/// <50% shown dimmed.
fn draw_benchmark_stats(
    frame: &mut Frame,
    area: Rect,
    stats: &crate::benchmark::BenchmarkStats,
) {
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " Benchmark across {} pairs ({})",
            stats.total_pairs,
            format_elapsed(stats.total_elapsed_ms)
        ),
        Style::default().add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            "  {:<16}  {:>5}  {:>12}  {:>8}  {:>10}",
            "backend", "wins", "ok rate", "p50 lat", "avg spread"
        ),
        Style::default().fg(DIM).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(Span::styled(
        format!("  {}", "-".repeat(58)),
        Style::default().fg(DIM),
    )));

    for (idx, stat) in stats.backends.iter().enumerate() {
        let ok_label = format!(
            "{}/{} ({:.0}%)",
            stat.successes, stat.attempts, stat.success_rate
        );
        let lat_label = format!("{}ms", stat.median_latency_ms);
        let spread_label = format!("{:+.3}%", stat.avg_spread_pct);
        let mut style = Style::default();
        if idx < 3 {
            style = style.fg(ACCENT).add_modifier(Modifier::BOLD);
        } else if stat.success_rate < 50.0 {
            style = style.fg(DIM);
        }
        lines.push(Line::from(vec![Span::styled(
            format!(
                "  {:<16}  {:>5}  {:>12}  {:>8}  {:>10}",
                stat.name, stat.wins, ok_label, lat_label, spread_label
            ),
            style,
        )]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Press Esc to return to the main menu",
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

/// v1.2 Phase 6: doctor self-test view. Renders progressively
/// as `drain_doctor` appends items to `doctor_sections`. When
/// the `Finished` event lands, the phase flips to
/// `Phase::ShowingDoctor` and this same function renders the
/// completed report (the render code paths are identical — the
/// phase just controls the block title and spinner).
fn draw_doctor(frame: &mut Frame, area: Rect, app: &App) {
    let (title, border_color) = match app.phase {
        Phase::Doctoring => {
            let section = app.doctor_current_section.unwrap_or("…");
            (format!(" Doctor · {section} "), PRIMARY)
        }
        Phase::ShowingDoctor => (" Doctor complete ".to_string(), ACCENT),
        _ => (" Doctor ".to_string(), DIM),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut lines: Vec<Line> = Vec::new();
    for (idx, section) in app.doctor_sections.iter().enumerate() {
        if idx > 0 {
            lines.push(Line::from(""));
        }
        lines.push(Line::from(Span::styled(
            format!(" ── {} ──", section.name),
            Style::default().add_modifier(Modifier::BOLD),
        )));
        for item in &section.items {
            let (icon, icon_color) = match item.status {
                crate::doctor::DoctorStatus::Ok => ("✓", ACCENT),
                crate::doctor::DoctorStatus::Warn => ("⚠", Color::Yellow),
                crate::doctor::DoctorStatus::Fail => ("✗", Color::Red),
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    icon.to_string(),
                    Style::default().fg(icon_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
                Span::styled(item.body.clone(), Style::default()),
            ]));
            if let Some((note_status, note)) = &item.note {
                let (nicon, ncolor) = match note_status {
                    crate::doctor::DoctorStatus::Ok => ("✓", ACCENT),
                    crate::doctor::DoctorStatus::Warn => ("⚠", Color::Yellow),
                    crate::doctor::DoctorStatus::Fail => ("✗", Color::Red),
                };
                lines.push(Line::from(vec![
                    Span::raw("    "),
                    Span::styled(nicon.to_string(), Style::default().fg(ncolor)),
                    Span::raw(" "),
                    Span::styled(note.clone(), Style::default().fg(DIM)),
                ]));
            }
        }
    }

    // Spinner row for the active section, only while still
    // probing.
    if app.phase == Phase::Doctoring {
        if let Some(current) = app.doctor_current_section {
            let spinner = SPINNER_FRAMES[app.spinner_frame % SPINNER_FRAMES.len()];
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(spinner.to_string(), Style::default().fg(PRIMARY)),
                Span::raw("  "),
                Span::styled(
                    format!("running {}…", current),
                    Style::default().fg(DIM),
                ),
            ]));
        }
    } else if app.phase == Phase::ShowingDoctor {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                " Completed in {} · Esc return to menu",
                format_elapsed(app.doctor_total_elapsed_ms)
            ),
            Style::default().fg(DIM),
        )));
    }

    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// Safe U256 → f64 conversion scaled by the token's decimal
/// exponent. Used in the depth panel to compute effective rates.
fn u256_to_f64_scaled(v: U256, decimals: u8) -> f64 {
    let divisor = U256::from(10u128).pow(U256::from(decimals));
    if divisor.is_zero() {
        return 0.0;
    }
    let whole = v / divisor;
    let frac = v % divisor;
    let w = if whole > U256::from(u128::MAX) {
        u128::MAX as f64
    } else {
        whole.to::<u128>() as f64
    };
    let f = if frac > U256::from(u128::MAX) {
        u128::MAX as f64
    } else {
        frac.to::<u128>() as f64
    };
    let scale = 10f64.powi(decimals as i32);
    w + (f / scale)
}

/// Shared helper for centering a fixed-size rect inside a parent area.
fn center_rect(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

/// Thin strip at the very top of the TUI showing live network data.
/// On EVM chains: gas price (gwei), ETH/USD, typical swap cost, block
/// number, freshness. On Solana: priority fee (µlmp/CU), SOL/USD,
/// typical swap cost, slot, freshness. Re-renders every frame so the
/// freshness counter ticks visibly between polls.
fn draw_gas_tracker(frame: &mut Frame, area: Rect, app: &App) {
    let line = match app.chain {
        Chain::Solana => draw_solana_gas(app),
        _ => draw_evm_gas(app),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn draw_evm_gas(app: &App) -> Line<'static> {
    match &app.gas_snapshot {
        Some(snap) => {
            let gwei = snap.gas_price_gwei();
            let eth = snap.eth_usd;
            let swap = snap.swap_cost_usd();
            let age = snap.age_secs();
            let age_label = if age == 0 {
                "now".to_string()
            } else {
                format!("{age}s ago")
            };

            // Color the gas value by heat: green under 0.1 gwei, yellow
            // 0.1–0.5, red above. Arbitrum usually sits around 0.01–0.1
            // gwei; the spikes only happen when L1 calldata costs jump.
            let gwei_color = if gwei < 0.1 {
                ACCENT
            } else if gwei < 0.5 {
                Color::Yellow
            } else {
                Color::Red
            };

            Line::from(vec![
                Span::raw("  "),
                Span::styled("gas ", Style::default().fg(DIM)),
                Span::styled(
                    format!("{:.3} gwei", gwei),
                    Style::default().fg(gwei_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled("   ETH ", Style::default().fg(DIM)),
                Span::styled(
                    format!("${:.2}", eth),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("   swap ", Style::default().fg(DIM)),
                Span::styled(
                    if swap >= 0.01 {
                        format!("~${:.2}", swap)
                    } else {
                        "~<$0.01".to_string()
                    },
                    Style::default().fg(ACCENT),
                ),
                Span::styled("   block ", Style::default().fg(DIM)),
                Span::styled(
                    format!("{}", snap.block_number),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("   · {age_label}"), Style::default().fg(DIM)),
            ])
        }
        None => {
            if app.gas_rx.is_some() {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "gas tracker: fetching first snapshot…",
                        Style::default().fg(DIM),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "gas tracker: disabled (no RPC configured)",
                        Style::default().fg(DIM),
                    ),
                ])
            }
        }
    }
}

fn draw_solana_gas(app: &App) -> Line<'static> {
    match &app.sol_gas_snapshot {
        Some(snap) => {
            let pri = snap.priority_fee_micro_lamports_per_cu;
            let sol = snap.sol_usd;
            let swap = snap.swap_cost_usd();
            let age = snap.age_secs();
            let age_label = if age == 0 {
                "now".to_string()
            } else {
                format!("{age}s ago")
            };

            // Priority fee color coding: 0 = green (idle, free),
            // under 10k = green, 10k-100k = yellow (congested),
            // above = red (heavy congestion). Solana fees are
            // nearly-always near zero in practice so green is the
            // dominant state.
            let pri_color = if pri < 10_000 {
                ACCENT
            } else if pri < 100_000 {
                Color::Yellow
            } else {
                Color::Red
            };
            let pri_label = format_priority_fee(pri);

            // Solana swap costs are extremely low — display with extra
            // precision below $0.01 so the difference between "free"
            // (base fee only) and "slightly congested" is visible.
            let swap_label = if swap >= 0.01 {
                format!("~${:.2}", swap)
            } else if swap >= 0.0001 {
                format!("~${:.4}", swap)
            } else {
                "~<$0.0001".to_string()
            };

            Line::from(vec![
                Span::raw("  "),
                Span::styled("pri ", Style::default().fg(DIM)),
                Span::styled(
                    pri_label,
                    Style::default().fg(pri_color).add_modifier(Modifier::BOLD),
                ),
                Span::styled("   SOL ", Style::default().fg(DIM)),
                Span::styled(
                    format!("${:.2}", sol),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled("   swap ", Style::default().fg(DIM)),
                Span::styled(swap_label, Style::default().fg(ACCENT)),
                Span::styled("   slot ", Style::default().fg(DIM)),
                Span::styled(
                    format!("{}", snap.slot),
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("   · {age_label}"), Style::default().fg(DIM)),
            ])
        }
        None => {
            if app.sol_gas_rx.is_some() {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "sol gas tracker: fetching first snapshot…",
                        Style::default().fg(DIM),
                    ),
                ])
            } else {
                Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        "sol gas tracker: disabled",
                        Style::default().fg(DIM),
                    ),
                ])
            }
        }
    }
}

/// Format a priority fee in micro-lamports per CU with a compact
/// `0.0µ`, `5.2kµ`, `120kµ` style — the raw number gets large fast and
/// the header strip has limited horizontal space.
fn format_priority_fee(micro_lamports_per_cu: u64) -> String {
    if micro_lamports_per_cu < 1_000 {
        format!("{}µ/cu", micro_lamports_per_cu)
    } else if micro_lamports_per_cu < 1_000_000 {
        format!("{:.1}kµ/cu", micro_lamports_per_cu as f64 / 1_000.0)
    } else {
        format!("{:.1}Mµ/cu", micro_lamports_per_cu as f64 / 1_000_000.0)
    }
}

fn draw_form(frame: &mut Frame, area: Rect, app: &App) {
    let title = format!(" dexquote · {} ", app.chain.name());
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(title)
        .title_alignment(Alignment::Left);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Sell
            Constraint::Length(1), // Buy
            Constraint::Length(1), // Amount
            Constraint::Length(1), // spacer
            Constraint::Length(1), // Find Quote button
            Constraint::Length(1), // spacer
            Constraint::Length(1), // hint
        ])
        .split(inner);

    frame.render_widget(
        field_line("Sell", token_display(app.sell.as_ref()), app.focus == Field::Sell),
        rows[0],
    );
    frame.render_widget(
        field_line("Buy", token_display(app.buy.as_ref()), app.focus == Field::Buy),
        rows[1],
    );
    frame.render_widget(
        amount_line("Amount", &app.amount_input, app.focus == Field::Amount),
        rows[2],
    );

    frame.render_widget(
        find_quote_button(app.focus == Field::FindQuote, app.sell.is_some() && app.buy.is_some() && !app.amount_input.is_empty()),
        rows[4],
    );

    let hint = Paragraph::new(Line::from(vec![
        Span::styled(
            "Tab ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("navigate  ", Style::default().fg(DIM)),
        Span::styled(
            "Enter ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("select  ", Style::default().fg(DIM)),
        Span::styled(
            "R ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("re-run  ", Style::default().fg(DIM)),
        Span::styled(
            "Ctrl+Enter ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("quick  ", Style::default().fg(DIM)),
        Span::styled(
            "Esc ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled("quit", Style::default().fg(DIM)),
    ]));
    frame.render_widget(hint, rows[6]);
}

fn find_quote_button<'a>(focused: bool, ready: bool) -> Paragraph<'a> {
    // Three visual states:
    //   - focused + ready: bright green solid button, primed to fire
    //   - unfocused + ready: dim button, waiting to be navigated to
    //   - not ready (any focus): dark-gray button, "fill fields first"
    let label = "  Find Quote  ";
    let (style, border) = if focused && ready {
        (
            Style::default()
                .fg(Color::Black)
                .bg(ACCENT)
                .add_modifier(Modifier::BOLD),
            "▶",
        )
    } else if focused {
        (
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            "▶",
        )
    } else if ready {
        (Style::default().fg(ACCENT), " ")
    } else {
        (Style::default().fg(DIM), " ")
    };

    Paragraph::new(Line::from(vec![
        Span::raw("   "),
        Span::styled(border.to_string(), Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)),
        Span::raw(" "),
        Span::styled(format!("[{label}]"), style),
        Span::raw("  "),
        if !ready {
            Span::styled(
                "fill all three fields first",
                Style::default().fg(DIM).add_modifier(Modifier::ITALIC),
            )
        } else if focused {
            Span::styled(
                "press Enter to fetch quotes",
                Style::default().fg(DIM),
            )
        } else {
            Span::raw("")
        },
    ]))
}

fn field_line<'a>(label: &'a str, value: String, focused: bool) -> Paragraph<'a> {
    let marker = if focused { "›" } else { " " };
    let marker_style = if focused {
        Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let label_style = Style::default().fg(DIM);
    let value_style = if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };
    Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(marker.to_string(), marker_style),
        Span::raw(" "),
        Span::styled(format!("{label:<8}"), label_style),
        Span::styled(value, value_style),
    ]))
}

fn amount_line<'a>(label: &'a str, value: &'a str, focused: bool) -> Paragraph<'a> {
    let marker = if focused { "›" } else { " " };
    let marker_style = if focused {
        Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(DIM)
    };
    let display = if value.is_empty() {
        "(type an amount)".to_string()
    } else if focused {
        format!("{value}▏")
    } else {
        value.to_string()
    };
    let value_style = if focused {
        Style::default().add_modifier(Modifier::BOLD)
    } else if value.is_empty() {
        Style::default().fg(DIM).add_modifier(Modifier::ITALIC)
    } else {
        Style::default()
    };
    Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(marker.to_string(), marker_style),
        Span::raw(" "),
        Span::styled(format!("{label:<8}"), Style::default().fg(DIM)),
        Span::styled(display, value_style),
    ]))
}

fn token_display(token: Option<&dexquote_core::Token>) -> String {
    match token {
        Some(t) => format!("{} · {}", t.symbol, t.name),
        None => "(press Enter to pick)".into(),
    }
}

fn draw_results(frame: &mut Frame, area: Rect, app: &App) {
    let (title, border_color) = match app.phase {
        Phase::Quoting => {
            let (done, total) = app
                .pending
                .as_ref()
                .map(|p| p.progress())
                .unwrap_or((0, 0));
            (format!(" Fetching {done}/{total} "), PRIMARY)
        }
        Phase::ShowingResults => (" Results ".to_string(), ACCENT),
        Phase::ShowingRoute => (" Route ".to_string(), ACCENT),
        _ => (" Ready ".to_string(), DIM),
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if app.phase == Phase::Quoting {
        if let Some(pending) = &app.pending {
            draw_pending_rows(frame, inner, pending, app.spinner_frame);
            return;
        }
    }

    if let Some(results) = &app.results {
        if let Some(req) = &app.last_request {
            if app.phase == Phase::ShowingRoute {
                draw_route_rows(frame, inner, results, req, app.total_elapsed_ms);
                return;
            }
            draw_result_rows(frame, inner, results, req, app.total_elapsed_ms);
            return;
        }
    }

    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            " Pick a sell token, a buy token, an amount, then press Enter.",
            Style::default().fg(DIM),
        )),
        Line::from(""),
        Line::from(vec![
            Span::raw(" "),
            Span::styled(
                "Tip ",
                Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "— type in the picker to fuzzy-search the token list.",
                Style::default().fg(DIM),
            ),
        ]),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), inner);
}

/// Draw the streaming in-flight panel: one row per backend, showing a
/// spinner next to pending backends and a full result row next to finished
/// ones. The "★ leader" marker lives on whichever completed row currently
/// has the highest amount_out — it hops as new results land.
fn draw_pending_rows(
    frame: &mut Frame,
    area: Rect,
    pending: &super::app::PendingQuote,
    spinner_frame: usize,
) {
    let spinner = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];

    // Running best = max amount_out across already-received success rows.
    let running_best: Option<U256> = pending
        .received
        .iter()
        .filter_map(|slot| slot.as_ref())
        .filter_map(|r| r.quote.as_ref().ok())
        .map(|q| q.amount_out)
        .max();

    let token_out_decimals = pending.request.token_out.decimals;
    let token_out_symbol = &pending.request.token_out.symbol;

    let mut lines = Vec::with_capacity(pending.received.len() + 3);
    lines.push(Line::from(""));

    for (i, slot) in pending.received.iter().enumerate() {
        let name = pending
            .backend_names
            .get(i)
            .copied()
            .unwrap_or("?");
        let line = match slot {
            None => Line::from(vec![
                Span::raw(" "),
                Span::styled(spinner.to_string(), Style::default().fg(PRIMARY)),
                Span::raw("  "),
                Span::styled(
                    format!("{:<10}", name),
                    Style::default().add_modifier(Modifier::DIM),
                ),
                Span::raw("  "),
                Span::styled("fetching…", Style::default().fg(DIM)),
            ]),
            Some(result) => match &result.quote {
                Ok(q) => {
                    let amount = format_amount(q.amount_out, token_out_decimals, 4);
                    let gas = match q.gas_usd {
                        Some(usd) if usd >= 0.01 => format!("gas ${:.2}", usd),
                        Some(_) => "gas <$0.01".to_string(),
                        None => "gas —".to_string(),
                    };
                    let is_best = running_best
                        .map(|b| b == q.amount_out)
                        .unwrap_or(false);

                    let name_style = if is_best {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let amount_style = if is_best {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };
                    let marker = if is_best {
                        Span::styled(
                            " ★ leading",
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        )
                    } else {
                        Span::raw("")
                    };

                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            "✓".to_string(),
                            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("  "),
                        Span::styled(format!("{:<10}", name), name_style),
                        Span::raw("  "),
                        Span::styled(
                            format!("{:>16} {}", amount, token_out_symbol),
                            amount_style,
                        ),
                        Span::raw("   "),
                        Span::styled(gas, Style::default().fg(DIM)),
                        marker,
                    ])
                }
                Err(e) => Line::from(vec![
                    Span::raw(" "),
                    Span::styled(
                        "✗".to_string(),
                        Style::default().fg(DIM),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:<10}", name),
                        Style::default().fg(DIM),
                    ),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:>16}   ", "—"),
                        Style::default().fg(DIM),
                    ),
                    Span::styled(short_error(e), Style::default().fg(DIM)),
                ]),
            },
        };
        lines.push(line);
    }

    let (done, total) = pending.progress();
    let elapsed_ms = pending.started_at.elapsed().as_millis();
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " {} of {} · {} · Esc to cancel",
            done,
            total,
            format_elapsed(elapsed_ms)
        ),
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_result_rows(
    frame: &mut Frame,
    area: Rect,
    results: &[BackendResult],
    request: &QuoteRequest,
    elapsed_ms: u128,
) {
    let successful: Vec<&dexquote_core::Quote> =
        results.iter().filter_map(|r| r.quote.as_ref().ok()).collect();
    let best_amount = successful.iter().map(|q| q.amount_out).max();

    let mut lines = Vec::new();
    lines.push(Line::from(""));

    for r in results {
        let line = match &r.quote {
            Ok(q) => {
                let amount = format_amount(q.amount_out, request.token_out.decimals, 4);
                let gas = match q.gas_usd {
                    Some(usd) if usd >= 0.01 => format!("gas ${:.2}", usd),
                    Some(_) => "gas <$0.01".into(),
                    None => "gas —".into(),
                };
                let is_best = best_amount
                    .map(|b| b == q.amount_out)
                    .unwrap_or(false);
                let marker_span = if is_best {
                    Span::styled(
                        " ★ best",
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
                    )
                } else {
                    Span::raw("")
                };
                let name_span = Span::styled(
                    format!(" {:<10}", r.name),
                    if is_best {
                        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    },
                );
                Line::from(vec![
                    name_span,
                    Span::raw("  "),
                    Span::styled(
                        format!("{:>16} {}", amount, request.token_out.symbol),
                        if is_best {
                            Style::default().add_modifier(Modifier::BOLD)
                        } else {
                            Style::default()
                        },
                    ),
                    Span::raw("   "),
                    Span::styled(gas, Style::default().fg(DIM)),
                    marker_span,
                ])
            }
            Err(e) => Line::from(vec![
                Span::styled(
                    format!(" {:<10}", r.name),
                    Style::default().fg(DIM),
                ),
                Span::raw("  "),
                Span::styled("—".to_string(), Style::default().fg(DIM)),
                Span::raw("  "),
                Span::styled(short_error(e), Style::default().fg(DIM)),
            ]),
        };
        lines.push(line);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(" Fetched in {}", format_elapsed(elapsed_ms)),
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

/// v1.2 Route results renderer. Same source data as `draw_result_rows`
/// (a completed `quote_all`), but the row layout is name | amount |
/// route-path instead of name | amount | gas | best. Rows whose path
/// has ≥3 hops are highlighted in cyan — that's the interesting case
/// where an aggregator split the order across multiple venues. Rows
/// with no route data (on-chain backends, or aggregators that didn't
/// surface path info) show `—` in the path column.
fn draw_route_rows(
    frame: &mut Frame,
    area: Rect,
    results: &[BackendResult],
    request: &QuoteRequest,
    elapsed_ms: u128,
) {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    for r in results {
        let line = match &r.quote {
            Ok(q) => {
                let amount = format_amount(q.amount_out, request.token_out.decimals, 4);
                let path_str = match &q.route {
                    Some(hops) if !hops.is_empty() => hops.join(" → "),
                    _ => "—".to_string(),
                };
                let hop_count = q.route.as_ref().map(|r| r.len()).unwrap_or(0);
                let path_style = if hop_count >= 3 {
                    Style::default().fg(PRIMARY)
                } else {
                    Style::default().fg(DIM)
                };
                Line::from(vec![
                    Span::raw(" "),
                    Span::styled(format!("{:<12}", r.name), Style::default()),
                    Span::raw("  "),
                    Span::styled(
                        format!("{:>16} {}", amount, request.token_out.symbol),
                        Style::default(),
                    ),
                    Span::raw("   "),
                    Span::styled(path_str, path_style),
                ])
            }
            Err(e) => Line::from(vec![
                Span::raw(" "),
                Span::styled(format!("{:<12}", r.name), Style::default().fg(DIM)),
                Span::raw("  "),
                Span::styled("—".to_string(), Style::default().fg(DIM)),
                Span::raw("  "),
                Span::styled(short_error(e), Style::default().fg(DIM)),
            ]),
        };
        lines.push(line);
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!(
            " Fetched in {} · path column shows the venues each backend routed through",
            format_elapsed(elapsed_ms)
        ),
        Style::default().fg(DIM),
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

/// Extract a short but meaningful error description from a
/// `DexQuoteError`. v1.2 improvement: surfaces rate-limit status
/// codes, RPC revert reasons, and HTTP error details so the user
/// can tell WHY a backend failed rather than seeing a generic
/// "http error" or "rpc error".
fn short_error(e: &dexquote_core::DexQuoteError) -> String {
    use dexquote_core::DexQuoteError::*;
    match e {
        NoRoute { .. } => "no route".into(),
        Timeout { ms, .. } => format!("timeout ({ms}ms)"),
        Http { source, .. } => {
            let msg = source.to_string();
            if msg.contains("429") || msg.to_ascii_lowercase().contains("rate") {
                "rate limited (429)".into()
            } else if msg.contains("403") {
                "forbidden (403)".into()
            } else if msg.contains("502") || msg.contains("503") || msg.contains("504") {
                let code = if msg.contains("502") {
                    "502"
                } else if msg.contains("503") {
                    "503"
                } else {
                    "504"
                };
                format!("server error ({code})")
            } else if msg.contains("timed out") || msg.contains("connect") {
                "connection failed".into()
            } else {
                truncate_err(&msg, 40)
            }
        }
        Rpc { source, .. } => {
            let msg = source.to_string();
            if msg.to_ascii_lowercase().contains("rate")
                || msg.contains("429")
                || msg.to_ascii_lowercase().contains("limit")
            {
                "rate limited".into()
            } else if msg.to_ascii_lowercase().contains("revert") {
                "reverted".into()
            } else if msg.to_ascii_lowercase().contains("execution")
                && msg.to_ascii_lowercase().contains("failed")
            {
                "execution failed".into()
            } else {
                truncate_err(&msg, 40)
            }
        }
        Decode { message, .. } => truncate_err(message, 40),
        _ => "error".into(),
    }
}

fn truncate_err(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max.saturating_sub(1)])
    }
}

fn format_elapsed(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", (ms as f64) / 1000.0)
    }
}

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let p = Paragraph::new(Line::from(Span::styled(
        format!(" {}", app.status),
        Style::default().fg(DIM),
    )));
    frame.render_widget(p, area);
}

fn draw_picker_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let overlay = centered_rect(70, 70, area);
    frame.render_widget(Clear, overlay);

    let title = match app.picker_target {
        Field::Sell => " Sell token ",
        Field::Buy => " Buy token ",
        Field::Amount | Field::FindQuote => " Pick token ",
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(title);
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(2), Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let prompt = Paragraph::new(Line::from(vec![
        Span::styled(" Filter: ", Style::default().fg(DIM)),
        Span::styled(
            format!("{}▏", app.picker_filter),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(prompt, rows[0]);

    let matches = app.picker_matches();
    let mut items: Vec<ListItem> = Vec::with_capacity(matches.len() + 1);
    items.push(ListItem::new(Line::from(vec![
        Span::styled(
            "  [+] ",
            Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            "Enter custom token address…",
            Style::default().fg(PRIMARY),
        ),
    ])));
    for token in &matches {
        items.push(ListItem::new(Line::from(vec![
            Span::raw("  "),
            Span::styled(
                format!("{:<10}", token.symbol),
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                token.name.clone(),
                Style::default().fg(DIM),
            ),
        ])));
    }

    let mut state = ListState::default();
    state.select(Some(app.picker_cursor));
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    frame.render_stateful_widget(list, rows[1], &mut state);

    let hint = Paragraph::new(Line::from(Span::styled(
        " type to filter · ↑↓ navigate · Enter select · Esc cancel",
        Style::default().fg(DIM),
    )));
    frame.render_widget(hint, rows[2]);
}

fn draw_custom_address_overlay(frame: &mut Frame, area: Rect, app: &App) {
    let overlay = centered_rect(60, 30, area);
    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(" Custom token address ");
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(2),
            Constraint::Length(1),
        ])
        .split(inner);

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Paste a 0x… ERC20 contract address.",
            Style::default().fg(DIM),
        ))),
        rows[0],
    );

    let input = Paragraph::new(Line::from(vec![
        Span::raw(" "),
        Span::styled(
            format!("{}▏", app.custom_address_input),
            Style::default().add_modifier(Modifier::BOLD),
        ),
    ]));
    frame.render_widget(input, rows[1]);

    if let Some(err) = &app.custom_address_error {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {err}"),
                Style::default().fg(Color::Red),
            ))),
            rows[2],
        );
    }

    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Enter to accept · Esc to go back",
            Style::default().fg(DIM),
        ))),
        rows[3],
    );
}

/// Full keybinding reference modal. Triggered by `?`, dismissed by
/// pressing `?` again or Esc.
fn draw_help_overlay(frame: &mut Frame, area: Rect) {
    // Wider overlay to fit the "Other modes" section's CLI column.
    let overlay = centered_rect(72, 85, area);
    frame.render_widget(Clear, overlay);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PRIMARY))
        .title(" Keybindings — press ? or Esc to dismiss ");
    let inner = block.inner(overlay);
    frame.render_widget(block, overlay);

    let key_style = Style::default().fg(PRIMARY).add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(DIM);

    let entries: &[(&str, &str)] = &[
        ("", ""),
        ("Navigation", ""),
        ("  Tab / Shift-Tab", "cycle between fields"),
        ("  ↑ / ↓", "same as Tab / Shift-Tab"),
        ("", ""),
        ("Actions", ""),
        ("  Enter", "pick token (on field) or fire quote (on button)"),
        ("  Ctrl-Enter", "fire quote from any field"),
        ("  R", "re-run the last quote with the current inputs"),
        ("  S", "swap sell ↔ buy tokens"),
        ("  Y", "copy best quote to clipboard"),
        ("", ""),
        ("Picker", ""),
        ("  (type)", "fuzzy-filter the token list"),
        ("  Enter", "select highlighted token"),
        ("  Esc", "cancel and return to fields"),
        ("  [+] Custom", "enter a raw 0x… / base58 address"),
        ("", ""),
        ("Global", ""),
        ("  ?", "toggle this help overlay"),
        ("  q / Esc", "quit"),
        ("  Ctrl-C", "quit immediately"),
        ("", ""),
        ("Other modes", "(run bare `dexquote` for the main menu)"),
        ("  Depth", "dexquote depth WETH USDC 1"),
        ("  Route", "dexquote route WETH USDC 1"),
        ("  Benchmark", "dexquote benchmark"),
        ("  Doctor", "dexquote doctor"),
        ("  Tokens", "dexquote tokens"),
        ("  History", "dexquote history"),
    ];

    let lines: Vec<Line> = entries
        .iter()
        .map(|(key, desc)| {
            if key.is_empty() && desc.is_empty() {
                Line::from("")
            } else if desc.is_empty() {
                // Section header
                Line::from(Span::styled(
                    format!(" {key}"),
                    Style::default().add_modifier(Modifier::BOLD),
                ))
            } else {
                Line::from(vec![
                    Span::styled(format!("{:<20}", key), key_style),
                    Span::styled((*desc).to_string(), desc_style),
                ])
            }
        })
        .collect();

    let p = Paragraph::new(lines);
    frame.render_widget(p, inner);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
