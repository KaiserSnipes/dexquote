//! Live ODOS integration test. Gated behind `#[ignore]` so CI without
//! network access doesn't break; run with `cargo test -- --ignored`.

use dexquote_core::token::parse_amount;
use dexquote_core::{Chain, DexBackend, OdosBackend, QuoteRequest, Token};

#[tokio::test]
#[ignore]
async fn odos_quotes_weth_to_usdc_on_arbitrum() {
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

    let backend = OdosBackend::new();
    let quote = backend.quote(&req).await.expect("ODOS quote succeeds");
    assert!(!quote.amount_out.is_zero(), "non-zero output");
    assert!(quote.latency_ms > 0, "latency captured");
}
