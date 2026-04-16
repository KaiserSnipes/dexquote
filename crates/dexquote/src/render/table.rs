//! Hand-rolled table formatter. No `comfy-table`, no column-width drift.
//!
//! Four visual columns: `backend`, `amount`, `gas`, `marker`. Each row is
//! built from fixed-width cells; the marker column is a single final word
//! (`★ best`, `thin liq`, or empty) that sits flush against the row.

use crate::theme::Theme;
use colored::Colorize;
use dexquote_core::token::format_amount;
use alloy::primitives::ruint::ParseError;
use dexquote_core::{BackendResult, Chain, DexQuoteError, Quote, QuoteRequest, Token, U256};

#[allow(dead_code)]
type _IgnoreForLint = ParseError;
use unicode_width::UnicodeWidthStr;

// Canonical native Arbitrum USDC address, lowercased for comparison.
const USDC_NATIVE_ARBITRUM: &str = "0xaf88d065e77c8cc2239327c5edb3a432268e5831";

pub struct RenderInput<'a> {
    pub request: &'a QuoteRequest,
    pub results: &'a [BackendResult],
    pub total_elapsed_ms: u128,
    pub theme: Theme,
    /// Optional reference to the prior quote for this exact pair; when
    /// present, the footer shows a "+0.04% since 3m ago" delta indicator.
    /// `None` on the very first quote of a pair or when history is disabled.
    pub prior: Option<PriorQuoteRef>,
}

/// Minimal projection of a history entry the renderer needs to compute a
/// delta. Deliberately decoupled from `history::HistoryEntry` so the
/// render module doesn't depend on history schema changes.
pub struct PriorQuoteRef {
    pub ts: u64,
    pub sell_decimals: u8,
    pub buy_decimals: u8,
    pub amount_in_base_units: String,
    pub best_amount_out_base_units: String,
}

#[derive(Debug, Clone)]
struct Row {
    backend: String,
    amount: String,
    net: String,
    gas: String,
    marker: Marker,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Marker {
    None,
    Best,
    BestNet,
    ThinLiquidity,
    DeadPool,
    Error(String),
}

pub fn render_human(input: &RenderInput) -> String {
    let rows = build_rows(input.results, input.request, input.theme);
    let tip = compute_usdc_tip(input);

    let mut out = String::new();

    // Header — e.g. "1.00 WETH → USDC on Arbitrum"
    out.push('\n');
    out.push_str(&format_header(input));
    out.push('\n');

    // Column widths from row content plus separator lengths.
    let (bw, aw, nw, gw) = column_widths(&rows);
    let total_width = bw + 2 + aw + 2 + nw + 3 + gw + 2 + 10;
    let total_width = total_width.max(header_width(input));

    let sep = separator_line(total_width, input.theme);
    out.push_str(&sep);
    out.push('\n');

    for row in &rows {
        out.push(' ');
        out.push_str(&pad_right(&row.backend, bw));
        out.push_str("  ");
        out.push_str(&pad_left(&row.amount, aw));
        out.push_str("  ");
        out.push_str(&pad_left(&row.net, nw));
        out.push_str("   ");
        out.push_str(&pad_right(&row.gas, gw));
        out.push_str("  ");
        out.push_str(&format_marker(row, input.theme));
        out.push('\n');
    }

    out.push_str(&sep);
    out.push('\n');
    out.push_str(&format_footer(input));
    out.push('\n');
    if let Some(tip) = tip {
        out.push('\n');
        out.push_str(&tip);
        out.push('\n');
    }
    out
}

fn build_rows(results: &[BackendResult], request: &QuoteRequest, theme: Theme) -> Vec<Row> {
    let successful: Vec<&Quote> = results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok())
        .collect();

    let best_amount = successful.iter().map(|q| q.amount_out).max();
    let median = median_amount(&successful);

