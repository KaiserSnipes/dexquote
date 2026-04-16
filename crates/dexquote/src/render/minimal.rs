//! Single-line output for shell scripting.
//!
//! Prints one line describing the best quote only, tab-separated so it pipes
//! cleanly into `awk` / `cut`. Empty string when every backend failed.

use dexquote_core::token::format_amount;
use dexquote_core::{BackendResult, QuoteRequest};

pub fn render_minimal(results: &[BackendResult], request: &QuoteRequest) -> String {
    let best = results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok())
        .max_by_key(|q| q.amount_out);

    match best {
        Some(q) => format!(
            "{}\t{}\t{}",
            q.backend,
            format_amount(q.amount_out, request.token_out.decimals, 6),
            request.token_out.symbol,
        ),
        None => String::new(),
    }
}
