use super::AppContext;
use crate::store::VectorStore;
use anyhow::Result;
use serde_json::json;

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let workspace_root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let path = std::path::Path::new(path_str);
    if path.is_absolute() {
        return Err(anyhow::anyhow!("Path must be relative to the vault (e.g. 'architecture/file.md'), but received absolute path: {}", path_str));
    }
    let file_path = workspace_root.join(path_str);

    if let Some(linked_content) = crate::link::get_linked_content(&file_path) {
        Ok(json!({
            "content": [{"type": "text", "text": linked_content}]
        }))
    } else {
        let store = ctx
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
        let full_text = store.read_document(path_str).await?;
        Ok(json!({
            "content": [{"type": "text", "text": full_text}]
        }))
    }
}
