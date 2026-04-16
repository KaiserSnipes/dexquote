//! Route-mode renderer: shows each backend's `amount_out` next to the
//! multi-hop path it took. Backends without route data (all on-chain
//! backends — they ARE the route — and any aggregator that didn't
//! surface path info) show a `—` in the path column.

use crate::theme::Theme;
use colored::Colorize;
use dexquote_core::token::format_amount;
use dexquote_core::{BackendResult, QuoteRequest};
use std::time::Duration;

pub fn render_route(
    request: &QuoteRequest,
    results: &[BackendResult],
    elapsed: Duration,
    theme: Theme,
) -> String {
    let mut out = String::new();

    let amount_h = format_amount(request.amount_in, request.token_in.decimals, 6);
    let header = format!(
        "\n {} {} → {} route on {}\n",
        amount_h,
        request.token_in.symbol,
        request.token_out.symbol,
        request.chain.name()
    );
    if theme.color {
        out.push_str(&header.bold().to_string());
    } else {
        out.push_str(&header);
    }

    let sep: String = "-".repeat(90);
    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    for result in results {
        let name = result.name;
        match &result.quote {
            Ok(q) => {
                let amount_str = format!(
                    "{} {}",
                    format_amount(q.amount_out, request.token_out.decimals, 4),
                    request.token_out.symbol
                );
                let path_str = match &q.route {
                    Some(hops) if !hops.is_empty() => hops.join(" → "),
                    _ => "—".to_string(),
                };

                let row = format!(" {:<12}  {:>22}  {}\n", name, amount_str, path_str);
                if theme.color {
                    // Highlight rows with a real multi-hop path (≥3 venues)
                    // in cyan to make the interesting rows jump out.
                    let hop_count = q.route.as_ref().map(|r| r.len()).unwrap_or(0);
                    if hop_count >= 3 {
                        out.push_str(&row.cyan().to_string());
                    } else {
                        out.push_str(&row);
                    }
                } else {
                    out.push_str(&row);
                }
            }
            Err(_) => {
                let row = format!(
                    " {:<12}  {:>22}  {}\n",
                    name,
                    theme.em_dash(),
                    theme.em_dash()
                );
                if theme.color {
                    out.push_str(&row.dimmed().to_string());
                } else {
                    out.push_str(&row);
                }
            }
        }
    }

    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    let footer = format!(
        " Fetched in {:.1}s · path column shows underlying venues the backend routed through\n \
         on-chain backends are a single venue so they don't surface a multi-hop path\n",
        elapsed.as_secs_f64()
    );
    if theme.color {
        out.push_str(&footer.dimmed().to_string());
    } else {
        out.push_str(&footer);
    }

    out
}
