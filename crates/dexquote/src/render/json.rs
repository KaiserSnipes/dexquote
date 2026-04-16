//! Machine-readable JSON output for scripting.

use dexquote_core::BackendResult;

pub fn render_json(results: &[BackendResult]) -> String {
    let best = results
        .iter()
        .filter_map(|r| r.quote.as_ref().ok().map(|q| q.amount_out))
        .max();

    let arr: Vec<serde_json::Value> = results
        .iter()
        .map(|r| match &r.quote {
            Ok(q) => serde_json::json!({
                "backend": q.backend,
                "amount_out": q.amount_out.to_string(),
                "gas_estimate": q.gas_estimate,
                "gas_usd": q.gas_usd,
                "latency_ms": q.latency_ms,
                "best": best.map(|b| b == q.amount_out).unwrap_or(false),
                "error": serde_json::Value::Null,
            }),
            Err(e) => serde_json::json!({
                "backend": r.name,
                "amount_out": serde_json::Value::Null,
                "gas_estimate": serde_json::Value::Null,
                "gas_usd": serde_json::Value::Null,
                "latency_ms": serde_json::Value::Null,
                "best": false,
                "error": e.to_string(),
            }),
        })
        .collect();
    serde_json::to_string_pretty(&arr).unwrap_or_else(|_| "[]".into())
}
