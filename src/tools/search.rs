use super::AppContext;
use anyhow::Result;

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
    let query_str = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let limit = (args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize).min(100);
    let min_confidence = args
        .get("min_confidence")
        .and_then(|v| v.as_f64())
        .map(|c| c as f32);

    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Indexer not initialized"))?;
    let query_vector = {
        let mut idx = indexer.lock().await;
        let embeddings = idx
            .embed(&[query_str.to_string()])
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        embeddings.into_iter().next().unwrap_or_default()
    };

    let results = store
        .search(query_vector, query_str.to_string(), limit, min_confidence)
        .await?;

    Ok(super::response::json_text_response(&serde_json::to_string(
        &results,
    )?))
}
