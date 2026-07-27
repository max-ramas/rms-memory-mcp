use super::AppContext;
use crate::audit::inject_audit_metadata;
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
    super::validation::reject_wiki_write(path_str)?;
    let initial_file_path = super::validation::resolve_vault_path(workspace_root, path_str)?;
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("replace");

    // Resolve `link:` frontmatter to the source document, then verify the
    // resolved path (after any symlinks) is still inside the vault, and is
    // still outside the Wiki namespace.
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

    // Soft supersede: resolve predecessor before writing so we can stamp both sides.
    let supersedes_path = args.get("supersedes").and_then(|v| v.as_str());
    let mut write_args = args.clone();
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

    let content = inject_audit_metadata(
        content,
        &ctx.caller_id,
        ctx.project_key.as_deref(),
        &write_args,
    )?;

    if file_path.exists() && ctx.max_backups > 0 {
        let mut backups = Vec::new();
        let parent = file_path.parent().unwrap_or(std::path::Path::new(""));
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

        while backups.len() >= ctx.max_backups {
            if let Some(oldest) = backups.first() {
                let _ = std::fs::remove_file(oldest);
            }
            backups.remove(0);
        }

        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
        let bak_path = parent.join(format!("{}.bak.{}", base_name, timestamp));

        if let Err(e) = std::fs::copy(&file_path, &bak_path) {
            tracing::error!(
                "Write-Guard: Failed to create snapshot for {:?}: {}",
                file_path,
                e
            );
        } else {
            tracing::info!("Write-Guard: Created snapshot at {:?}", bak_path);
        }
    }

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    match mode {
        "append" => {
            use std::io::Write;
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file_path)?;
            f.write_all(content.as_bytes())?;
        }
        "create" | "replace" => {
            std::fs::write(&file_path, content)?;
        }
        m => {
            return Err(anyhow::anyhow!(
                "Unknown write mode '{}'. Valid modes: create, append, replace",
                m
            ));
        }
    }

    if let Some((old_path, _old_id)) = predecessor {
        let new_doc = crate::document::Document::parse(&file_path)?;
        let new_rel = file_path.strip_prefix(workspace_root).unwrap_or(&file_path);
        let new_id = new_doc.index_id(new_rel);
        mark_document_superseded(&old_path, &new_id)?;
    }

    Ok(super::response::json_text_response(&format!(
        "Successfully wrote to {}",
        path_str
    )))
}

fn mark_document_superseded(path: &std::path::Path, superseded_by: &str) -> Result<()> {
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

    fn make_ctx(root: std::path::PathBuf) -> AppContext {
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

        // Create a symlink inside the vault pointing outside, then a link file
        // that references it via a `link:` frontmatter.
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

        // The outside target must be untouched.
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
}
