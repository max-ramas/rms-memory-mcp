use super::AppContext;
use anyhow::Result;

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let workspace_root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let file_path = super::validation::resolve_vault_path(workspace_root, path_str)?;

    // If this document is a link, fetch the linked content — but only when the
    // resolved target stays inside the vault. Escapes fall through to reading
    // the link file itself instead of leaking data from outside the vault.
    if let Some(linked_content) =
        crate::link::get_linked_content_in_vault(&file_path, workspace_root)
    {
        Ok(super::response::json_text_response(&linked_content))
    } else {
        let store = ctx
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
        let full_text = store.read_document(path_str).await?;
        Ok(super::response::json_text_response(&full_text))
    }
}
