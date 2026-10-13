//! Refuses MCP writes whose text contains credentials with a known provider
//! prefix. The text is never rewritten: the caller gets an error naming the
//! field, kind and line, and is expected to remove the secret and retry.

use anyhow::{Result, bail};
use serde_json::{Map, Value};

pub fn reject_secrets(args: &Map<String, Value>, fields: &[&str]) -> Result<()> {
    for field in fields {
        let Some(text) = args.get(*field).and_then(Value::as_str) else {
            continue;
        };
        let findings = rms_memory_core::secrets::scan(text);
        if !findings.is_empty() {
            bail!(
                "Refusing to save: `{field}` contains what looks like a credential: {}. Remove it (reference an env var or secret manager instead) and retry.",
                rms_memory_core::secrets::describe(&findings)
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rejects_without_echoing_the_secret() {
        let token = format!("ghp_{}", "a1B2".repeat(9));
        let mut args = Map::new();
        args.insert("content".into(), json!(format!("ci\nuse {token}")));
        let error = reject_secrets(&args, &["content"]).unwrap_err().to_string();
        assert!(error.contains("github_token (line 2)"), "{error}");
        assert!(!error.contains(&token));
    }

    #[test]
    fn clean_text_passes() {
        let mut args = Map::new();
        args.insert("content".into(), json!("password: see 1Password"));
        reject_secrets(&args, &["content"]).unwrap();
    }
}
