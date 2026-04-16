//! Self-test subcommand. Reports the health of every layer the tool
//! depends on so a user debugging "why is it slow" gets an answer in
//! one command instead of a round-trip on Discord.
//!
//! Checks performed:
//!   1. Config file exists and parses
//!   2. RPC endpoint reachable + `eth_blockNumber` latency
//!   3. Chainlink ETH/USD feed returns a positive price
//!   4. Each enabled backend returns a quote for a canonical test pair
//!      (0.1 WETH → USDC on the active chain)
//!
//! v1.2 Phase 6a: this module was previously a pile of `println!`s.
//! It now returns a `Vec<DoctorSection>` that the CLI `run` function
//! formats via `render_doctor_report` and the TUI `doctor_stream`
//! module walks as a progress feed. The string output of the CLI
//! path must stay byte-identical — the refactor is verified with
//! `dexquote doctor` before/after diffs.

use crate::config::Config;
use crate::error::CliResult;
use crate::theme::{ColorMode, Theme};
use alloy::network::Ethereum;
use alloy::primitives::{address, Address};
use alloy::providers::{DynProvider, Provider, ProviderBuilder};
use alloy::sol;
use colored::Colorize;
use dexquote_core::token::parse_amount;
use dexquote_core::{
    AerodromeBackend, BalancerV2Backend, CamelotV3Backend, Chain, CowSwapBackend, CurveBackend,
    DexBackend, GasPricer, KyberSwapBackend, LiFiBackend, MaverickV2Backend, OdosBackend,
    OnChainContext, OpenOceanBackend, PancakeV3Backend, ParaswapBackend, QuoteRequest,
    DodoV2Backend, FraxSwapBackend, SlipstreamBackend, SushiV2Backend, Token, TraderJoeBackend,
    UniswapV2Backend, UniswapV3Backend, UniswapV4Backend,
};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Status marker for a single check. Mapped to ✓/!/✗ icons at
/// render time by `icon_for`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorStatus {
    Ok,
    Warn,
    Fail,
}

/// One row in a doctor report. The `body` field is the exact
/// pre-formatted content that the CLI renderer would have
/// `println!`'d under the icon — no trailing newline, no leading
/// icon. `note` is an optional indented sub-line (used for the
/// "latency above 1500ms" hint).
#[derive(Debug, Clone)]
pub struct DoctorItem {
    pub status: DoctorStatus,
    pub body: String,
    pub note: Option<(DoctorStatus, String)>,
}

/// A named grouping of `DoctorItem`s. Rendered with a `── name ──`
/// header and a blank line separator.
#[derive(Debug, Clone)]
pub struct DoctorSection {
    pub name: String,
    pub items: Vec<DoctorItem>,
}

// Chainlink ETH/USD feeds per chain. Mirrors the table in gas.rs — if you
// add a chain, update both.
const CHAINLINK_ETH_USD_ARBITRUM: Address =
    address!("639Fe6ab55C921f74e7fac1ee960C0B6293ba612");
const CHAINLINK_ETH_USD_BASE: Address =
    address!("71041dddad3595F9CEd3DcCFBe3D1F4b0a16Bb70");
const CHAINLINK_ETH_USD_ETHEREUM: Address =
    address!("5f4eC3Df9cbd43714FE2740f5E3616155c5b8419");

fn chainlink_feed_for(chain: Chain) -> Option<Address> {
    match chain {
        Chain::Arbitrum => Some(CHAINLINK_ETH_USD_ARBITRUM),
        Chain::Base => Some(CHAINLINK_ETH_USD_BASE),
        Chain::Ethereum => Some(CHAINLINK_ETH_USD_ETHEREUM),
        Chain::Solana => None,
    }
}

sol! {
    #[sol(rpc)]
    interface IChainlinkAggregator {
        function latestAnswer() external view returns (int256);
    }
}

