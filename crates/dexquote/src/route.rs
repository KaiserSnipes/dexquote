//! Route mode — runs a standard quote and then displays the multi-hop
//! path each backend used to produce its answer. Aggregators return
//! routing data in their response payloads (ODOS `pathViz`, Paraswap
//! `bestRoute`, KyberSwap `routeSummary.route`, OpenOcean `path.routes`,
//! LiFi `includedSteps`, CoWSwap solver label). This subcommand surfaces
//! all of them side by side.
//!
//! The insight: running `dexquote WETH USDC 1` tells you what each
//! backend quotes. Running `dexquote route WETH USDC 1` tells you
//! *where each backend actually routes*. Aggregators that look identical
//! in amount_out often take completely different paths — which is gold
//! for understanding where DEX liquidity actually lives.

use crate::config::Config;
use crate::error::{CliError, CliResult};
use crate::render::route::render_route;
use crate::theme::{ColorMode, Theme};
use dexquote_core::token::parse_amount;
use dexquote_core::{quote_all, Chain, QuoteRequest, Token};
use std::time::{Duration, Instant};

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

    let amount_in = parse_amount(amount_input, sell.decimals).map_err(CliError::from)?;

    let built = crate::build_backends(&selection, chain, rpc_url.as_deref(), timeout).await?;
    let backends = built.backends;
    if backends.is_empty() {
        return Err(CliError::setup(
            "no backends available for route query".to_string(),
            "check `dexquote config show` and ensure the selected chain has enabled backends"
                .to_string(),
        ));
    }

    let request = QuoteRequest {
        chain,
        token_in: sell.clone(),
        token_out: buy.clone(),
        amount_in,
        block_id: None,
    };

    let start = Instant::now();
    let results = quote_all(&backends, &request, timeout).await;
    let elapsed = start.elapsed();

    let theme = Theme::resolve(ColorMode::Auto);
    println!("{}", render_route(&request, &results, elapsed, theme));

    Ok(())
}
