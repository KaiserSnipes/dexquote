//! Benchmark leaderboard renderer. Two outputs: a hand-rolled human-
//! readable table (default) and machine-readable JSON for the writeup.

use crate::benchmark::BenchmarkStats;
use crate::theme::Theme;
use colored::Colorize;

pub fn render_benchmark(stats: &BenchmarkStats, theme: Theme) -> String {
    let mut out = String::new();

    let header = format!(
        "\n  Benchmark across {} pairs ({:.1}s total)\n",
        stats.total_pairs,
        stats.total_elapsed_ms as f64 / 1000.0
    );
    if theme.color {
        out.push_str(&header.bold().to_string());
    } else {
        out.push_str(&header);
    }

    // Column widths fixed for a 17-row leaderboard.
    let bw = 14;
    let wins_w = 6;
    let succ_w = 12;
    let lat_w = 10;
    let spread_w = 12;
    let total_w = bw + 2 + wins_w + 2 + succ_w + 2 + lat_w + 2 + spread_w + 2;
    let sep: String = "-".repeat(total_w);

    out.push(' ');
    out.push_str(&sep);
    out.push('\n');
    out.push_str(&format!(
        " {:<bw$}  {:>wins_w$}  {:>succ_w$}  {:>lat_w$}  {:>spread_w$}\n",
        "backend", "wins", "ok rate", "p50 lat", "avg spread",
        bw = bw,
        wins_w = wins_w,
        succ_w = succ_w,
        lat_w = lat_w,
        spread_w = spread_w,
    ));
    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    for (i, b) in stats.backends.iter().enumerate() {
        let win_str = format!("{}", b.wins);
        let ok_str = format!("{}/{} ({:.0}%)", b.successes, b.attempts, b.success_rate);
        let lat_str = format!("{}ms", b.median_latency_ms);
        let spread_str = if b.avg_spread_pct == 0.0 && b.successes == 0 {
            "—".to_string()
        } else if b.avg_spread_pct >= 0.0 {
            format!("+{:.3}%", b.avg_spread_pct)
        } else {
            format!("{:.3}%", b.avg_spread_pct)
        };

        let row = format!(
            " {:<bw$}  {:>wins_w$}  {:>succ_w$}  {:>lat_w$}  {:>spread_w$}",
            b.name,
            win_str,
            ok_str,
            lat_str,
            spread_str,
            bw = bw,
            wins_w = wins_w,
            succ_w = succ_w,
            lat_w = lat_w,
            spread_w = spread_w,
        );

        if theme.color {
            // Top 3 highlighted, low success rate dimmed.
            if i == 0 {
                out.push_str(&row.green().bold().to_string());
            } else if i < 3 {
                out.push_str(&row.green().to_string());
            } else if b.success_rate < 50.0 {
                out.push_str(&row.dimmed().to_string());
            } else {
                out.push_str(&row);
            }
        } else {
            out.push_str(&row);
        }
        out.push('\n');
    }

    out.push(' ');
    out.push_str(&sep);
    out.push('\n');

    let footer = " wins = pairs where this backend produced the highest amount_out\n \
         spread = average % deviation from the per-pair median (negative = consistently below)\n \
         healthy quotes only — thin_liq + dead_pool excluded from win/spread calculation\n"
        .to_string();
    if theme.color {
        out.push_str(&footer.dimmed().to_string());
    } else {
        out.push_str(&footer);
    }
    out
}

pub fn render_benchmark_json(stats: &BenchmarkStats) -> String {
    serde_json::to_string_pretty(stats).unwrap_or_else(|_| "{}".to_string())
}