/// CLI entry point. Builds the `Vec<DoctorSection>` by running each
/// check in sequence, then renders the string output. Byte-identical
/// to pre-refactor output.
pub async fn run(config: &Config, config_path: &Path) -> CliResult<()> {
    let theme = Theme::resolve(ColorMode::Auto);
    let sections = run_data(config, config_path).await;
    print!("{}", render_doctor_report(&sections, theme.color));
    Ok(())
}

/// Pure data path: run every check, collect results into structured
/// sections, return. The CLI `run` function wraps this with a
/// renderer; the TUI `doctor_stream` module uses a streaming
/// variant (per-check async events) instead. Both produce the same
/// `DoctorSection` shape.
pub async fn run_data(config: &Config, config_path: &Path) -> Vec<DoctorSection> {
    let mut sections = Vec::new();

    let mut env_items: Vec<DoctorItem> = Vec::new();
    env_items.extend(check_config(config_path));
    env_items.extend(check_defaults(config));
    sections.push(DoctorSection {
        name: "Environment".to_string(),
        items: env_items,
    });

    let rpc_url = if config.defaults.rpc.is_empty() {
        None
    } else {
        Some(config.defaults.rpc.as_str())
    };
    let (rpc_items, provider) = check_rpc(rpc_url).await;

    let chain = Chain::parse(&config.defaults.chain).unwrap_or(Chain::Arbitrum);

    let mut rpc_section_items = rpc_items;
    if let Some(provider) = &provider {
        rpc_section_items.extend(check_chainlink(provider, chain).await);
    }
    sections.push(DoctorSection {
        name: "RPC".to_string(),
        items: rpc_section_items,
    });

    sections.push(DoctorSection {
        name: "Backends (0.1 WETH → USDC probe)".to_string(),
        items: check_backends(chain, provider.as_ref(), config).await,
    });

    sections
}

pub fn render_doctor_report(sections: &[DoctorSection], colored: bool) -> String {
    let mut out = String::new();
    out.push('\n');
    for (idx, section) in sections.iter().enumerate() {
        if idx > 0 {
            out.push('\n');
        }
        let header = format!(" ── {} ──", section.name);
        if colored {
            out.push_str(&header.bold().to_string());
        } else {
            out.push_str(&header);
        }
        out.push('\n');
        for item in &section.items {
            out.push_str(&format!(
                "  {} {}\n",
                icon_for(item.status, colored),
                item.body
            ));
            if let Some((note_status, note)) = &item.note {
                out.push_str(&format!(
                    "    {} {}\n",
                    icon_for(*note_status, colored),
                    note
                ));
            }
        }
    }
    out.push('\n');
    out
}

fn icon_for(status: DoctorStatus, colored: bool) -> String {
    if colored {
        match status {
            DoctorStatus::Ok => "✓".green().bold().to_string(),
            DoctorStatus::Warn => "⚠".yellow().bold().to_string(),
            DoctorStatus::Fail => "✗".red().bold().to_string(),
        }
    } else {
        match status {
            DoctorStatus::Ok => "✓".to_string(),
            DoctorStatus::Warn => "!".to_string(),
            DoctorStatus::Fail => "x".to_string(),
        }
    }
}

pub fn check_config(path: &Path) -> Vec<DoctorItem> {
    let mut items = Vec::new();
    if path.exists() {
        items.push(DoctorItem {
            status: DoctorStatus::Ok,
            body: format!("config file found: {}", path.display()),
            note: None,
        });
    } else {
        items.push(DoctorItem {
            status: DoctorStatus::Fail,
            body: format!(
                "config file missing: {} (run dexquote to regenerate)",
                path.display()
            ),
            note: None,
        });
    }
    items
}

