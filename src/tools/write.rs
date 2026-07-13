use super::AppContext;
use anyhow::{Context, Result};
use std::io::Write;
use std::path::Path;

/// Replaces a Markdown document without exposing a partially truncated target
/// file. The temporary file is created in the target directory so the final
/// rename remains atomic on supported local filesystems.
fn write_atomic(path: &Path, content: &str) -> Result<()> {
    let parent = path
        .parent()
        .context("Cannot atomically write a path without a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content.as_bytes())?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    std::fs::File::open(parent)
        .and_then(|directory| directory.sync_all())
        .ok();
    Ok(())
}

fn inject_audit_metadata(
    content: &str,
    caller_id: &str,
    args: &serde_json::Map<String, serde_json::Value>,
) -> String {
    use chrono::Utc;

    let now = Utc::now().to_rfc3339();
    let conf_value = args.get("confidence").and_then(|v| v.as_f64());

    if content.starts_with("---\n") || content.starts_with("---\r\n") {
        // Existing frontmatter — parse and patch
        if let Some(end_idx) = content
            .find("\n---\n")
            .or_else(|| content.find("\r\n---\r\n"))
        {
            let fm_text = &content[4..end_idx];
            if let Ok(mut fm) = serde_yaml::from_str::<crate::document::Frontmatter>(fm_text) {
                let existing_created = fm.created_at.clone();
                if fm.id.is_none() {
                    fm.id = Some(uuid::Uuid::new_v4().to_string());
                }
                fm.last_modified_by = Some(caller_id.to_string());
                fm.timestamp = Some(now.clone());
                if existing_created.is_none() {
                    fm.created_at = Some(now);
                }
                if let Some(c) = conf_value
                    && (0.0..=1.0).contains(&c)
                {
                    fm.confidence = Some(c);
                }
                if let Some(s) = args.get("source").and_then(|v| v.as_str()) {
                    fm.source = Some(s.to_string());
                }
                let updated_fm = serde_yaml::to_string(&fm)
                    .unwrap_or_default()
                    .trim_end()
                    .to_string();
                let remainder = &content[end_idx..];
                return format!("---\n{}\n{}", updated_fm, remainder);
            }
        }
    }

    // No existing frontmatter — create new
    let fm = crate::document::Frontmatter {
        memory_version: None,
        id: Some(uuid::Uuid::new_v4().to_string()),
        alias: None,
        doc_type: None,
        status: None,
        link: None,
        last_modified_by: Some(caller_id.to_string()),
        timestamp: Some(now.clone()),
        created_at: Some(now),
        confidence: conf_value.filter(|c| (0.0..=1.0).contains(c)),
        source: args
            .get("source")
            .and_then(|v| v.as_str())
            .map(String::from),
    };
    // Only serialize non-None optional fields
    let fm_yaml = serde_yaml::to_string(&fm)
        .unwrap_or_default()
        .trim_end()
        .to_string();
    if content.is_empty() {
        format!("---\n{}\n---\n", fm_yaml)
    } else {
        format!("---\n{}\n---\n\n{}", fm_yaml, content)
    }
}

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let workspace_root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let initial_file_path = super::validation::resolve_vault_path(workspace_root, path_str)?;
    let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let mode = args
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("replace");

    let file_path = crate::link::resolve_link(&initial_file_path);

    // CREATE mode: reject if file already exists
    if mode == "create" && file_path.exists() {
        return Err(anyhow::anyhow!(
            "File already exists: {}. Use mode='replace' or 'append' to modify existing files.",
            path_str
        ));
    }

    // Inject audit metadata into frontmatter
    let content = inject_audit_metadata(content, &ctx.caller_id, args);

    // WRITE-GUARD: Backup file if it exists
    if file_path.exists() && ctx.max_backups > 0 {
        let mut backups = Vec::new();
        let parent = file_path.parent().unwrap_or(std::path::Path::new(""));
        let base_name = file_path.file_name().unwrap_or_default().to_string_lossy();

        // Discover existing backups
        if let Ok(entries) = std::fs::read_dir(parent) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with(&format!("{}.bak.", base_name)) {
                    backups.push(entry.path());
                }
            }
        }

        // Sort by modification time (oldest first)
        backups.sort_by_key(|a| {
            std::fs::metadata(a)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });

        // Keep up to max_backups - 1 before adding the new one
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
            let mut f = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&file_path)?;
            f.write_all(content.as_bytes())?;
        }
        "create" | "replace" => {
            write_atomic(&file_path, &content)?;
        }
        m => {
            return Err(anyhow::anyhow!(
                "Unknown write mode '{}'. Valid modes: create, append, replace",
                m
            ));
        }
    }

    Ok(super::response::json_text_response(&format!(
        "Successfully wrote to {}",
        path_str
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_frontmatter_keeps_source_and_closing_delimiter_on_separate_lines() {
        let args = serde_json::json!({
            "source": "release gate",
            "confidence": 0.9,
        });
        let content = inject_audit_metadata(
            "# Release gate",
            "test-client",
            args.as_object().expect("JSON object"),
        );

        assert!(content.contains("source: release gate\n---\n\n# Release gate"));
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("memory.md");
        std::fs::write(&path, content).expect("write test document");
        let document = crate::document::Document::parse(&path).expect("valid frontmatter");
        assert!(
            document
                .frontmatter
                .and_then(|frontmatter| frontmatter.id)
                .is_some()
        );
    }

    #[test]
    fn repeated_metadata_injection_keeps_one_id_and_the_complete_body() {
        let args = serde_json::Map::new();
        let first = inject_audit_metadata("# Complete body\n\nDo not truncate.", "writer-a", &args);
        let second = inject_audit_metadata(&first, "writer-b", &args);
        assert_eq!(second.matches("\nid:").count(), 1);
        assert!(second.ends_with("# Complete body\n\nDo not truncate."));
    }

    #[test]
    fn atomic_replace_never_exposes_a_frontmatter_only_result() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("memory.md");
        std::fs::write(&path, "old body").unwrap();
        let replacement = "---\nid: stable-id\n---\n\n# Complete replacement body\n";

        write_atomic(&path, replacement).unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), replacement);
        let document = crate::document::Document::parse(&path).unwrap();
        assert_eq!(
            document.frontmatter.unwrap().id.as_deref(),
            Some("stable-id")
        );
        assert_eq!(document.content, "\n# Complete replacement body\n");
    }
}
