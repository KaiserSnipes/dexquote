//! Streaming quote output for TTY mode.
//!
//! Shows one `indicatif::ProgressBar` per backend up-front, each spinning
//! with "fetching…". As each backend's future resolves, its bar is replaced
//! in place with the finished row. When all backends are done, the bars are
//! finalized and a footer is printed underneath.
//!
//! Non-TTY paths skip this module entirely and go straight to the batch
//! `render::render_human` renderer.

use crate::theme::Theme;
use colored::Colorize;
use dexquote_core::token::format_amount;
use dexquote_core::{BackendResult, DexBackend, DexQuoteError, Quote, QuoteRequest};
use futures::future::join_all;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct StreamConfig<'a> {
    pub request: &'a QuoteRequest,
    pub backends: &'a [Arc<dyn DexBackend>],
    pub per_backend_timeout: Duration,
    pub theme: Theme,
}

pub struct StreamOutcome {
    pub results: Vec<BackendResult>,
    pub total_elapsed_ms: u128,
}

/// Drive the quote through `indicatif` live output. The function blocks
/// until every backend has either returned or timed out, then returns the
/// collected results alongside the total wall-clock elapsed.
pub async fn run(config: StreamConfig<'_>) -> StreamOutcome {
    // Header line above the bars — printed once, statically.
    let header = format_header(config.request, config.theme);
    let mp = MultiProgress::new();
    mp.println(header).ok();

    let bar_style = ProgressStyle::with_template("{spinner:.cyan} {msg}")
        .unwrap_or_else(|_| ProgressStyle::default_spinner())
        .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏", "✓"]);

    let bars: Vec<ProgressBar> = config
        .backends
        .iter()
        .map(|backend| {
            let pb = mp.add(ProgressBar::new_spinner());
            pb.set_style(bar_style.clone());
            pb.set_message(format_pending(backend.name(), config.theme));
            pb.enable_steady_tick(Duration::from_millis(80));
            pb
        })
        .collect();

    let start = Instant::now();

    let futures = config.backends.iter().zip(bars.iter()).map(|(backend, pb)| {
        let backend = backend.clone();
        let request = config.request.clone();
        let timeout = config.per_backend_timeout;
        let theme = config.theme;
        let pb = pb.clone();
        async move {
            let name = backend.name();
            let local_start = Instant::now();
            let result = match tokio::time::timeout(timeout, backend.quote(&request)).await {
                Ok(Ok(mut q)) => {
                    if q.latency_ms == 0 {
                        q.latency_ms = local_start.elapsed().as_millis();
                    }
                    Ok(q)
                }
                Ok(Err(e)) => Err(e),
                Err(_) => Err(DexQuoteError::Timeout {
                    backend: name,
                    ms: timeout.as_millis() as u64,
                }),
            };

            // Render the finished row into the progress bar's message, then
            // finish_with_message so the spinner halts but the line stays.
            let finished = format_finished(&request, name, &result, theme);
            pb.set_style(
                ProgressStyle::with_template("  {msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            pb.finish_with_message(finished);

            BackendResult {
                name,
                quote: result,
            }
        }
    });

    let results = join_all(futures).await;
    let total_elapsed_ms = start.elapsed().as_millis();

    StreamOutcome {
        results,
        total_elapsed_ms,
    }
}

fn format_header(request: &QuoteRequest, theme: Theme) -> String {
    let amount = format_amount(request.amount_in, request.token_in.decimals, 6);
    let line = format!(
        "\n {} {} {} {} on {}",
        amount,
        request.token_in.symbol,
        theme.arrow(),
        request.token_out.symbol,
        request.chain.name(),
    );
    if theme.color {
        line.bold().to_string()
    } else {
        line
    }
}

fn format_pending(name: &str, theme: Theme) -> String {
    let line = format!("{:<10}  fetching…", name);
    if theme.color {
        line.dimmed().to_string()
    } else {
        line
    }
}

fn format_finished(
    request: &QuoteRequest,
    name: &str,
    result: &Result<Quote, DexQuoteError>,
    theme: Theme,
) -> String {
    match result {
        Ok(q) => {
            let amount = format_amount(q.amount_out, request.token_out.decimals, 4);
            let gas = match q.gas_usd {
                Some(usd) if usd >= 0.01 => format!("gas ${:.2}", usd),
                Some(_) => "gas <$0.01".into(),
                None => "gas —".into(),
            };
            format!(
                "{:<10}  {:>16} {}   {}",
                name, amount, request.token_out.symbol, gas
            )
        }
        Err(e) => {
            let kind = match e {
                DexQuoteError::NoRoute { .. } => "no route",
                DexQuoteError::Timeout { .. } => "timeout",
                DexQuoteError::Http { .. } => "http error",
                DexQuoteError::Rpc { .. } => "rpc error",
                DexQuoteError::Decode { .. } => "decode error",
                _ => "error",
            };
            let line = format!("{:<10}  {:>16}   {}", name, theme.em_dash(), kind);
            if theme.color {
                line.bright_black().to_string()
            } else {
                line
            }
        }
    }
}
