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
    /// Full document text committed to disk; also the fingerprint / diff.after.
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
    // Watcher debounce can lag several seconds; refresh the durable vault graph
    // immediately so agents can traverse new Markdown links via `rms_graph`.
    let graph_refresh = super::graph::refresh_vault_graph_after_write(ctx).await;
    let message = match &graph_refresh {
        super::graph::GraphRefreshOutcome::Ok => {
            format!("Successfully wrote to {}", plan.path_str)
        }
        super::graph::GraphRefreshOutcome::Skipped { reason } => format!(
            "Successfully wrote to {}\n(warning: vault graph refresh skipped: {reason})",
            plan.path_str
        ),
        super::graph::GraphRefreshOutcome::Failed { error } => format!(
            "Successfully wrote to {}\n(warning: vault graph refresh failed: {error})",
            plan.path_str
        ),
    };
    super::response::json_structured_response(&serde_json::json!({
        "message": message,
        "file_path": plan.path_str,
        "action": plan.action.as_str(),
        "graph_refresh": graph_refresh,
    }))
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

    let before_document = if file_path.exists() {
        Some(std::fs::read_to_string(&file_path)?)
    } else {
        None
    };

    let source = match (mode.as_str(), &before_document) {
        ("append" | "replace", Some(before)) => {
            merge_into_existing(&mode, before, content, path_str)?
        }
        _ => content.to_string(),
    };
    let after_document = inject_audit_metadata(
        &source,
        &ctx.caller_id,
        ctx.project_key.as_deref(),
        &write_args,
    )?;

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

    std::fs::write(&plan.file_path, &plan.after_document)?;

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

/// Rebuilds the document so append/replace keep the existing frontmatter
/// (id, status, pinned, supersession links) instead of minting a new block.
fn merge_into_existing(mode: &str, before: &str, content: &str, path_str: &str) -> Result<String> {
    use crate::frontmatter;

    let (mut mapping, existing_body) = match frontmatter::split(before) {
        Some(split) => match frontmatter::parse_mapping(split.yaml) {
            Ok(mapping) => (mapping, split.body),
            Err(_) if mode == "replace" => (serde_yaml::Mapping::new(), ""),
            Err(error) => {
                return Err(error.context(format!(
                    "Cannot append to {path_str}: its frontmatter is invalid YAML; repair it first"
                )));
            }
        },
        None => (serde_yaml::Mapping::new(), before),
    };
    // inject_audit_metadata re-stamps the current workspace project.
    mapping.remove("project");

    let incoming_body = match frontmatter::split(content) {
        Some(split) => match frontmatter::parse_mapping(split.yaml) {
            Ok(incoming) => {
                mapping.extend(incoming);
                split.body
            }
            Err(_) => content,
        },
        None => content,
    };
    let body = if mode == "append" {
        append_body(existing_body, incoming_body)
    } else {
        incoming_body.to_string()
    };
    if mapping.is_empty() {
        return Ok(body);
    }
    frontmatter::compose(&mapping, &body)
}

fn append_body(existing: &str, addition: &str) -> String {
    let addition = addition.trim_start_matches(['\r', '\n']);
    if existing.trim().is_empty() {
        return format!("\n{addition}");
    }
    format!("{}\n\n{addition}", existing.trim_end_matches(['\r', '\n']))
}

fn mark_document_superseded(path: &Path, superseded_by: &str) -> Result<()> {
    let text = std::fs::read_to_string(path)?;
    let split = crate::frontmatter::split(&text).ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot mark superseded: {} has no closed frontmatter",
            path.display()
        )
    })?;
    let mut mapping = crate::frontmatter::parse_mapping(split.yaml)?;
    mapping.insert(
        serde_yaml::Value::String("status".into()),
        serde_yaml::Value::String("superseded".into()),
    );
    mapping.insert(
        serde_yaml::Value::String("superseded_by".into()),
        serde_yaml::Value::String(superseded_by.to_string()),
    );
    std::fs::write(path, crate::frontmatter::compose(&mapping, split.body)?)?;
    Ok(())
}

