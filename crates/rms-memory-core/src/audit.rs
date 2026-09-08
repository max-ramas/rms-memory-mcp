//! Frontmatter audit metadata helpers (vault-side; no MCP tools dependency).
use anyhow::Result;

/// Frontmatter keys rewritten on every `inject_audit_metadata` call.
/// Excluded from [`content_fingerprint`] so stamp-only refreshes classify as noop
/// (see `decisions/rms-write-dry-run-2026-09-08.md`).
pub const VOLATILE_AUDIT_KEYS: &[&str] = &["timestamp", "last_modified_by"];

/// Stable content fingerprint for dry_run create/update/noop classification.
///
/// Strips [`VOLATILE_AUDIT_KEYS`] from YAML frontmatter, sorts remaining keys,
/// normalizes newlines in the body, then returns a blake3 hex digest.
pub fn content_fingerprint(markdown: &str) -> String {
    let normalized = normalize_for_fingerprint(markdown);
    blake3::hash(normalized.as_bytes()).to_hex().to_string()
}

fn normalize_for_fingerprint(markdown: &str) -> String {
    let text = markdown.replace("\r\n", "\n").replace('\r', "\n");
    if (text.starts_with("---\n"))
        && let Some(end_idx) = text.find("\n---\n")
    {
        let fm_text = &text[4..end_idx];
        let body = text[end_idx + "\n---\n".len()..].trim_start_matches('\n');
        if let Ok(mapping) = serde_yaml::from_str::<serde_yaml::Mapping>(fm_text) {
            let mut pairs: Vec<(String, String)> = mapping
                .iter()
                .filter_map(|(k, v)| {
                    let key = k.as_str()?;
                    if VOLATILE_AUDIT_KEYS.contains(&key) {
                        return None;
                    }
                    let value = serde_yaml::to_string(v).ok()?.trim().to_string();
                    Some((key.to_string(), value))
                })
                .collect();
            pairs.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = String::from("FM\n");
            for (k, v) in pairs {
                out.push_str(&k);
                out.push('\t');
                out.push_str(&v);
                out.push('\n');
            }
            out.push_str("BODY\n");
            out.push_str(body);
            return out;
        }
    }
    format!("BODY\n{text}")
}

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
                let id = args
                    .get("id")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
                mapping.insert(
                    serde_yaml::Value::String("id".into()),
                    serde_yaml::Value::String(id),
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
    let id = args
        .get("id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    mapping.insert(
        serde_yaml::Value::String("id".into()),
        serde_yaml::Value::String(id),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_ignores_volatile_audit_stamps() {
        let a = "---\nid: n1\ncreated_at: t0\ntimestamp: 2026-01-01T00:00:00Z\nlast_modified_by: alice\n---\n\nBody.\n";
        let b = "---\nid: n1\ncreated_at: t0\ntimestamp: 2026-09-08T12:00:00Z\nlast_modified_by: bob\n---\n\nBody.\n";
        assert_eq!(content_fingerprint(a), content_fingerprint(b));
    }

    #[test]
    fn fingerprint_changes_when_body_changes() {
        let a = "---\nid: n1\n---\n\nBody A.\n";
        let b = "---\nid: n1\n---\n\nBody B.\n";
        assert_ne!(content_fingerprint(a), content_fingerprint(b));
    }

    #[test]
    fn fingerprint_changes_when_stable_frontmatter_changes() {
        let a = "---\nid: n1\nstatus: active\n---\n\nBody.\n";
        let b = "---\nid: n1\nstatus: draft\n---\n\nBody.\n";
        assert_ne!(content_fingerprint(a), content_fingerprint(b));
    }
}
