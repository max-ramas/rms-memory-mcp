use super::AppContext;
use anyhow::Result;
use rms_memory_index::file_history::{
    FileHistoryRecord, catch_up_file_history, reindex_file_history,
};

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
    let code_path = ctx
        .code_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("code_path not initialized for file history"))?;

    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("query");
    let explicit_project = args.get("project").and_then(|v| v.as_str());

    match action {
        "reindex" => {
            // Destructive wipe — refuse sticky-bind so agents cannot wipe the
            // wrong project's cache when `project` is omitted.
            if explicit_project.is_none() {
                return Err(anyhow::anyhow!(
                    "action=reindex requires an explicit `project` key (refuses sticky-bind wipe)"
                ));
            }
            let report = reindex_file_history(store, code_path).await?;
            Ok(super::response::json_text_response(&serde_json::to_string(
                &serde_json::json!({
                    "action": "reindex",
                    "project": explicit_project,
                    "from_sha": report.from_sha,
                    "to_sha": report.to_sha,
                    "commits_scanned": report.commits_scanned,
                    "rows_upserted": report.rows_upserted,
                }),
            )?))
        }
        "catch_up" => {
            let report = catch_up_file_history(store, code_path).await?;
            Ok(super::response::json_text_response(&serde_json::to_string(
                &serde_json::json!({
                    "action": "catch_up",
                    "noop": report.noop,
                    "from_sha": report.from_sha,
                    "to_sha": report.to_sha,
                    "commits_scanned": report.commits_scanned,
                    "rows_upserted": report.rows_upserted,
                }),
            )?))
        }
        "query" => {
            let path = args
                .get("path")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow::anyhow!("path is required for rms_file_history query"))?;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(20)
                .clamp(1, 200) as usize;
            let include_message = args
                .get("include_message")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            // Lazy catch-up before query (event-driven; no interval poll).
            let catch_up = catch_up_file_history(store, code_path).await?;
            let rows = store.query_file_history(path, limit).await?;
            Ok(super::response::json_text_response(&serde_json::to_string(
                &serde_json::json!({
                    "path": path,
                    "catch_up": {
                        "noop": catch_up.noop,
                        "from_sha": catch_up.from_sha,
                        "to_sha": catch_up.to_sha,
                        "commits_scanned": catch_up.commits_scanned,
                        "rows_upserted": catch_up.rows_upserted,
                    },
                    "commits": rows_to_json(&rows, include_message),
                }),
            )?))
        }
        other => Err(anyhow::anyhow!(
            "Unknown rms_file_history action '{other}'. Valid: query, catch_up, reindex."
        )),
    }
}

pub fn rows_to_json(rows: &[FileHistoryRecord], include_message: bool) -> Vec<serde_json::Value> {
    rows.iter()
        .map(|row| {
            let mut map = serde_json::Map::new();
            map.insert("sha".into(), serde_json::json!(row.commit_sha));
            map.insert("author".into(), serde_json::json!(row.author));
            map.insert("committed_at".into(), serde_json::json!(row.committed_at));
            if include_message {
                map.insert("message".into(), serde_json::json!(row.message));
            }
            serde_json::Value::Object(map)
        })
        .collect()
}
