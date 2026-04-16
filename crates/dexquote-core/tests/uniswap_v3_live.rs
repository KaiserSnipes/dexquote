//! Live UniswapV3 integration test. Requires `DEXQUOTE_TEST_RPC` to be set
//! to an Arbitrum RPC URL. Gated behind `#[ignore]`; run with
//! `DEXQUOTE_TEST_RPC=https://arb1.arbitrum.io/rpc cargo test -- --ignored`.

use alloy::providers::{Provider, ProviderBuilder};
use dexquote_core::token::parse_amount;
use dexquote_core::{
    Chain, DexBackend, GasPricer, OnChainContext, QuoteRequest, Token, UniswapV3Backend,
};

#[tokio::test]
#[ignore]
async fn uniswap_v3_quotes_weth_to_usdc_on_arbitrum() {
    let Ok(rpc_url) = std::env::var("DEXQUOTE_TEST_RPC") else {
        eprintln!("skipping: DEXQUOTE_TEST_RPC not set");
        return;
    };

    let chain = Chain::Arbitrum;
    let sell = Token::resolve_static("WETH", chain)
        .expect("valid input")
        .expect("WETH registry hit");
    let buy = Token::resolve_static("USDC", chain)
        .expect("valid input")
        .expect("USDC registry hit");
    let amount_in = parse_amount("1.0", sell.decimals).expect("parse 1.0");

    let req = QuoteRequest {
        chain,
        token_in: sell,
        token_out: buy,
        amount_in,
        block_id: None,
    };

    let provider = ProviderBuilder::new()
        .connect(&rpc_url)
        .await
        .expect("connect")
        .erased();
    let gas_pricer = GasPricer::new(chain, Some(provider.clone()));
    let backend = UniswapV3Backend::new(OnChainContext {
        provider,
        gas_pricer,
    });
    let quote = backend.quote(&req).await.expect("UniswapV3 quote succeeds");
    assert!(!quote.amount_out.is_zero(), "non-zero output");
}
