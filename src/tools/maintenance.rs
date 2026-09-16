//! MCP maintenance tools: doctor, reindex, sync.

use super::AppContext;
use anyhow::{Result, anyhow};

pub async fn execute_doctor(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let repair_frontmatter = args
        .get("repair_frontmatter")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if repair_frontmatter && args.get("project").and_then(|v| v.as_str()).is_none() {
        return Err(anyhow!(
            "repair_frontmatter requires an explicit `project` key (refuses sticky-bind repair)"
        ));
    }
    let workspace = crate::tools::graph::workspace_from_ctx(ctx)?;
    let report = crate::doctor::diagnose(
        &workspace,
        crate::doctor::DiagnoseOptions { repair_frontmatter },
    )
    .await?;
    let companion = crate::companion_status::detect();
    let mut payload = serde_json::to_value(&report)?;
    if let Some(obj) = payload.as_object_mut() {
        obj.insert(
            "companion".into(),
            serde_json::json!({
                "gui_installed": companion.gui_installed,
                "ai_configured": companion.ai_configured,
            }),
        );
    }
    super::response::json_structured_response(&payload)
}

pub async fn execute_reindex(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    if args.get("project").and_then(|v| v.as_str()).is_none() {
        return Err(anyhow!(
            "rms_reindex requires an explicit `project` key (refuses sticky-bind rebuild)"
        ));
    }
    let workspace = crate::tools::graph::workspace_from_ctx(ctx)?;
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow!("Store not initialized"))?;
    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| anyhow!("Indexer not initialized"))?;
    let corpus = args
        .get("corpus")
        .and_then(|v| v.as_str())
        .unwrap_or("vault");

    let mut indexer = indexer.lock().await;
    let mut vault_done = false;
    let mut code_stats = None;
    match corpus {
        "vault" => {
            crate::indexer::index_vault_full(&workspace, store, &mut indexer).await?;
            vault_done = true;
        }
        "code" => {
            code_stats =
                Some(crate::code_indexer::index_code_full(&workspace, store, &mut indexer).await?);
        }
        "all" => {
            crate::indexer::index_vault_full(&workspace, store, &mut indexer).await?;
            vault_done = true;
            code_stats =
                Some(crate::code_indexer::index_code_full(&workspace, store, &mut indexer).await?);
        }
        other => {
            return Err(anyhow!(
                "Unknown corpus '{other}'. Valid: vault, code, all."
            ));
        }
    }

    let payload = serde_json::json!({
        "action": "reindex",
        "project": args.get("project").and_then(|v| v.as_str()),
        "corpus": corpus,
        "vault_reindexed": vault_done,
        "code": code_stats.as_ref().map(|s| serde_json::json!({
            "files_indexed": s.files_indexed,
            "items_indexed": s.items_indexed,
            "segments_indexed": s.segments_indexed,
            "segments_embedded": s.segments_embedded,
            "segments_reused": s.segments_reused,
            "segments_deleted": s.segments_deleted,
            "files_skipped": s.files_skipped,
        })),
    });
    super::response::json_structured_response(&payload)
}

pub async fn execute_sync(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let workspace = crate::tools::graph::workspace_from_ctx(ctx)?;
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow!("Store not initialized"))?;
    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| anyhow!("Indexer not initialized"))?;
    let mut indexer = indexer.lock().await;
    crate::indexer::sync_vault(&workspace, store, &mut indexer).await?;
    let payload = serde_json::json!({
        "action": "sync",
        "project": args.get("project").and_then(|v| v.as_str()),
        "vault_root": workspace.root.display().to_string(),
        "ok": true,
    });
    super::response::json_structured_response(&payload)
}