fn extract_frontmatter_id(markdown: &str) -> Option<String> {
    let split = crate::frontmatter::split(markdown)?;
    let mapping = crate::frontmatter::parse_mapping(split.yaml).ok()?;
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

    fn write_args(
        path: &str,
        mode: &str,
        content: &str,
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!(path));
        args.insert("mode".into(), serde_json::json!(mode));
        args.insert("content".into(), serde_json::json!(content));
        args
    }

    fn frontmatter_of(text: &str) -> serde_yaml::Mapping {
        let split = crate::frontmatter::split(text).expect("frontmatter");
        crate::frontmatter::parse_mapping(split.yaml).expect("yaml")
    }

    #[test]
    fn empty_frontmatter_parses_without_panicking() {
        let dir = tempdir().unwrap();
        for (label, text) in [
            ("lf", "---\n---\n# body\n"),
            ("crlf", "---\r\n---\r\n# body\r\n"),
        ] {
            let path = dir.path().join(format!("empty-{label}.md"));
            std::fs::write(&path, text).unwrap();
            let doc = crate::document::Document::parse(&path).expect("empty block is valid");
            assert!(doc.frontmatter.unwrap().id.is_none());
            assert!(doc.content.starts_with("# body"));
        }
    }

    #[tokio::test]
    async fn append_and_replace_keep_existing_frontmatter() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut create = write_args("notes/a.md", "create", "# Title\n\nfirst paragraph");
        create.insert("pinned".into(), serde_json::json!(true));
        create.insert("status".into(), serde_json::json!("superseded"));
        execute(&ctx, &create).await.unwrap();
        let file = ctx.workspace_root.as_ref().unwrap().join("notes/a.md");
        let original = frontmatter_of(&std::fs::read_to_string(&file).unwrap());

        execute(
            &ctx,
            &write_args("notes/a.md", "append", "second paragraph"),
        )
        .await
        .unwrap();
        let appended = std::fs::read_to_string(&file).unwrap();
        assert_eq!(appended.matches("\n---\n").count(), 1, "{appended}");
        assert_eq!(appended.lines().filter(|l| l.starts_with("id:")).count(), 1);
        assert!(
            appended.ends_with("first paragraph\n\nsecond paragraph"),
            "{appended}"
        );
        assert_eq!(frontmatter_of(&appended).get("id"), original.get("id"));

        execute(
            &ctx,
            &write_args("notes/a.md", "replace", "# Title\n\nrewritten body"),
        )
        .await
        .unwrap();
        let replaced = std::fs::read_to_string(&file).unwrap();
        let fm = frontmatter_of(&replaced);
        assert_eq!(fm.get("id"), original.get("id"));
        assert_eq!(fm.get("created_at"), original.get("created_at"));
        assert_eq!(fm.get("pinned"), Some(&serde_yaml::Value::Bool(true)));
        assert_eq!(
            fm.get("status").and_then(|v| v.as_str()),
            Some("superseded")
        );
        assert!(replaced.ends_with("# Title\n\nrewritten body"));
        assert!(!replaced.contains("first paragraph"));
    }

    #[tokio::test]
    async fn replace_frontmatter_overrides_existing_keys() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        let mut create = write_args("a.md", "create", "body");
        create.insert("pinned".into(), serde_json::json!(true));
        execute(&ctx, &create).await.unwrap();
        execute(
            &ctx,
            &write_args("a.md", "replace", "---\npinned: false\n---\n\nnew body"),
        )
        .await
        .unwrap();
        let text = std::fs::read_to_string(dir.path().join("a.md")).unwrap();
        assert_eq!(
            frontmatter_of(&text).get("pinned"),
            Some(&serde_yaml::Value::Bool(false))
        );
    }

    #[tokio::test]
    async fn append_refuses_invalid_existing_frontmatter() {
        let dir = tempdir().unwrap();
        let ctx = make_ctx(dir.path().to_path_buf());
        std::fs::write(dir.path().join("bad.md"), "---\nid: [\n---\nbody\n").unwrap();
        let error = execute(&ctx, &write_args("bad.md", "append", "more"))
            .await
            .unwrap_err();
        assert!(format!("{error:#}").contains("frontmatter"), "{error:#}");
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

    #[tokio::test]
    async fn successful_write_refreshes_vault_graph_neighbors() {
        let dir = tempdir().unwrap();
        let root = dir.path().to_path_buf();
        std::fs::write(root.join("b.md"), "---\nid: doc-b\n---\n# B\n").unwrap();
        let store = crate::store::Store::init(&root.join("db").to_string_lossy(), "memory")
            .await
            .unwrap();
        let mut ctx = make_ctx(root.clone());
        ctx.code_path = Some(root.clone());
        ctx.store = Some(store);

        let mut args = serde_json::Map::new();
        args.insert("path".into(), serde_json::json!("a.md"));
        args.insert("mode".into(), serde_json::json!("create"));
        args.insert(
            "content".into(),
            serde_json::json!("---\nid: doc-a\n---\n[Read B](b.md)\n"),
        );
        execute(&ctx, &args).await.expect("write");

        let neighbors =
            crate::tools::graph::neighbors_for_path(ctx.store.as_ref().unwrap(), "a.md", 8)
                .await
                .expect("neighbors");
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0]["direction"], "out");
        assert_eq!(neighbors[0]["neighbor"]["path"], "b.md");

        args.insert("mode".into(), serde_json::json!("replace"));
        args.insert(
            "content".into(),
            serde_json::json!("---\nid: doc-a\n---\n[Read B](b.md)\nUpdated.\n"),
        );
        let response = execute(&ctx, &args).await.expect("rewrite");
        assert_eq!(
            response["structuredContent"]["graph_refresh"]["status"],
            "ok"
        );
        assert!(
            response["structuredContent"]["message"]
                .as_str()
                .unwrap()
                .contains("Successfully wrote")
        );
    }
}