pub fn check_defaults(config: &Config) -> Vec<DoctorItem> {
    let mut items = Vec::new();
    items.push(DoctorItem {
        status: DoctorStatus::Ok,
        body: format!("chain:     {}", config.defaults.chain),
        note: None,
    });
    let rpc_label = if config.defaults.rpc.is_empty() {
        "<empty>".to_string()
    } else {
        config.defaults.rpc.clone()
    };
    items.push(DoctorItem {
        status: DoctorStatus::Ok,
        body: format!("rpc:       {}", rpc_label),
        note: None,
    });
    items.push(DoctorItem {
        status: DoctorStatus::Ok,
        body: format!("timeout:   {}ms", config.defaults.timeout_ms),
        note: None,
    });
    items.push(DoctorItem {
        status: DoctorStatus::Ok,
        body: format!("backends:  {} enabled", config.backends.enabled.len()),
        note: None,
    });
    items
}

/// Connect to the configured RPC and probe `eth_blockNumber`.
/// Returns the collected items plus (on success) the built provider
/// so downstream checks can reuse it for the Chainlink probe and
/// the on-chain backend probes.
pub async fn check_rpc(
    rpc_url: Option<&str>,
) -> (Vec<DoctorItem>, Option<DynProvider<Ethereum>>) {
    let mut items = Vec::new();
    let Some(url) = rpc_url else {
        items.push(DoctorItem {
            status: DoctorStatus::Warn,
            body: "no RPC configured — only ODOS / Paraswap / KyberSwap / OpenOcean / LiFi will work"
                .to_string(),
            note: None,
        });
        return (items, None);
    };

    let connect_start = Instant::now();
    let provider = match ProviderBuilder::new().connect(url).await {
        Ok(p) => p.erased(),
        Err(e) => {
            items.push(DoctorItem {
                status: DoctorStatus::Fail,
                body: format!("could not connect to {url}: {e}"),
                note: None,
            });
            return (items, None);
        }
    };
    let connect_ms = connect_start.elapsed().as_millis();
    items.push(DoctorItem {
        status: DoctorStatus::Ok,
        body: format!("connected to {url} ({connect_ms}ms)"),
        note: None,
    });

    let call_start = Instant::now();
    match provider.get_block_number().await {
        Ok(block) => {
            let call_ms = call_start.elapsed().as_millis();
            let status = if call_ms > 1500 {
                DoctorStatus::Warn
            } else {
                DoctorStatus::Ok
            };
            let note = if call_ms > 1500 {
                Some((
                    DoctorStatus::Warn,
                    "latency above 1500ms — consider using a dedicated RPC".to_string(),
                ))
            } else {
                None
            };
            items.push(DoctorItem {
                status,
                body: format!("eth_blockNumber → {block} ({call_ms}ms)"),
                note,
            });
        }
        Err(e) => {
            items.push(DoctorItem {
                status: DoctorStatus::Fail,
                body: format!("eth_blockNumber failed: {e}"),
                note: None,
            });
            return (items, None);
        }
    }

    (items, Some(provider))
}

pub async fn check_chainlink(
    provider: &DynProvider<Ethereum>,
    chain: Chain,
) -> Vec<DoctorItem> {
    let mut items = Vec::new();
    let Some(feed_address) = chainlink_feed_for(chain) else {
        items.push(DoctorItem {
            status: DoctorStatus::Ok,
            body: format!("Chainlink ETH/USD: not applicable on {}", chain.name()),
            note: None,
        });
        return items;
    };
    let feed = IChainlinkAggregator::new(feed_address, provider.clone());
    let start = Instant::now();
    let builder = feed.latestAnswer();
    match builder.call().await {
        Ok(answer) => {
            let ms = start.elapsed().as_millis();
            if answer.is_negative() {
                items.push(DoctorItem {
                    status: DoctorStatus::Fail,
                    body: "Chainlink ETH/USD returned negative answer".to_string(),
                    note: None,
                });
                return items;
            }
            let scaled = answer.into_raw();
            let whole = scaled / alloy::primitives::U256::from(10u128.pow(8));
            let whole_f: f64 = whole.to_string().parse().unwrap_or(0.0);
            items.push(DoctorItem {
                status: DoctorStatus::Ok,
                body: format!("Chainlink ETH/USD: ${:.2} ({}ms)", whole_f, ms),
                note: None,
            });
        }
        Err(e) => {
            items.push(DoctorItem {
                status: DoctorStatus::Fail,
                body: format!("Chainlink ETH/USD call failed: {e}"),
                note: None,
            });
        }
    }
    items
}