    // Precompute per-quote net amounts so we can pick the best-net
    // winner alongside the best-gross winner. `net` is only computable
    // when we can price the output token in USD — either it's a
    // stablecoin (trivial) or the input is a stablecoin and we can
    // derive output/USD from the effective rate.
    let nets: Vec<Option<U256>> = results
        .iter()
        .map(|r| {
            r.quote
                .as_ref()
                .ok()
                .and_then(|q| compute_net_amount(q, request))
        })
        .collect();

    // Best-net winner is the highest net across healthy quotes
    // (excluding thin and dead pools which are already outliers).
    let best_net = nets
        .iter()
        .zip(results.iter())
        .filter_map(|(net, r)| {
            let n = (*net)?;
            let q = r.quote.as_ref().ok()?;
            if is_thin_liquidity(q.amount_out, median) || is_dead_pool(q.amount_out, median) {
                return None;
            }
            Some((r.name, n))
        })
        .max_by_key(|(_, n)| *n)
        .map(|(name, _)| name);

    results
        .iter()
        .zip(nets.iter())
        .map(|(result, net_opt)| match &result.quote {
            Ok(quote) => {
                let amount_str = format!(
                    "{} {}",
                    format_amount(quote.amount_out, request.token_out.decimals, 4),
                    request.token_out.symbol
                );
                let net_str = match net_opt {
                    Some(net) => {
                        format_amount(*net, request.token_out.decimals, 4)
                    }
                    None => theme.em_dash().to_string(),
                };
                let gas_str = format_gas(quote);
                let is_best = best_amount
                    .map(|best| quote.amount_out == best)
                    .unwrap_or(false);
                let is_best_net =
                    best_net.map(|n| n == result.name).unwrap_or(false) && !is_best;
                let is_dead = is_dead_pool(quote.amount_out, median);
                let is_thin = is_thin_liquidity(quote.amount_out, median);

                let marker = if is_dead {
                    Marker::DeadPool
                } else if is_best {
                    Marker::Best
                } else if is_best_net {
                    Marker::BestNet
                } else if is_thin {
                    Marker::ThinLiquidity
                } else {
                    Marker::None
                };

                Row {
                    backend: backend_display(result.name, theme),
                    amount: amount_str,
                    net: net_str,
                    gas: gas_str,
                    marker,
                }
            }
            Err(_) => Row {
                backend: backend_display(result.name, theme),
                amount: theme.em_dash().to_string(),
                net: theme.em_dash().to_string(),
                gas: theme.em_dash().to_string(),
                marker: Marker::Error(short_error_kind(&result.quote)),
            },
        })
        .collect()
}

/// Compute an after-gas net amount_out, expressed in output-token base
/// units. Returns None when we can't price the output token reliably.
///
/// Pricing rules:
/// 1. If `token_out` is a known stablecoin: `output_usd_price = 1.0`
/// 2. Else if `token_in` is a known stablecoin: derive
///    `output_usd_price = amount_in_usd / amount_out_human`, which
///    gives a backend-specific effective price that's "what this
///    backend thinks the output is worth in USD."
/// 3. Else: no reliable USD anchor, return None.
fn compute_net_amount(quote: &Quote, request: &QuoteRequest) -> Option<U256> {
    let gas_usd = quote.gas_usd?;
    if gas_usd <= 0.0 {
        return Some(quote.amount_out);
    }

    let output_usd = output_usd_price_for(quote, request)?;
    if output_usd <= 0.0 {
        return None;
    }

    // Gas as output-token base units:
    //   gas_usd / output_usd = gas in output-token "human" units
    //   × 10^decimals          = base units
    let gas_in_output_human = gas_usd / output_usd;
    let scale = 10f64.powi(request.token_out.decimals as i32);
    let gas_base: u128 = (gas_in_output_human * scale) as u128;
    let gas_u256 = U256::from(gas_base);

    Some(quote.amount_out.saturating_sub(gas_u256))
}

