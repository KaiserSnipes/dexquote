//! Benchmark mode — runs a hardcoded canonical pair set across every
//! backend on every supported chain and aggregates per-backend stats.
//!
//! The output is the writeup ammunition for the v0.7 blog post: every
//! claim about which backend wins what kind of pair, where DODO PMM
//! beats aggregators, where Curve has dead pools, etc., is grounded in
//! the numbers this command produces.
//!
//! Pair set is hardcoded (`const BENCHMARK_PAIRS`) so results are
//! reproducible. ~30 pairs across all 3 chains, picked to exercise:
//! - Canonical WETH/USDC + native↔bridged stablecoin pairs
//! - LST pools (wstETH, cbETH, rETH)
//! - BTC variants (WBTC, cbBTC, tBTC)
//! - Chain-native tokens (ARB, AERO, GMX, DEGEN, BRETT)
//! - Memecoins (SHIB, PEPE) for thin-liquidity stress
//!
//! No persistence to history.jsonl — benchmark mode is one-shot. v0.8+
//! can layer on `dexquote benchmark history` if useful.

use crate::config::Config;
use crate::error::{CliError, CliResult};
use crate::render::benchmark::{render_benchmark, render_benchmark_json};
use crate::theme::{ColorMode, Theme};
use alloy::primitives::U256;
use dexquote_core::token::parse_amount;
use dexquote_core::{quote_all, BackendResult, Chain, QuoteRequest, Quote, Token};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Hardcoded benchmark pair set: `(chain, sell_symbol, buy_symbol, amount_str)`.
/// Amounts are in human units (parsed via `parse_amount` against the
/// sell token's decimals).
pub(crate) const BENCHMARK_PAIRS: &[(Chain, &str, &str, &str)] = &[
    // Ethereum mainnet — 12 pairs
    (Chain::Ethereum, "WETH", "USDC", "1"),
    (Chain::Ethereum, "USDC", "USDT", "1000"),
    (Chain::Ethereum, "DAI", "USDC", "1000"),
    (Chain::Ethereum, "WETH", "wstETH", "1"),
    (Chain::Ethereum, "WBTC", "WETH", "0.1"),
    (Chain::Ethereum, "WETH", "UNI", "1"),
    (Chain::Ethereum, "WETH", "LINK", "1"),
    (Chain::Ethereum, "WETH", "AAVE", "1"),
    (Chain::Ethereum, "USDC", "DAI", "1000"),
    (Chain::Ethereum, "USDT", "DAI", "1000"),
    (Chain::Ethereum, "WETH", "PEPE", "1"),
    (Chain::Ethereum, "SHIB", "WETH", "1000000"),
    // Arbitrum — 8 pairs
    (Chain::Arbitrum, "WETH", "USDC", "1"),
    (Chain::Arbitrum, "WETH", "USDC.e", "1"),
    (Chain::Arbitrum, "USDC", "USDT", "1000"),
    (Chain::Arbitrum, "ARB", "WETH", "100"),
    (Chain::Arbitrum, "WETH", "wstETH", "1"),
    (Chain::Arbitrum, "WBTC", "WETH", "0.1"),
    (Chain::Arbitrum, "GMX", "WETH", "1"),
    (Chain::Arbitrum, "MAGIC", "WETH", "100"),
    // Base — 9 pairs
    (Chain::Base, "WETH", "USDC", "1"),
    (Chain::Base, "USDC", "USDbC", "1000"),
    (Chain::Base, "WETH", "cbETH", "1"),
    (Chain::Base, "WETH", "wstETH", "1"),
    (Chain::Base, "WETH", "AERO", "1"),
    (Chain::Base, "WETH", "DEGEN", "1"),
    (Chain::Base, "WETH", "BRETT", "1"),
    (Chain::Base, "WETH", "TOSHI", "1"),
    (Chain::Base, "WETH", "cbBTC", "1"),
];

/// Per-backend aggregated stats across the entire benchmark sweep.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BackendStat {
    pub name: String,
    /// Number of pairs where this backend was the highest-amount-out
    /// winner (excluding thin_liq and dead_pool quotes).
    pub wins: u32,
    /// Successful quote responses across all pairs.
    pub successes: u32,
    /// Total attempts (including failures and `NoRoute`).
    pub attempts: u32,
    /// `successes / attempts * 100`.
    pub success_rate: f64,
    /// Median latency across successful quotes only.
    pub median_latency_ms: u128,
    /// Average percent deviation from per-pair median, across successful
    /// quotes. Negative means consistently below median, positive means
    /// consistently above.
    pub avg_spread_pct: f64,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct BenchmarkStats {
    pub backends: Vec<BackendStat>,
    pub total_pairs: usize,
    pub total_elapsed_ms: u128,
}

