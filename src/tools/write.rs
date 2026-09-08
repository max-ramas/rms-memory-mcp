use super::AppContext;
use crate::audit::{content_fingerprint, inject_audit_metadata};
use anyhow::Result;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteAction {
    Create,
    Update,
    Noop,
}

impl WriteAction {
    fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Noop => "noop",
        }
    }
}

struct WritePlan {
    path_str: String,
    file_path: PathBuf,
    mode: String,
    /// Full document text that would be committed for create/replace,
    /// or the injected chunk for append.
    planned_payload: String,
    /// Full on-disk+planned document used for fingerprint / diff.after.
    after_document: String,
    before_document: Option<String>,
    action: WriteAction,
    predecessor: Option<(PathBuf, String)>,
    doc_id: Option<String>,
}

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let dry_run = args
        .get("dry_run")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let plan = plan_write(ctx, args)?;

    if dry_run {
        return Ok(super::response::json_text_response(&serde_json::to_string(
            &dry_run_payload(&plan),
        )?));
    }

    // Real writes always commit (including stamp-only refreshes) for backward
    // compatibility — noop classification is meaningful only for dry_run.
    commit_write(ctx, &plan)?;
    Ok(super::response::json_text_response(&format!(
        "Successfully wrote to {}",
        plan.path_str
    )))
}

fn plan_write(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<WritePlan> {
    let workspace_root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    super::validation::reject_wiki_write(path_str)?;
    let initial_file_path = super::validation::resolve_vault_path(workspace_root, path_str)?;
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("replace")
        .to_string();

    let file_path = if initial_file_path.exists() {
        let resolved = crate::link::resolve_link_in_vault(
            &initial_file_path,
            workspace_root,
            ctx.code_path.as_deref(),
        )?;
        if crate::path_policy::is_vault_wiki_path(workspace_root, &resolved) {
            return Err(anyhow::anyhow!(
                "Resolved link target '{}' is inside the generated Wiki namespace and cannot be written through the canonical memory tools.",
                resolved.display()
            ));
        }
        resolved
    } else {
        initial_file_path.clone()
    };

    if mode == "create" && file_path.exists() {
        return Err(anyhow::anyhow!(
            "File already exists: {}. Use mode='replace' or 'append' to modify existing files.",
            path_str
        ));
    }

    match mode.as_str() {
        "append" | "create" | "replace" => {}
        m => {
            return Err(anyhow::anyhow!(
                "Unknown write mode '{}'. Valid modes: create, append, replace",
                m
            ));
        }
    }

    let supersedes_path = args.get("supersedes").and_then(|v| v.as_str());
    let mut write_args = args.clone();
    write_args.remove("dry_run");
    // Stable item_key across dry_run → real create for the same path when the
    // caller omits `id` (UUID v5 of project+path). Agents may still pass an
    // explicit id; dry_run returns item_key for transparency.
    if mode == "create"
        && write_args
            .get("id")
            .and_then(|v| v.as_str())
            .is_none_or(|s| s.is_empty())
    {
        let seed = format!("{}:{}", ctx.project_key.as_deref().unwrap_or(""), path_str);
        write_args.insert(
            "id".into(),
            serde_json::json!(
                uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, seed.as_bytes()).to_string()
            ),
        );
    }
    let predecessor = if let Some(old_rel) = supersedes_path {
        let old_path = super::validation::resolve_vault_path(workspace_root, old_rel)?;
        if !old_path.exists() {
            return Err(anyhow::anyhow!("supersedes path does not exist: {old_rel}"));
        }
        let old_doc = crate::document::Document::parse(&old_path)?;
        let old_rel_path = old_path.strip_prefix(workspace_root).unwrap_or(&old_path);
        let old_id = old_doc.index_id(old_rel_path);
        write_args.insert("supersedes_id".into(), serde_json::json!(old_id));
        Some((old_path, old_id))
    } else {
        None
    };

    let planned_payload = inject_audit_metadata(
        content,
        &ctx.caller_id,
        ctx.project_key.as_deref(),
        &write_args,
    )?;

    let before_document = if file_path.exists() {
        Some(std::fs::read_to_string(&file_path)?)
    } else {
        None
    };

    let after_document = match mode.as_str() {
        "append" => match &before_document {
            Some(before) => format!("{before}{planned_payload}"),
            None => planned_payload.clone(),
        },
        _ => planned_payload.clone(),
    };

    let action = match &before_document {
        None => WriteAction::Create,
        Some(before) => {
            if content_fingerprint(before) == content_fingerprint(&after_document) {
                WriteAction::Noop
            } else {
                WriteAction::Update
            }
        }
    };

    let doc_id = extract_frontmatter_id(&after_document);

    Ok(WritePlan {
        path_str: path_str.to_string(),
        file_path,
        mode,
        planned_payload,
        after_document,
        before_document,
        action,
        predecessor,
        doc_id,
    })
}