fn output_usd_price_for(quote: &Quote, request: &QuoteRequest) -> Option<f64> {
    if is_stablecoin(&request.token_out.symbol) {
        return Some(1.0);
    }
    // Derive via input side: if input is a stablecoin, the amount_in
    // value in USD equals its human amount, so
    //   output_usd = amount_in_human / amount_out_human
    if is_stablecoin(&request.token_in.symbol) {
        let in_human = amount_to_f64(request.amount_in, request.token_in.decimals);
        let out_human = amount_to_f64(quote.amount_out, request.token_out.decimals);
        if in_human > 0.0 && out_human > 0.0 {
            return Some(in_human / out_human);
        }
    }
    None
}

fn is_stablecoin(symbol: &str) -> bool {
    matches!(
        symbol.to_ascii_uppercase().as_str(),
        "USDC"
            | "USDT"
            | "DAI"
            | "USDC.E"
            | "USDBC"
            | "FRAX"
            | "LUSD"
            | "CRVUSD"
            | "USDE"
            | "SUSDE"
            | "PYUSD"
            | "GHO"
            | "USDS"
            | "USDSM"
            | "SCRVUSD"
    )
}

fn backend_display(name: &str, _theme: Theme) -> String {
    name.to_string()
}

pub(crate) fn median_amount(quotes: &[&Quote]) -> Option<U256> {
    if quotes.is_empty() {
        return None;
    }
    let mut sorted: Vec<U256> = quotes.iter().map(|q| q.amount_out).collect();
    sorted.sort();
    Some(sorted[sorted.len() / 2])
}

/// Mark a quote as thin-liquidity when its amount is less than 90% of the
/// median across successful backends. A >10% gap almost always means either
/// a dead pool or wildly thin liquidity getting crushed by price impact.
pub(crate) fn is_thin_liquidity(amount: U256, median: Option<U256>) -> bool {
    let Some(median) = median else {
        return false;
    };
    if median.is_zero() || amount >= median {
        return false;
    }
    // amount < median * 0.9  <=>  amount * 10 < median * 9
    amount.saturating_mul(U256::from(10u64)) < median.saturating_mul(U256::from(9u64))
}

/// A quote is "dead pool" when it's more than 99% below the median of
/// successful backends — i.e. a factor of 100 or more off. Thin-liq
/// catches the 10–90% range; this catches the tail past that, where a
/// Curve pool is listed in the registry but every `get_dy` call returns
/// dust regardless of input size.
pub(crate) fn is_dead_pool(amount: U256, median: Option<U256>) -> bool {
    let Some(median) = median else {
        return false;
    };
    if median.is_zero() || amount >= median {
        return false;
    }
    // amount < median / 100  <=>  amount * 100 < median
    amount.saturating_mul(U256::from(100u64)) < median
}

fn format_gas(quote: &Quote) -> String {
    match quote.gas_usd {
        Some(usd) if usd >= 0.01 => format!("gas ${:.2}", usd),
        Some(_) => "gas <$0.01".into(),
        None => "gas —".into(),
    }
}

fn short_error_kind(result: &Result<Quote, dexquote_core::DexQuoteError>) -> String {
    use dexquote_core::DexQuoteError::*;
    match result {
        Err(NoRoute { .. }) => "no route".into(),
        Err(Timeout { ms, .. }) => format!("timeout ({ms}ms)"),
        Err(Http { source, .. }) => {
            let msg = source.to_string();
            if msg.contains("429") || msg.to_ascii_lowercase().contains("rate") {
                "rate limited (429)".into()
            } else {
                "http error".into()
            }
        }
        Err(Rpc { source, .. }) => {
            let msg = source.to_string();
            if msg.to_ascii_lowercase().contains("rate") || msg.contains("429") {
                "rate limited".into()
            } else if msg.to_ascii_lowercase().contains("revert") {
                "reverted".into()
            } else {
                "rpc error".into()
            }
        }
        Err(Decode { .. }) => "decode error".into(),
        _ => "error".into(),
    }
}