pub async fn check_backends(
    chain: Chain,
    provider: Option<&DynProvider<Ethereum>>,
    config: &Config,
) -> Vec<DoctorItem> {
    let mut items = Vec::new();
    let (Some(sell), Some(buy)) = (
        Token::resolve_static("WETH", chain).ok().flatten(),
        Token::resolve_static("USDC", chain).ok().flatten(),
    ) else {
        items.push(DoctorItem {
            status: DoctorStatus::Fail,
            body: "test pair missing from registry".to_string(),
            note: None,
        });
        return items;
    };
    let amount_in = parse_amount("0.1", sell.decimals)
        .unwrap_or(alloy::primitives::U256::from(100_000_000_000_000_000u128));
    let request = QuoteRequest {
        chain,
        token_in: sell,
        token_out: buy,
        amount_in,
        block_id: None,
    };

    let timeout = Duration::from_millis(config.defaults.timeout_ms);
    let gas_pricer = GasPricer::new(chain, provider.cloned());
    let ctx: Option<OnChainContext> = provider.map(|p| OnChainContext {
        provider: p.clone(),
        gas_pricer: gas_pricer.clone(),
    });

    let http = reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(concat!("dexquote/", env!("CARGO_PKG_VERSION")))
        .build()
        .unwrap_or_else(|_| reqwest::Client::new());

    let backends: Vec<(&'static str, Arc<dyn DexBackend>)> = build_probe_backends(chain, ctx, &http);

    let probes = backends.into_iter().enumerate().map(|(idx, (name, backend))| {
        let request = request.clone();
        async move {
            let start = Instant::now();
            let result = tokio::time::timeout(timeout, backend.quote(&request)).await;
            let ms = start.elapsed().as_millis();
            (idx, name, result, ms)
        }
    });

    let mut results: Vec<(usize, &'static str, _, u128)> =
        futures::future::join_all(probes).await;
    results.sort_by_key(|(idx, _, _, _)| *idx);

    for (_, name, result, ms) in results {
        items.push(backend_result_to_item(
            name,
            result,
            ms,
            request.token_out.decimals,
            &request.token_out.symbol,
        ));
    }

    items
}

fn build_probe_backends(
    chain: Chain,
    ctx: Option<OnChainContext>,
    http: &reqwest::Client,
) -> Vec<(&'static str, Arc<dyn DexBackend>)> {
    let mut v: Vec<(&'static str, Arc<dyn DexBackend>)> = Vec::new();
    if let Some(c) = ctx {
        if UniswapV2Backend::supports(chain) {
            v.push(("UniswapV2", Arc::new(UniswapV2Backend::new(c.clone()))));
        }
        if UniswapV3Backend::supports(chain) {
            v.push(("UniswapV3", Arc::new(UniswapV3Backend::new(c.clone()))));
        }
        if UniswapV4Backend::supports(chain) {
            v.push(("UniswapV4", Arc::new(UniswapV4Backend::new(c.clone()))));
        }
        if SushiV2Backend::supports(chain) {
            v.push(("SushiV2", Arc::new(SushiV2Backend::new(c.clone()))));
        }
        if FraxSwapBackend::supports(chain) {
            v.push(("FraxSwap", Arc::new(FraxSwapBackend::new(c.clone()))));
        }
        if TraderJoeBackend::supports(chain) {
            v.push(("TraderJoe", Arc::new(TraderJoeBackend::new(c.clone()))));
        }
        if PancakeV3Backend::supports(chain) {
            v.push(("PancakeV3", Arc::new(PancakeV3Backend::new(c.clone()))));
        }
        if CamelotV3Backend::supports(chain) {
            v.push(("CamelotV3", Arc::new(CamelotV3Backend::new(c.clone()))));
        }
        if CurveBackend::supports(chain) {
            v.push(("Curve", Arc::new(CurveBackend::new(c.clone()))));
        }
        if AerodromeBackend::supports(chain) {
            v.push(("Aerodrome", Arc::new(AerodromeBackend::new(c.clone()))));
        }
        if SlipstreamBackend::supports(chain) {
            v.push(("Slipstream", Arc::new(SlipstreamBackend::new(c.clone()))));
        }
        if BalancerV2Backend::supports(chain) {
            v.push(("BalancerV2", Arc::new(BalancerV2Backend::new(c.clone()))));
        }
        if MaverickV2Backend::supports(chain) {
            v.push(("Maverick", Arc::new(MaverickV2Backend::new(c.clone()))));
        }
        if DodoV2Backend::supports(chain) {
            v.push(("DODO", Arc::new(DodoV2Backend::new(c.clone()))));
        }
    }
    v.push(("ODOS", Arc::new(OdosBackend::with_client(http.clone()))));
    v.push(("Paraswap", Arc::new(ParaswapBackend::with_client(http.clone()))));
    v.push(("KyberSwap", Arc::new(KyberSwapBackend::with_client(http.clone()))));
    v.push(("OpenOcean", Arc::new(OpenOceanBackend::with_client(http.clone()))));
    v.push(("LiFi", Arc::new(LiFiBackend::with_client(http.clone()))));
    v.push(("CoWSwap", Arc::new(CowSwapBackend::with_client(http.clone()))));
    v
}

type ProbeResult = Result<
    Result<dexquote_core::Quote, dexquote_core::DexQuoteError>,
    tokio::time::error::Elapsed,
>;

fn backend_result_to_item(
    name: &'static str,
    result: ProbeResult,
    ms: u128,
    out_decimals: u8,
    out_symbol: &str,
) -> DoctorItem {
    match result {
        Ok(Ok(q)) => {
            let amount_h = dexquote_core::token::format_amount(q.amount_out, out_decimals, 4);
            DoctorItem {
                status: DoctorStatus::Ok,
                body: format!("{:<10} {:>12} {} ({}ms)", name, amount_h, out_symbol, ms),
                note: None,
            }
        }
        Ok(Err(e)) => DoctorItem {
            status: DoctorStatus::Warn,
            body: format!("{:<10} {} ({ms}ms)", name, short_error(&e)),
            note: None,
        },
        Err(_) => DoctorItem {
            status: DoctorStatus::Fail,
            body: format!("{:<10} timeout after {ms}ms", name),
            note: None,
        },
    }
}

fn short_error(e: &dexquote_core::DexQuoteError) -> String {
    use dexquote_core::DexQuoteError::*;
    match e {
        NoRoute { .. } => "no route".to_string(),
        Timeout { ms, .. } => format!("timeout ({ms}ms)"),
        Http { source, .. } => {
            let msg = source.to_string();
            if msg.contains("429") || msg.to_ascii_lowercase().contains("rate") {
                "rate limited (429)".to_string()
            } else {
                format!("http: {}", &msg[..msg.len().min(50)])
            }
        }
        Rpc { source, .. } => {
            let msg = source.to_string();
            if msg.to_ascii_lowercase().contains("rate") || msg.contains("429") {
                "rate limited".to_string()
            } else if msg.to_ascii_lowercase().contains("revert") {
                "reverted".to_string()
            } else {
                format!("rpc: {}", &msg[..msg.len().min(50)])
            }
        }
        Decode { message, .. } => format!("decode: {}", &message[..message.len().min(50)]),
        _ => e.to_string(),
    }
}