fn dry_run_payload(plan: &WritePlan) -> serde_json::Value {
    let mutates = matches!(plan.action, WriteAction::Create | WriteAction::Update);
    let mut diff = serde_json::Map::new();
    if let Some(before) = &plan.before_document {
        diff.insert("before".into(), serde_json::json!(before));
    } else {
        diff.insert("before".into(), serde_json::Value::Null);
    }
    diff.insert("after".into(), serde_json::json!(plan.after_document));

    serde_json::json!({
        "dry_run": true,
        "action": plan.action.as_str(),
        "file_path": plan.path_str,
        "item_key": plan.doc_id,
        "diff": diff,
        "would_affect": {
            "index_delete": mutates,
            "index_reinsert": mutates,
            "watcher_events_suppressed": true
        }
    })
}

fn commit_write(ctx: &AppContext, plan: &WritePlan) -> Result<()> {
    if plan.file_path.exists() && ctx.max_backups > 0 {
        write_guard_backup(&plan.file_path, ctx.max_backups)?;
    }

    if let Some(parent) = plan.file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match plan.mode.as_str() {
        "append" => {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&plan.file_path)?;
            f.write_all(plan.planned_payload.as_bytes())?;
        }
        "create" | "replace" => {
            std::fs::write(&plan.file_path, &plan.planned_payload)?;
        }
        _ => unreachable!("mode validated in plan_write"),
    }

    if let Some((old_path, _old_id)) = &plan.predecessor {
        let workspace_root = ctx
            .workspace_root
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
        let new_doc = crate::document::Document::parse(&plan.file_path)?;
        let new_rel = plan
            .file_path
            .strip_prefix(workspace_root)
            .unwrap_or(&plan.file_path);
        let new_id = new_doc.index_id(new_rel);
        mark_document_superseded(old_path, &new_id)?;
    }

    Ok(())
}

fn write_guard_backup(file_path: &Path, max_backups: usize) -> Result<()> {
    let mut backups = Vec::new();
    let parent = file_path.parent().unwrap_or(Path::new(""));
    let base_name = file_path.file_name().unwrap_or_default().to_string_lossy();

    if let Ok(entries) = std::fs::read_dir(parent) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with(&format!("{}.bak.", base_name)) {
                backups.push(entry.path());
            }
        }
    }

    backups.sort_by_key(|a| {
        std::fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
    });

    while backups.len() >= max_backups {
        if let Some(oldest) = backups.first() {
            let _ = std::fs::remove_file(oldest);
        }
        backups.remove(0);
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
    let bak_path = parent.join(format!("{}.bak.{}", base_name, timestamp));

    if let Err(e) = std::fs::copy(file_path, &bak_path) {
        tracing::error!(
            "Write-Guard: Failed to create snapshot for {:?}: {}",
            file_path,
            e
        );
    } else {
        tracing::info!("Write-Guard: Created snapshot at {:?}", bak_path);
    }
    Ok(())
}

fn mark_document_superseded(path: &Path, superseded_by: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    if !(text.starts_with("---\n") || text.starts_with("---\r\n")) {
        return Err(anyhow::anyhow!(
            "Cannot mark superseded: {} has no frontmatter",
            path.display()
        ));
    }
    let end_idx = text
        .find("\n---\n")
        .or_else(|| text.find("\r\n---\r\n"))
        .ok_or_else(|| anyhow::anyhow!("Unclosed frontmatter in {}", path.display()))?;
    let fm_text = &text[4..end_idx];
    let mut mapping: serde_yaml::Mapping = serde_yaml::from_str(fm_text)?;
    mapping.insert(
        serde_yaml::Value::String("status".into()),
        serde_yaml::Value::String("superseded".into()),
    );
    mapping.insert(
        serde_yaml::Value::String("superseded_by".into()),
        serde_yaml::Value::String(superseded_by.to_string()),
    );
    let updated_fm = serde_yaml::to_string(&mapping)
        .unwrap_or_default()
        .trim_end()
        .to_string();
    let remainder = &text[end_idx..];
    std::fs::write(path, format!("---\n{}\n{}", updated_fm, remainder))?;
    Ok(())
}

