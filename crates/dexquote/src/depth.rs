//! Depth mode — quotes the same pair at multiple notionals and renders
//! the price-impact curve. Helps users see how much of the input amount
//! a venue can actually fill before slippage eats the trade.
//!
//! Notionals are scaled relative to the user's input: `0.1×, 1×, 10×,
//! 100×, 1000×`. Each level runs a fresh `quote_all` round and captures
//! the best (non-thin, non-dead) `amount_out`. Price impact is computed
//! relative to the smallest level (the closest thing to a "spot" rate).

use crate::config::Config;
use crate::error::{CliError, CliResult};
use crate::render::depth::{render_depth, DepthLevel, DepthReport};
use crate::theme::{ColorMode, Theme};
use alloy::primitives::U256;
use dexquote_core::token::parse_amount;
use dexquote_core::{quote_all, Chain, QuoteRequest, Token};
use std::time::Duration;

/// Notional multipliers applied to the user's input amount. The first
/// entry (`0.1`) acts as the baseline — every other level's price impact
/// is computed relative to its effective rate.
pub(crate) const NOTIONALS: [f64; 5] = [0.1, 1.0, 10.0, 100.0, 1000.0];

pub async fn run(
    config: &Config,
    sell_input: &str,
    buy_input: &str,
    amount_input: &str,
    chain_override: Option<&str>,
) -> CliResult<()> {
    let chain_str = chain_override
        .map(|s| s.to_string())
        .unwrap_or_else(|| config.defaults.chain.clone());
    let chain = Chain::parse(&chain_str).map_err(CliError::from)?;

    let timeout = Duration::from_millis(config.defaults.timeout_ms);
    let backend_names = config.backends.enabled.clone();
    let selection = crate::parse_backend_names(&backend_names)?;

    // Resolve tokens against the static registry. Depth mode uses the
    // same chain-default RPC pattern as benchmark mode — users wanting
    // their own RPC should pass --rpc or set the config first.
    let rpc_url = if !config.defaults.rpc.is_empty() {
        Some(config.defaults.rpc.clone())
    } else {
        Some(chain.default_public_rpc().to_string())
    };

    let sell = Token::resolve(sell_input, chain, rpc_url.as_deref())
        .await
        .map_err(CliError::from)?;
    let buy = Token::resolve(buy_input, chain, rpc_url.as_deref())
        .await
        .map_err(CliError::from)?;

    let base_amount = parse_amount(amount_input, sell.decimals).map_err(CliError::from)?;

    let built = crate::build_backends(
        &selection,
        chain,
        rpc_url.as_deref(),
        timeout,
    )
    .await?;
    let backends = built.backends;
    if backends.is_empty() {
        return Err(CliError::setup(
            "no backends available for depth sweep".to_string(),
            "check `dexquote config show` and ensure on-chain backends \
             have an RPC configured"
                .to_string(),
        ));
    }

    let theme = Theme::resolve(ColorMode::Auto);

    eprintln!();
    eprintln!(
        " Depth sweep: {} {} → {} on {} ({} backends)",
        amount_input,
        sell.symbol,
        buy.symbol,
        chain.name(),
        backends.len()
    );
    eprintln!();

    let mut levels: Vec<DepthLevel> = Vec::new();

    for &mult in NOTIONALS.iter() {
        let scaled_amount = scale_amount(base_amount, mult);
        let request = QuoteRequest {
            chain,
            token_in: sell.clone(),
            token_out: buy.clone(),
            amount_in: scaled_amount,
            block_id: None,
        };

        let label = format!("{}× ({} {})", mult, format_mult(mult, amount_input), sell.symbol);
        eprint!("    {} ... ", label);

        let results = quote_all(&backends, &request, timeout).await;

        // Reuse the existing thin/dead filters so depth mode picks the
        // same "healthy best" the main quote command would.
        let successes: Vec<&dexquote_core::Quote> = results
            .iter()
            .filter_map(|r| r.quote.as_ref().ok())
            .collect();
        let median = crate::render::table::median_amount(&successes);

        let best = successes
            .iter()
            .filter(|q| !crate::render::table::is_thin_liquidity(q.amount_out, median))
            .filter(|q| !crate::render::table::is_dead_pool(q.amount_out, median))
            .max_by_key(|q| q.amount_out);

        match best {
            Some(q) => {
                eprintln!("{} {} via {}", format_amount_human(q.amount_out, buy.decimals), buy.symbol, q.backend);
                levels.push(DepthLevel {
                    multiplier: mult,
                    amount_in: scaled_amount,
                    amount_out: Some(q.amount_out),
                    best_venue: Some(q.backend.to_string()),
                });
            }
            None => {
                eprintln!("no route");
                levels.push(DepthLevel {
                    multiplier: mult,
                    amount_in: scaled_amount,
                    amount_out: None,
                    best_venue: None,
                });
            }
        }
    }

    let report = DepthReport {
        chain,
        sell: sell.clone(),
        buy: buy.clone(),
        base_amount_human: amount_input.to_string(),
        levels,
    };

    println!("{}", render_depth(&report, theme));

    Ok(())
}

pub(crate) fn scale_amount(base: U256, mult: f64) -> U256 {
    if mult <= 0.0 || !mult.is_finite() {
        return U256::ZERO;
    }
    // Scale via fixed-point: multiply base by (mult * 1e9) then divide
    // by 1e9. Keeps 9 digits of precision for the multiplier — plenty
    // for fractional notionals like 0.1.
    let scale = 1_000_000_000u64;
    let scaled_mult = (mult * scale as f64) as u128;
    let factor = U256::from(scaled_mult);
    let divisor = U256::from(scale);
    base.saturating_mul(factor) / divisor
}

fn format_amount_human(amount: U256, decimals: u8) -> String {
    dexquote_core::token::format_amount(amount, decimals, 4)
}

fn format_mult(mult: f64, base_str: &str) -> String {
    let base: f64 = base_str.parse().unwrap_or(1.0);
    let scaled = base * mult;
    if scaled >= 1000.0 {
        format!("{:.0}", scaled)
    } else if scaled >= 1.0 {
        format!("{:.2}", scaled)
    } else {
        format!("{:.4}", scaled)
    }
}
