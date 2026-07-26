//! Frontmatter audit metadata helpers (vault-side; no MCP tools dependency).
use anyhow::Result;

/// Inject / refresh RMS audit fields in Markdown frontmatter.
pub fn inject_audit_metadata(
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
            if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
                mapping.insert(
                    serde_yaml::Value::String("status".into()),
                    serde_yaml::Value::String(status.to_string()),
                );
            }
            if let Some(pinned) = args.get("pinned").and_then(|v| v.as_bool()) {
                mapping.insert(
                    serde_yaml::Value::String("pinned".into()),
                    serde_yaml::Value::Bool(pinned),
                );
            }
            if let Some(supersedes) = args.get("supersedes_id").and_then(|v| v.as_str()) {
                mapping.insert(
                    serde_yaml::Value::String("supersedes".into()),
                    serde_yaml::Value::String(supersedes.to_string()),
                );
                if mapping.get("status").is_none() {
                    mapping.insert(
                        serde_yaml::Value::String("status".into()),
                        serde_yaml::Value::String("active".into()),
                    );
                }
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
    if let Some(status) = args.get("status").and_then(|v| v.as_str()) {
        mapping.insert(
            serde_yaml::Value::String("status".into()),
            serde_yaml::Value::String(status.to_string()),
        );
    }
    if let Some(pinned) = args.get("pinned").and_then(|v| v.as_bool()) {
        mapping.insert(
            serde_yaml::Value::String("pinned".into()),
            serde_yaml::Value::Bool(pinned),
        );
    }
    if let Some(supersedes) = args.get("supersedes_id").and_then(|v| v.as_str()) {
        mapping.insert(
            serde_yaml::Value::String("supersedes".into()),
            serde_yaml::Value::String(supersedes.to_string()),
        );
        if mapping.get("status").is_none() {
            mapping.insert(
                serde_yaml::Value::String("status".into()),
                serde_yaml::Value::String("active".into()),
            );
        }
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