fn extract_frontmatter_id(markdown: &str) -> Option<String> {
    if !(markdown.starts_with("---\n") || markdown.starts_with("---\r\n")) {
        return None;
    }
    let end_idx = markdown
        .find("\n---\n")
        .or_else(|| markdown.find("\r\n---\r\n"))?;
    let fm_text = &markdown[4..end_idx];
    let mapping: serde_yaml::Mapping = serde_yaml::from_str(fm_text).ok()?;
    mapping
        .get(serde_yaml::Value::String("id".into()))
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::AppContext;
    use tempfile::tempdir;

    #[test]
    fn repeated_metadata_injection_keeps_one_id_and_the_complete_body() {
        let args = serde_json::Map::new();
        let first = crate::audit::inject_audit_metadata(
            "# Complete body\n\nDo not truncate.",
            "writer-a",
            Some("p"),
            &args,
        )
        .expect("first");
        let second = crate::audit::inject_audit_metadata(&first, "writer-b", Some("p"), &args)
            .expect("second");
        assert_eq!(second.matches("\nid:").count(), 1);
        assert!(second.ends_with("# Complete body\n\nDo not truncate."));
    }

    fn make_ctx(root: PathBuf) -> AppContext {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        AppContext {
            store: None,
            indexer: None,
            workspace_root: Some(root),
            code_path: None,
            max_backups: 0,
            scope: None,
            caller_id: "test".to_string(),
            project_key: None,
        }
    }

    fn parse_dry_run(value: &serde_json::Value) -> serde_json::Value {
        let text = value["content"][0]["text"].as_str().expect("text");
        serde_json::from_str(text).expect("dry_run json")
    }

    #[tokio::test]
    async fn rejects_wiki_write_path() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("wiki/index.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("hi"));
        let error = execute(&ctx, &args).await.unwrap_err().to_string();
        assert!(
            error.contains("Wiki") || error.contains("wiki"),
            "got: {error}"
        );
    }

    #[tokio::test]
    async fn rejects_non_markdown_write_path() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("notes/api.txt"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("hi"));
        let error = execute(&ctx, &args).await.unwrap_err().to_string();
        assert!(error.contains("Markdown"), "got: {error}");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn rejects_write_through_link_that_escapes_vault() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_target = outside.path().join("target.md");
        std::fs::write(&outside_target, "external content").unwrap();

        let escape_link = vault.path().join("escape.md");
        std::os::unix::fs::symlink(&outside_target, &escape_link).unwrap();
        let doc = vault.path().join("doc.md");
        std::fs::write(&doc, "---\nlink: escape.md\n---\n").unwrap();

        let ctx = make_ctx(vault.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("doc.md"));
        args.insert("mode".into(), serde_json::json!("replace"));
        args.insert("content".into(), serde_json::json!("clobber"));
        let error = execute(&ctx, &args).await.unwrap_err().to_string();
        assert!(
            error.contains("escapes vault")
                || error.contains("escapes allowed")
                || error.contains("escapes project")
                || error.contains("Failed to canonicalize"),
            "got: {error}"
        );

        let disk = std::fs::read_to_string(&outside_target).unwrap();
        assert_eq!(disk, "external content");
    }

    #[tokio::test]
    async fn soft_supersede_marks_predecessor_and_links_ids() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
        let old_path = dir.path().join("decisions/old.md");
        std::fs::write(
            &old_path,
            "---\nid: old-note-1\nstatus: active\n---\n\nOld fact.\n",
        )
        .unwrap();

        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("decisions/new.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("New fact."));
        args.insert("supersedes".into(), serde_json::json!("decisions/old.md"));
        execute(&ctx, &args).await.expect("write");

        let new_doc = crate::document::Document::parse(&dir.path().join("decisions/new.md"))
            .expect("parse new");
        let new_fm = new_doc.frontmatter.expect("fm");
        assert_eq!(new_fm.supersedes.as_deref(), Some("old-note-1"));
        assert_eq!(new_fm.status.as_deref(), Some("active"));
        let new_id = new_fm.id.clone().expect("new id");

        let old_doc = crate::document::Document::parse(&old_path).expect("parse old");
        let old_fm = old_doc.frontmatter.expect("old fm");
        assert_eq!(old_fm.status.as_deref(), Some("superseded"));
        assert_eq!(old_fm.superseded_by.as_deref(), Some(new_id.as_str()));
    }

    #[tokio::test]
    async fn dry_run_create_does_not_touch_disk() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("decisions/new.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("Hello."));
        args.insert("dry_run".into(), serde_json::json!(true));
        let response = execute(&ctx, &args).await.expect("dry_run");
        let payload = parse_dry_run(&response);
        assert_eq!(payload["action"], "create");
        assert_eq!(payload["dry_run"], true);
        assert_eq!(payload["would_affect"]["index_delete"], true);
        assert_eq!(payload["would_affect"]["watcher_events_suppressed"], true);
        assert!(!dir.path().join("decisions/new.md").exists());
        assert!(payload["item_key"].as_str().is_some_and(|s| !s.is_empty()));
    }

    #[tokio::test]
    async fn dry_run_create_item_key_stable_across_plans() {
        let dir = tempdir().unwrap();
        let mut ctx = make_ctx(dir.path().to_path_buf());
        ctx.project_key = Some("demo".into());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("decisions/new.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("Hello."));
        args.insert("dry_run".into(), serde_json::json!(true));
        let a = parse_dry_run(&execute(&ctx, &args).await.expect("dry_run a"));
        let b = parse_dry_run(&execute(&ctx, &args).await.expect("dry_run b"));
        assert_eq!(a["item_key"], b["item_key"]);
        args.insert("dry_run".into(), serde_json::json!(false));
        execute(&ctx, &args).await.expect("write");
        let written = std::fs::read_to_string(dir.path().join("decisions/new.md")).unwrap();
        assert!(written.contains(a["item_key"].as_str().unwrap()));
    }

    #[tokio::test]
    async fn dry_run_stamp_only_refresh_is_noop() {
        // ADR: volatile timestamp/last_modified_by must not force update.
        let dir = tempdir().unwrap();
        let path = dir.path().join("rules/api.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let on_disk = "---\nid: note-1\ncreated_at: 2026-01-01T00:00:00Z\nproject: p\ntimestamp: 2026-01-01T00:00:00Z\nlast_modified_by: older\n---\n\nStable body.\n";
        std::fs::write(&path, on_disk).unwrap();

        let mut ctx = make_ctx(dir.path().to_path_buf());
        ctx.caller_id = "newer-caller".into();
        ctx.project_key = Some("p".into());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("rules/api.md"));
        args.insert("mode".into(), serde_json::json!("replace"));
        // Same semantic body; inject will refresh stamps.
        args.insert(
            "content".into(),
            serde_json::json!(
                "---\nid: note-1\ncreated_at: 2026-01-01T00:00:00Z\nproject: p\n---\n\nStable body.\n"
            ),
        );
        args.insert("dry_run".into(), serde_json::json!(true));
        let response = execute(&ctx, &args).await.expect("dry_run");
        let payload = parse_dry_run(&response);
        assert_eq!(payload["action"], "noop", "payload={payload}");
        assert_eq!(payload["would_affect"]["index_delete"], false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk);
    }

    #[tokio::test]
    async fn dry_run_body_change_is_update() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("rules/api.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let on_disk = "---\nid: note-1\ncreated_at: 2026-01-01T00:00:00Z\n---\n\nOld body.\n";
        std::fs::write(&path, on_disk).unwrap();

        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("rules/api.md"));
        args.insert("mode".into(), serde_json::json!("replace"));
        args.insert(
            "content".into(),
            serde_json::json!(
                "---\nid: note-1\ncreated_at: 2026-01-01T00:00:00Z\n---\n\nNew body.\n"
            ),
        );
        args.insert("dry_run".into(), serde_json::json!(true));
        let response = execute(&ctx, &args).await.expect("dry_run");
        let payload = parse_dry_run(&response);
        assert_eq!(payload["action"], "update");
        assert_eq!(payload["would_affect"]["index_reinsert"], true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), on_disk);
    }

    #[tokio::test]
    async fn dry_run_false_still_writes() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("decisions/x.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert("content".into(), serde_json::json!("Real write."));
        args.insert("dry_run".into(), serde_json::json!(false));
        execute(&ctx, &args).await.expect("write");
        assert!(dir.path().join("decisions/x.md").exists());
    }
}
