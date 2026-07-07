use super::AppContext;
use crate::store::VectorStore;
use anyhow::Result;
use serde_json::json;

pub async fn execute(
    ctx: &mut AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
    let query_str = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
    let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

    let query_vector = {
        if ctx.indexer.is_none() {
            let idx = tokio::task::spawn_blocking(crate::indexer::Indexer::new)
                .await
                .unwrap_or_else(|_| Err(anyhow::anyhow!("Indexer spawn blocked")))?;
            ctx.indexer = Some(std::sync::Arc::new(tokio::sync::Mutex::new(idx)));
        }
        let mut indexer = ctx.indexer.as_ref().unwrap().lock().await;
        let embeddings = indexer
            .embed(&[query_str.to_string()])
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        embeddings.into_iter().next().unwrap_or_default()
    };

    let results = store
        .search(query_vector, query_str.to_string(), limit)
        .await?;

    Ok(json!({
        "content": [{"type": "text", "text": serde_json::to_string(&results)? }]
    }))
}