pub async fn run(
    config: &Config,
    chain_filter: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let chain_filter = chain_filter
        .map(Chain::parse)
        .transpose()
        .map_err(CliError::from)?;

    let timeout = Duration::from_millis(config.defaults.timeout_ms);
    let backend_names = config.backends.enabled.clone();
    let selection = crate::parse_backend_names(&backend_names)?;

    let mut all_results: Vec<Vec<BackendResult>> = Vec::new();

    if !json {
        eprintln!();
        eprintln!(" Running benchmark across {} pairs...", BENCHMARK_PAIRS.len());
        eprintln!();
    }

    let total_start = Instant::now();
    for chain in Chain::ALL.iter().copied() {
        if let Some(filter) = chain_filter {
            if chain != filter {
                continue;
            }
        }

        // Build the backend list once per chain. We use the chain's
        // default public RPC; users wanting their own RPCs can set the
        // global config first.
        let rpc_url = chain.default_public_rpc().to_string();
        let built = match crate::build_backends(
            &selection,
            chain,
            Some(&rpc_url),
            timeout,
        )
        .await
        {
            Ok(b) => b,
            Err(e) => {
                if !json {
                    eprintln!("  ! {}: skipped — {}", chain.name(), e.message);
                }
                continue;
            }
        };
        let backends = built.backends;
        if backends.is_empty() {
            continue;
        }

        if !json {
            eprintln!("  {} ({} backends):", chain.name(), backends.len());
        }

        for (pair_chain, sell_sym, buy_sym, amount_str) in BENCHMARK_PAIRS {
            if *pair_chain != chain {
                continue;
            }

            let sell = match Token::resolve_static(sell_sym, chain).ok().flatten() {
                Some(t) => t,
                None => {
                    if !json {
                        eprintln!("    skip {} {} → {}: not in registry", amount_str, sell_sym, buy_sym);
                    }
                    continue;
                }
            };
            let buy = match Token::resolve_static(buy_sym, chain).ok().flatten() {
                Some(t) => t,
                None => {
                    if !json {
                        eprintln!("    skip {} {} → {}: not in registry", amount_str, sell_sym, buy_sym);
                    }
                    continue;
                }
            };
            let amount_in = match parse_amount(amount_str, sell.decimals) {
                Ok(a) => a,
                Err(_) => continue,
            };

            let request = QuoteRequest {
                chain,
                token_in: sell,
                token_out: buy,
                amount_in,
                block_id: None,
            };

            if !json {
                eprint!("    {} {} → {} ... ", amount_str, sell_sym, buy_sym);
            }
            let pair_start = Instant::now();
            let results = quote_all(&backends, &request, timeout).await;
            let pair_elapsed = pair_start.elapsed().as_millis();
            if !json {
                let n_ok = results.iter().filter(|r| r.quote.is_ok()).count();
                eprintln!("{}/{} ok ({}ms)", n_ok, results.len(), pair_elapsed);
            }
            all_results.push(results);
        }
    }

    let total_elapsed = total_start.elapsed();
    let stats = aggregate(&all_results, total_elapsed);

    if json {
        println!("{}", render_benchmark_json(&stats));
    } else {
        let theme = Theme::resolve(ColorMode::Auto);
        println!("{}", render_benchmark(&stats, theme));
    }

    Ok(())
}

/// Walk every per-pair result set, compute per-pair winners (excluding
/// thin_liq + dead_pool quotes via the existing render-layer helpers),
/// and accumulate per-backend totals.
pub(crate) fn aggregate(results: &[Vec<BackendResult>], total_elapsed: Duration) -> BenchmarkStats {
    struct Acc {
        wins: u32,
        successes: u32,
        attempts: u32,
        latencies: Vec<u128>,
        spreads: Vec<f64>,
    }

    let mut accs: HashMap<&'static str, Acc> = HashMap::new();

    for pair_results in results {
        let successes: Vec<&Quote> = pair_results
            .iter()
            .filter_map(|r| r.quote.as_ref().ok())
            .collect();

        let median = crate::render::table::median_amount(&successes);

        // Winner = highest amount_out among non-thin-liq, non-dead-pool.
        let winner: Option<&'static str> = successes
            .iter()
            .filter(|q| !crate::render::table::is_thin_liquidity(q.amount_out, median))
            .filter(|q| !crate::render::table::is_dead_pool(q.amount_out, median))
            .max_by_key(|q| q.amount_out)
            .map(|q| q.backend);

        for r in pair_results {
            let acc = accs.entry(r.name).or_insert(Acc {
                wins: 0,
                successes: 0,
                attempts: 0,
                latencies: Vec::new(),
                spreads: Vec::new(),
            });
            acc.attempts += 1;

            if let Ok(q) = &r.quote {
                acc.successes += 1;
                acc.latencies.push(q.latency_ms);

                if Some(r.name) == winner {
                    acc.wins += 1;
                }

                if let Some(med) = median {
                    if !med.is_zero() {
                        let amt_f = u256_to_f64(q.amount_out);
                        let med_f = u256_to_f64(med);
                        if med_f > 0.0 {
                            let spread = (amt_f - med_f) / med_f * 100.0;
                            acc.spreads.push(spread);
                        }
                    }
                }
            }
        }
    }

    let mut backend_stats: Vec<BackendStat> = accs
        .into_iter()
        .map(|(name, acc)| {
            let success_rate = if acc.attempts > 0 {
                (acc.successes as f64 / acc.attempts as f64) * 100.0
            } else {
                0.0
            };
            let mut sorted_lat = acc.latencies.clone();
            sorted_lat.sort();
            let median_latency = if sorted_lat.is_empty() {
                0
            } else {
                sorted_lat[sorted_lat.len() / 2]
            };
            let avg_spread = if acc.spreads.is_empty() {
                0.0
            } else {
                acc.spreads.iter().sum::<f64>() / acc.spreads.len() as f64
            };
            BackendStat {
                name: name.to_string(),
                wins: acc.wins,
                successes: acc.successes,
                attempts: acc.attempts,
                success_rate,
                median_latency_ms: median_latency,
                avg_spread_pct: avg_spread,
            }
        })
        .collect();

    backend_stats.sort_by(|a, b| b.wins.cmp(&a.wins).then(b.successes.cmp(&a.successes)));

    BenchmarkStats {
        backends: backend_stats,
        total_pairs: results.len(),
        total_elapsed_ms: total_elapsed.as_millis(),
    }
}

fn u256_to_f64(value: U256) -> f64 {
    value.to_string().parse().unwrap_or(0.0)
}
