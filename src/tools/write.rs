use super::AppContext;
use anyhow::Result;

fn inject_audit_metadata(
    content: &str,
    caller_id: &str,
    project_key: Option<&str>,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<String> {
    use chrono::Utc;

    let now = Utc::now().to_rfc3339();
    let conf_value = args.get("confidence").and_then(|v| v.as_f64());

    if (content.starts_with("---\n") || content.starts_with("---\r\n"))
        && let Some(end_idx) = content
            .find("\n---\n")
            .or_else(|| content.find("\r\n---\r\n"))
    {
        let fm_text = &content[4..end_idx];
        if let Ok(mut mapping) = serde_yaml::from_str::<serde_yaml::Mapping>(fm_text) {
            let existing_project = mapping.get("project").and_then(|v| v.as_str());

            if let Some(pk) = project_key
                && let Some(ep) = existing_project
                && ep != pk
            {
                return Err(anyhow::anyhow!(
                    "Project conflict: document belongs to '{}', current workspace is '{}'",
                    ep,
                    pk
                ));
            }

            if mapping.get("project").is_none()
                && let Some(pk) = project_key
            {
                mapping.insert(
                    serde_yaml::Value::String("project".into()),
                    serde_yaml::Value::String(pk.into()),
                );
            }

            if mapping.get("created_at").is_none() {
                mapping.insert(
                    serde_yaml::Value::String("created_at".into()),
                    serde_yaml::Value::String(now.clone()),
                );
            }

            mapping.insert(
                serde_yaml::Value::String("timestamp".into()),
                serde_yaml::Value::String(now.clone()),
            );
            mapping.insert(
                serde_yaml::Value::String("last_modified_by".into()),
                serde_yaml::Value::String(caller_id.to_string()),
            );

            if mapping.get("id").is_none() {
                mapping.insert(
                    serde_yaml::Value::String("id".into()),
                    serde_yaml::Value::String(uuid::Uuid::new_v4().to_string()),
                );
            }

            if let Some(c) = conf_value
                && (0.0..=1.0).contains(&c)
            {
                mapping.insert(
                    serde_yaml::Value::String("confidence".into()),
                    serde_yaml::Value::Number(c.into()),
                );
            }
            if let Some(s) = args.get("source").and_then(|v| v.as_str()) {
                mapping.insert(
                    serde_yaml::Value::String("source".into()),
                    serde_yaml::Value::String(s.to_string()),
                );
            }

            let updated_fm = serde_yaml::to_string(&mapping)
                .unwrap_or_default()
                .trim_end()
                .to_string();
            let remainder = &content[end_idx..];
            return Ok(format!("---\n{}\n{}", updated_fm, remainder));
        }
    }

    let mut mapping = serde_yaml::Mapping::new();
    mapping.insert(
        serde_yaml::Value::String("id".into()),
        serde_yaml::Value::String(uuid::Uuid::new_v4().to_string()),
    );
    mapping.insert(
        serde_yaml::Value::String("last_modified_by".into()),
        serde_yaml::Value::String(caller_id.to_string()),
    );
    mapping.insert(
        serde_yaml::Value::String("timestamp".into()),
        serde_yaml::Value::String(now.clone()),
    );
    mapping.insert(
        serde_yaml::Value::String("created_at".into()),
        serde_yaml::Value::String(now),
    );
    if let Some(pk) = project_key {
        mapping.insert(
            serde_yaml::Value::String("project".into()),
            serde_yaml::Value::String(pk.into()),
        );
    }
    if let Some(c) = conf_value.filter(|c| (0.0..=1.0).contains(c)) {
        mapping.insert(
            serde_yaml::Value::String("confidence".into()),
            serde_yaml::Value::Number(c.into()),
        );
    }
    if let Some(s) = args.get("source").and_then(|v| v.as_str()) {
        mapping.insert(
            serde_yaml::Value::String("source".into()),
            serde_yaml::Value::String(s.to_string()),
        );
    }

    let fm_yaml = serde_yaml::to_string(&mapping)
        .unwrap_or_default()
        .trim_end()
        .to_string();
    Ok(if content.is_empty() {
        format!("---\n{}\n---\n", fm_yaml)
    } else {
        format!("---\n{}\n---\n\n{}", fm_yaml, content)
    })
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

    if mode == "create" && file_path.exists() {
        return Err(anyhow::anyhow!(
            "File already exists: {}. Use mode='replace' or 'append' to modify existing files.",
            path_str
        ));
    }

    let content = inject_audit_metadata(content, &ctx.caller_id, ctx.project_key.as_deref(), args)?;

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

    Ok(super::response::json_text_response(&format!(
        "Successfully wrote to {}",
        path_str
    )))
}

#[cfg(test)]
mod tests {
    #[test]
    fn repeated_metadata_injection_keeps_one_id_and_the_complete_body() {
        let args = serde_json::Map::new();
        let first = super::inject_audit_metadata(
            "# Complete body\n\nDo not truncate.",
            "writer-a",
            Some("p"),
            &args,
        )
        .expect("first");
        let second =
            super::inject_audit_metadata(&first, "writer-b", Some("p"), &args).expect("second");
        assert_eq!(second.matches("\nid:").count(), 1);
        assert!(second.ends_with("# Complete body\n\nDo not truncate."));
    }
}
