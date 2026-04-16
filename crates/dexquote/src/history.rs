//! Quote history — append-only JSONL log of every non-watch quote.
//!
//! Lives at the OS-appropriate data dir:
//!   - Linux/macOS: `$XDG_DATA_HOME/dexquote/history.jsonl` (or `~/.local/share/...`)
//!   - Windows:     `%LOCALAPPDATA%\dexquote\history.jsonl`
//!
//! Each line is a single JSON object. Newer entries are always appended at
//! the end; the file is never rewritten, so reading is cheap even for long
//! histories. `dexquote last` grabs the tail line, `dexquote history`
//! streams all lines and filters.

use crate::error::{CliError, CliResult};
use dexquote_core::{BackendResult, Chain, QuoteRequest};
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_HISTORY_LINES: usize = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts: u64, // unix seconds
    pub chain: String,
    pub sell_symbol: String,
    pub sell_address: String,
    pub sell_decimals: u8,
    pub buy_symbol: String,
    pub buy_address: String,
    pub buy_decimals: u8,
    pub amount_in: String, // base units as decimal string
    pub best_backend: Option<String>,
    pub best_amount_out: Option<String>,
    pub spread_pct: Option<f64>,
    pub elapsed_ms: u128,
}

impl HistoryEntry {
    /// Convert the stored base-unit `amount_in` string into a human
    /// readable decimal, trimming trailing zeros. Used by both the
    /// CLI `history` subcommand and the in-TUI history view (v1.2).
    pub fn amount_in_human(&self) -> String {
        format_amount_from_base(&self.amount_in, self.sell_decimals)
    }

    pub fn from_quote(
        request: &QuoteRequest,
        results: &[BackendResult],
        elapsed_ms: u128,
    ) -> Self {
        let successful: Vec<&dexquote_core::Quote> =
            results.iter().filter_map(|r| r.quote.as_ref().ok()).collect();
        let best = successful.iter().max_by_key(|q| q.amount_out);
        let spread_pct = if successful.len() >= 2 {
            let max = successful.iter().map(|q| q.amount_out).max().unwrap_or_default();
            let min = successful.iter().map(|q| q.amount_out).min().unwrap_or_default();
            let max_f: f64 = max.to_string().parse().unwrap_or(0.0);
            let min_f: f64 = min.to_string().parse().unwrap_or(0.0);
            if max_f > 0.0 {
                Some((max_f - min_f) / max_f * 100.0)
            } else {
                None
            }
        } else {
            None
        };

        Self {
            ts: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            chain: request.chain.name().to_string(),
            sell_symbol: request.token_in.symbol.clone(),
            sell_address: format!("{:?}", request.token_in.address),
            sell_decimals: request.token_in.decimals,
            buy_symbol: request.token_out.symbol.clone(),
            buy_address: format!("{:?}", request.token_out.address),
            buy_decimals: request.token_out.decimals,
            amount_in: request.amount_in.to_string(),
            best_backend: best.map(|q| q.backend.to_string()),
            best_amount_out: best.map(|q| q.amount_out.to_string()),
            spread_pct,
            elapsed_ms,
        }
    }
}

pub fn history_path() -> CliResult<PathBuf> {
    let base = dirs::data_local_dir()
        .or_else(dirs::data_dir)
        .ok_or_else(|| {
            CliError::setup(
                "could not determine the user data directory".to_string(),
                "history is disabled; check your $XDG_DATA_HOME env var".to_string(),
            )
        })?;
    Ok(base.join("dexquote").join("history.jsonl"))
}

/// Best-effort append. Errors are swallowed so a history-write failure never
/// blocks the actual quote the user ran. A startup warning would be noisy;
/// if the user cares, `dexquote history` will return an error with the real
/// file-system issue.
pub fn record(entry: &HistoryEntry) {
    let Ok(path) = history_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Find the most recent history entry for a specific pair on a specific
/// chain. Used by the renderer to show a "delta vs last quote" line.
/// Address matching is case-insensitive so entries written with
/// checksummed addresses still match when compared against lowercased ones.
pub fn find_last_matching(
    chain: &str,
    sell_address: &str,
    buy_address: &str,
) -> Option<HistoryEntry> {
    let all = read_all().ok()?;
    all.into_iter().rev().find(|entry| {
        entry.chain.eq_ignore_ascii_case(chain)
            && entry.sell_address.eq_ignore_ascii_case(sell_address)
            && entry.buy_address.eq_ignore_ascii_case(buy_address)
    })
}

/// Read every history entry from disk. Returns oldest-first.
pub fn read_all() -> CliResult<Vec<HistoryEntry>> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path).map_err(|e| {
        CliError::setup(
            format!("could not read history file {}", path.display()),
            format!("check file permissions: {e}"),
        )
    })?;
    let mut entries = Vec::new();
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(entry) = serde_json::from_str::<HistoryEntry>(&line) {
            entries.push(entry);
        }
    }
    // Drop oldest entries beyond the cap so infinite CI loops don't grow the
    // file unbounded. We truncate at read-time only; the file on disk is
    // never rewritten.
    if entries.len() > MAX_HISTORY_LINES {
        let drop_count = entries.len() - MAX_HISTORY_LINES;
        entries.drain(0..drop_count);
    }
    Ok(entries)
}

/// Convert a history entry back into a `QuoteRequest` suitable for replay.
/// Returns `None` if the entry refers to a chain we no longer understand.
pub fn entry_to_request(entry: &HistoryEntry) -> Option<QuoteReplay> {
    let chain = Chain::parse(&entry.chain.to_ascii_lowercase()).ok()?;
    Some(QuoteReplay {
        chain,
        sell_input: entry.sell_address.clone(),
        buy_input: entry.buy_address.clone(),
        amount_human: format_amount_from_base(&entry.amount_in, entry.sell_decimals),
    })
}

#[derive(Debug, Clone)]
pub struct QuoteReplay {
    pub chain: Chain,
    pub sell_input: String,
    pub buy_input: String,
    pub amount_human: String,
}

/// Render a Unix timestamp as a "Ns/Nm/Nh/Nd ago" label relative
/// to now. Used by both the CLI history subcommand and the in-TUI
/// history view. Keeps the two renderers identical so swapping
/// between them looks seamless.
pub fn format_relative_ts(ts: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let delta = now.saturating_sub(ts);
    if delta < 60 {
        format!("{}s ago", delta)
    } else if delta < 3600 {
        format!("{}m ago", delta / 60)
    } else if delta < 86400 {
        format!("{}h ago", delta / 3600)
    } else {
        format!("{}d ago", delta / 86400)
    }
}

fn format_amount_from_base(base_units: &str, decimals: u8) -> String {
    let d = decimals as usize;
    if base_units.len() <= d {
        let padded = format!("{:0>width$}", base_units, width = d);
        format!("0.{}", padded.trim_end_matches('0'))
            .trim_end_matches('.')
            .to_string()
    } else {
        let split = base_units.len() - d;
        let int = &base_units[..split];
        let frac = &base_units[split..];
        let frac_trimmed = frac.trim_end_matches('0');
        if frac_trimmed.is_empty() {
            int.to_string()
        } else {
            format!("{int}.{frac_trimmed}")
        }
    }
}