fn format_marker(row: &Row, theme: Theme) -> String {
    let plain = match &row.marker {
        Marker::None => return String::new(),
        Marker::Best => format!("{} best", theme.star()),
        Marker::BestNet => format!("{} best net", theme.star()),
        Marker::ThinLiquidity => "thin liq".to_string(),
        Marker::DeadPool => "dead pool".to_string(),
        Marker::Error(kind) => kind.clone(),
    };
    if !theme.color {
        return plain;
    }
    match row.marker {
        Marker::None => String::new(),
        Marker::Best => plain.green().bold().to_string(),
        Marker::BestNet => plain.cyan().bold().to_string(),
        Marker::ThinLiquidity => plain.yellow().to_string(),
        Marker::DeadPool => plain.red().dimmed().to_string(),
        Marker::Error(_) => plain.bright_black().to_string(),
    }
}

fn column_widths(rows: &[Row]) -> (usize, usize, usize, usize) {
    let bw = rows.iter().map(|r| width(&r.backend)).max().unwrap_or(0);
    let aw = rows.iter().map(|r| width(&r.amount)).max().unwrap_or(0);
    let nw = rows.iter().map(|r| width(&r.net)).max().unwrap_or(0);
    let gw = rows.iter().map(|r| width(&r.gas)).max().unwrap_or(0);
    (bw.max(9), aw.max(14), nw.max(10), gw.max(10))
}

fn width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

fn pad_right(s: &str, w: usize) -> String {
    let cur = width(s);
    if cur >= w {
        return s.to_string();
    }
    format!("{}{}", s, " ".repeat(w - cur))
}

fn pad_left(s: &str, w: usize) -> String {
    let cur = width(s);
    if cur >= w {
        return s.to_string();
    }
    format!("{}{}", " ".repeat(w - cur), s)
}

fn separator_line(width: usize, theme: Theme) -> String {
    theme.sep_char().to_string().repeat(width.min(120))
}

fn format_header(input: &RenderInput) -> String {
    let amount_h = format_amount(
        input.request.amount_in,
        input.request.token_in.decimals,
        6,
    );
    let line = format!(
        " {} {} {} {} on {}",
        amount_h,
        input.request.token_in.symbol,
        input.theme.arrow(),
        input.request.token_out.symbol,
        chain_name(input.request.chain),
    );
    if input.theme.color {
        line.bold().to_string()
    } else {
        line
    }
}

fn header_width(input: &RenderInput) -> usize {
    let amount_h = format_amount(
        input.request.amount_in,
        input.request.token_in.decimals,
        6,
    );
    width(&format!(
        " {} {} {} {} on {}",
        amount_h,
        input.request.token_in.symbol,
        input.theme.arrow(),
        input.request.token_out.symbol,
        chain_name(input.request.chain),
    ))
}

fn chain_name(chain: Chain) -> &'static str {
    chain.name()
}

