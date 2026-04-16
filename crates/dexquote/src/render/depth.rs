//! Depth-mode renderer: a small table showing per-notional effective
//! rate and price impact relative to the smallest notional level.

use crate::theme::Theme;
use alloy::primitives::U256;
use colored::Colorize;
use dexquote_core::token::format_amount;
use dexquote_core::{Chain, Token};

#[derive(Debug, Clone)]
pub struct DepthLevel {
    pub multiplier: f64,
    pub amount_in: U256,
    pub amount_out: Option<U256>,
    pub best_venue: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DepthReport {
    pub chain: Chain,
    pub sell: Token,
    pub buy: Token,
    pub base_amount_human: String,
    pub levels: Vec<DepthLevel>,
}

pub fn render_depth(report: &DepthReport, theme: Theme) -> String {
    let mut out = String::new();

    let header = format!(
        "\n {} {} → {} depth on {}\n",
        report.base_amount_human,
        report.sell.symbol,
        report.buy.symbol,
        report.chain.name()
    );
    if theme.color {
        out.push_str(&header.bold().to_string());
    } else {
        out.push_str(&header);
    }

    let total_w = 72;
    let sep: String = "-".repeat(total_w);
    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    // Compute the baseline effective rate from the smallest level (0.1×)
    // for relative price-impact calculation. The baseline is the one with
    // the smallest non-zero amount_in that produced a non-zero amount_out.
    let baseline_rate = report
        .levels
        .iter()
        .find_map(|l| match l.amount_out {
            Some(out) if !out.is_zero() && !l.amount_in.is_zero() => {
                Some(effective_rate(l.amount_in, out, report.sell.decimals, report.buy.decimals))
            }
            _ => None,
        })
        .unwrap_or(0.0);

    for level in &report.levels {
        let in_str = format!(
            "{}× ({} {})",
            format_multiplier(level.multiplier),
            format_amount(level.amount_in, report.sell.decimals, 4),
            report.sell.symbol,
        );

        let (out_str, impact_str, venue_str) = match level.amount_out {
            Some(amount) if !amount.is_zero() => {
                let out_human = format!(
                    "{} {}",
                    format_amount(amount, report.buy.decimals, 4),
                    report.buy.symbol
                );
                let impact = if baseline_rate > 0.0 {
                    let level_rate = effective_rate(
                        level.amount_in,
                        amount,
                        report.sell.decimals,
                        report.buy.decimals,
                    );
                    let pct = (level_rate - baseline_rate) / baseline_rate * 100.0;
                    if level.multiplier == 0.1 {
                        "(baseline)".to_string()
                    } else if pct.abs() < 0.001 {
                        "0.000%".to_string()
                    } else {
                        format!("{:+.3}%", pct)
                    }
                } else {
                    "—".to_string()
                };
                let venue = level
                    .best_venue
                    .clone()
                    .unwrap_or_else(|| "—".to_string());
                (out_human, impact, venue)
            }
            _ => ("—".to_string(), "no route".to_string(), "—".to_string()),
        };

        let row = format!(
            "  {:<24}  {:>22}  {:>10}  {:<10}\n",
            in_str, out_str, impact_str, venue_str
        );
        if theme.color {
            // Highlight the level where impact crosses 1% as the
            // "watch out" boundary for traders.
            let parsed_impact: f64 = impact_str
                .trim_end_matches('%')
                .trim_start_matches('+')
                .parse()
                .unwrap_or(0.0);
            if parsed_impact <= -1.0 {
                out.push_str(&row.yellow().to_string());
            } else if parsed_impact <= -5.0 {
                out.push_str(&row.red().to_string());
            } else {
                out.push_str(&row);
            }
        } else {
            out.push_str(&row);
        }
    }

    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    let footer =
        " price impact = (level_rate - baseline_rate) / baseline_rate × 100\n \
         baseline = smallest notional level (0.1×) which approximates the spot rate\n \
         healthy quotes only — thin_liq + dead_pool excluded\n";
    if theme.color {
        out.push_str(&footer.dimmed().to_string());
    } else {
        out.push_str(footer);
    }

    out
}

fn effective_rate(
    amount_in: U256,
    amount_out: U256,
    in_dec: u8,
    out_dec: u8,
) -> f64 {
    let in_f = u256_to_f64_scaled(amount_in, in_dec);
    let out_f = u256_to_f64_scaled(amount_out, out_dec);
    if in_f == 0.0 {
        0.0
    } else {
        out_f / in_f
    }
}

fn u256_to_f64_scaled(value: U256, decimals: u8) -> f64 {
    let s = value.to_string();
    let d = decimals as usize;
    if s.len() <= d {
        let padded = format!("{:0>width$}", s, width = d);
        format!("0.{padded}").parse().unwrap_or(0.0)
    } else {
        let split = s.len() - d;
        let int = &s[..split];
        let frac = &s[split..];
        format!("{int}.{frac}").parse().unwrap_or(0.0)
    }
}

fn format_multiplier(m: f64) -> String {
    if m >= 1.0 {
        format!("{:.0}", m)
    } else {
        format!("{:.1}", m)
    }
}