fn format_footer(input: &RenderInput) -> String {
    let successful: Vec<&Quote> = input
        .results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok())
        .collect();

    let timing = format_elapsed(input.total_elapsed_ms);

    if successful.is_empty() {
        let line = format!(" Fetched in {timing}");
        return if input.theme.color {
            line.dimmed().to_string()
        } else {
            line
        };
    }

    // Effective rate line — how many units of the output token you get per
    // one unit of the input token, according to the winning backend. This
    // is the actionable number humans use to decide.
    let best_quote = successful.iter().max_by_key(|q| q.amount_out).copied();
    let rate_line = best_quote.map(|q| format_rate_line(q, input));

    if successful.len() < 2 {
        let mut out = String::new();
        if let Some(line) = rate_line {
            out.push_str(&line);
            out.push('\n');
        }
        let timing_line = format!(" Fetched in {timing}");
        if input.theme.color {
            out.push_str(&timing_line.dimmed().to_string());
        } else {
            out.push_str(&timing_line);
        }
        // Delta line works with any number of successful backends.
        if let Some(line) = format_delta_line(input) {
            out.push('\n');
            if input.theme.color {
                out.push_str(&line.dimmed().to_string());
            } else {
                out.push_str(&line);
            }
        }
        return out;
    }

    // Spread is computed across healthy backends only — non-thin-liquidity
    // AND non-dead-pool. Including either class of outlier would make the
    // number useless for deciding which DEX to trade on: the real question
    // is "is there real arb between the venues that actually have liquidity?"
    let median = median_amount(&successful).unwrap_or_default();
    let healthy: Vec<U256> = successful
        .iter()
        .map(|q| q.amount_out)
        .filter(|a| !is_thin_liquidity(*a, Some(median)))
        .filter(|a| !is_dead_pool(*a, Some(median)))
        .collect();

    let (spread_pct, best_name, delta_vs_median) = if healthy.len() >= 2 {
        let max = healthy.iter().copied().max().unwrap_or_default();
        let min = healthy.iter().copied().min().unwrap_or_default();
        let best = successful
            .iter()
            .filter(|q| !is_thin_liquidity(q.amount_out, Some(median)))
            .filter(|q| !is_dead_pool(q.amount_out, Some(median)))
            .max_by_key(|q| q.amount_out)
            .map(|q| q.backend)
            .unwrap_or("?");
        let healthy_median = {
            let mut sorted = healthy.clone();
            sorted.sort();
            sorted[sorted.len() / 2]
        };
        (percent_diff(max, min), best, max.saturating_sub(healthy_median))
    } else {
        // Fall back to the full set when fewer than two healthy quotes.
        let amounts: Vec<U256> = successful.iter().map(|q| q.amount_out).collect();
        let max = amounts.iter().copied().max().unwrap_or_default();
        let min = amounts.iter().copied().min().unwrap_or_default();
        let best = successful
            .iter()
            .max_by_key(|q| q.amount_out)
            .map(|q| q.backend)
            .unwrap_or("?");
        (percent_diff(max, min), best, max.saturating_sub(median))
    };

    let delta_str = format_amount(delta_vs_median, input.request.token_out.decimals, 2);
    let dot = input.theme.dot();

    let stats_line = format!(
        " Spread {:.2}%  {}  Best {}  (+{} {} vs median)  {}  {}",
        spread_pct,
        dot,
        best_name,
        delta_str,
        input.request.token_out.symbol,
        dot,
        timing,
    );

    let delta_line = format_delta_line(input);

    let mut out = String::new();
    if let Some(line) = rate_line {
        out.push_str(&line);
        out.push('\n');
    }
    if input.theme.color {
        out.push_str(&stats_line.dimmed().to_string());
    } else {
        out.push_str(&stats_line);
    }
    if let Some(line) = delta_line {
        out.push('\n');
        if input.theme.color {
            out.push_str(&line.dimmed().to_string());
        } else {
            out.push_str(&line);
        }
    }
    out
}

/// Compute the effective-rate delta between the current quote and the
/// prior one for the same pair. Rates are compared (not absolute amounts)
/// so that different amounts still produce a meaningful percentage — the
/// user ran "1 WETH → USDC" yesterday and "2 WETH → USDC" today, but the
/// per-unit price is still comparable.
fn format_delta_line(input: &RenderInput) -> Option<String> {
    let prior = input.prior.as_ref()?;
    let successful: Vec<&Quote> = input
        .results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok())
        .collect();
    let current_best = successful.iter().max_by_key(|q| q.amount_out)?;

    let current_rate = rate_from_u256(
        input.request.amount_in,
        current_best.amount_out,
        input.request.token_in.decimals,
        input.request.token_out.decimals,
    );

    let prior_amount_in = U256::from_str_radix(&prior.amount_in_base_units, 10).ok()?;
    let prior_amount_out = U256::from_str_radix(&prior.best_amount_out_base_units, 10).ok()?;
    let prior_rate = rate_from_u256(
        prior_amount_in,
        prior_amount_out,
        prior.sell_decimals,
        prior.buy_decimals,
    );

    if prior_rate <= 0.0 || current_rate <= 0.0 {
        return None;
    }

    let pct = (current_rate - prior_rate) / prior_rate * 100.0;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(prior.ts);
    let ago = now.saturating_sub(prior.ts);
    let ago_label = format_relative_age(ago);

    let sign = if pct >= 0.0 { "+" } else { "" };
    Some(format!(
        " Δ vs last: {sign}{pct:.3}%  ({ago_label})",
    ))
}

/// Convert `amount_out / amount_in` into a per-unit rate using the stored
/// decimals. Returns 0.0 on degenerate inputs rather than erroring.
fn rate_from_u256(
    amount_in: U256,
    amount_out: U256,
    in_decimals: u8,
    out_decimals: u8,
) -> f64 {
    let in_f = amount_to_f64(amount_in, in_decimals);
    let out_f = amount_to_f64(amount_out, out_decimals);
    if in_f == 0.0 {
        0.0
    } else {
        out_f / in_f
    }
}

fn format_relative_age(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Compute "X.YZ BUY per 1 SELL" at the best backend's rate. The ratio is
/// computed in f64 for display only; callers who need exact precision
/// should read `amount_out` directly.
fn format_rate_line(best: &Quote, input: &RenderInput) -> String {
    let amount_in_f = amount_to_f64(input.request.amount_in, input.request.token_in.decimals);
    let amount_out_f = amount_to_f64(best.amount_out, input.request.token_out.decimals);
    if amount_in_f <= 0.0 {
        return String::new();
    }
    let rate = amount_out_f / amount_in_f;
    let rate_str = format_rate_number(rate);
    let line = format!(
        " Best rate: {} {} per 1 {}",
        rate_str, input.request.token_out.symbol, input.request.token_in.symbol
    );
    if input.theme.color {
        line.bold().to_string()
    } else {
        line
    }
}

fn amount_to_f64(amount: U256, decimals: u8) -> f64 {
    // Split to keep precision for large values.
    let s = amount.to_string();
    let d = decimals as usize;
    if s.len() <= d {
        let padded = format!("{:0>width$}", s, width = d);
        format!("0.{padded}").parse().unwrap_or(0.0)
    } else {
        let split = s.len() - d;
        let int = &s[..split];
        let frac = &s[split..];
        if frac.is_empty() {
            int.parse().unwrap_or(0.0)
        } else {
            format!("{int}.{frac}").parse().unwrap_or(0.0)
        }
    }
}

fn format_rate_number(rate: f64) -> String {
    // Adaptive precision: very small rates (like 0.00043 ETH per USDC) need
    // more decimals; very large rates (like 2,322 USDC per ETH) need comma
    // grouping and few decimals.
    if rate >= 1000.0 {
        format_with_commas(rate, 2)
    } else if rate >= 1.0 {
        format!("{:.4}", rate).trim_end_matches('0').trim_end_matches('.').to_string()
    } else if rate >= 0.0001 {
        format!("{:.6}", rate).trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        format!("{:e}", rate)
    }
}

fn format_with_commas(value: f64, decimals: usize) -> String {
    let raw = format!("{:.*}", decimals, value);
    let (int_part, frac_part) = match raw.split_once('.') {
        Some((i, f)) => (i, Some(f)),
        None => (raw.as_str(), None),
    };
    let bytes = int_part.as_bytes();
    let len = bytes.len();
    let mut grouped = String::with_capacity(len + len / 3);
    for (i, &b) in bytes.iter().enumerate() {
        if i != 0 && (len - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(b as char);
    }
    match frac_part {
        Some(f) if !f.chars().all(|c| c == '0') => format!("{grouped}.{f}"),
        _ => grouped,
    }
}

fn percent_diff(max: U256, min: U256) -> f64 {
    if max.is_zero() {
        return 0.0;
    }
    let diff = max.saturating_sub(min);
    let max_f = u256_to_f64(max);
    let diff_f = u256_to_f64(diff);
    if max_f == 0.0 {
        0.0
    } else {
        (diff_f / max_f) * 100.0
    }
}

fn u256_to_f64(value: U256) -> f64 {
    let s = value.to_string();
    s.parse().unwrap_or(0.0)
}

fn format_elapsed(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else {
        format!("{:.1}s", (ms as f64) / 1000.0)
    }
}

/// Render JUST the footer line (spread/best/timing). Used by streaming mode
/// after the per-backend spinner lines have already been drawn.
pub fn render_footer_only(input: &RenderInput) -> String {
    let mut out = String::new();
    out.push('\n');
    out.push_str(&format_footer(input));
    out.push('\n');
    if let Some(tip) = compute_usdc_tip(input) {
        out.push('\n');
        out.push_str(&tip);
        out.push('\n');
    }
    out
}

/// When a request uses native USDC AND a pool-based DEX (SushiV2 or
/// TraderJoe) flags thin_liq or NoRoute, suggest that the user try USDC.e
/// instead — those DEXs historically built their pools in the bridged
/// token. This is the #1 "why is SushiV2 returning a garbage number" FAQ
/// for anyone new to Arbitrum.
fn compute_usdc_tip(input: &RenderInput) -> Option<String> {
    let in_addr = format!("{:?}", input.request.token_in.address).to_ascii_lowercase();
    let out_addr = format!("{:?}", input.request.token_out.address).to_ascii_lowercase();
    let uses_native_usdc =
        in_addr == USDC_NATIVE_ARBITRUM || out_addr == USDC_NATIVE_ARBITRUM;
    if !uses_native_usdc {
        return None;
    }

    // Look at SushiV2 + TraderJoe specifically.
    let successful: Vec<&Quote> = input
        .results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok())
        .collect();
    let median = median_amount(&successful).unwrap_or_default();

    let pool_based_backends = ["SushiV2", "TraderJoe"];
    let suspicious = input.results.iter().any(|r| {
        if !pool_based_backends.contains(&r.name) {
            return false;
        }
        match &r.quote {
            Err(DexQuoteError::NoRoute { .. }) => true,
            Ok(q) => {
                is_thin_liquidity(q.amount_out, Some(median))
                    || is_dead_pool(q.amount_out, Some(median))
            }
            _ => false,
        }
    });
    if !suspicious {
        return None;
    }

    // Replace USDC with USDC.e in the suggested command for whichever side
    // is native USDC.
    let (in_sym, out_sym) = (
        if in_addr == USDC_NATIVE_ARBITRUM {
            "USDC.e"
        } else {
            &input.request.token_in.symbol[..]
        },
        if out_addr == USDC_NATIVE_ARBITRUM {
            "USDC.e"
        } else {
            &input.request.token_out.symbol[..]
        },
    );
    let amount_display = format_amount(
        input.request.amount_in,
        input.request.token_in.decimals,
        6,
    );

    let tip_body = format!(
        " tip: SushiV2 and TraderJoe built their Arbitrum pools in bridged USDC.e,\n \
         so they look dead for native USDC pairs. Try:\n \
           dexquote {} {} {}",
        in_sym, out_sym, amount_display
    );
    if input.theme.color {
        Some(tip_body.yellow().to_string())
    } else {
        Some(tip_body)
    }
}

/// Render the bundled token registry. Used by `dexquote tokens`.
pub fn render_token_list(tokens: &[Token], theme: Theme) -> String {
    let mut out = String::new();
    out.push_str(&format!("\n Bundled tokens ({}):\n\n", tokens.len()));

    let sym_w = tokens.iter().map(|t| t.symbol.len()).max().unwrap_or(6);
    let name_w = tokens.iter().map(|t| t.name.len()).max().unwrap_or(12);

    for t in tokens {
        out.push(' ');
        out.push_str(&pad_right(&t.symbol, sym_w));
        out.push_str("  ");
        out.push_str(&pad_right(&t.name, name_w));
        out.push_str("  ");
        let addr = t.address.display_string();
        if theme.color {
            out.push_str(&addr.dimmed().to_string());
        } else {
            out.push_str(&addr);
        }
        out.push_str(&format!("  {}d", t.decimals));
        out.push('\n');
    }
    out.push('\n');
    out.push_str(
        " Any raw ERC20 address or SPL mint also works — pass the 0x… or base58 directly.\n",
    );
    out
}
