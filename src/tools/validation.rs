use anyhow::Result;
use std::path::{Component, Path, PathBuf};

/// Validate that a path is relative, ends in `.md`, and does not escape the
/// vault root. Rejects absolute paths, `..` components, symlink traversal,
/// and non-Markdown extensions.
pub fn validate_path(path_str: &str) -> Result<()> {
    let path = Path::new(path_str);
    if path.is_absolute() {
        return Err(anyhow::anyhow!(
            "Path must be relative to the vault, but received absolute path: {}",
            path_str
        ));
    }
    for component in path.components() {
        if component == Component::ParentDir {
            return Err(anyhow::anyhow!(
                "Path traversal detected: '..' is not allowed in vault paths"
            ));
        }
    }
    if !path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
    {
        return Err(anyhow::anyhow!(
            "Vault paths must point at Markdown (.md) documents: {}",
            path_str
        ));
    }
    Ok(())
}

/// Reject vault-relative paths inside the generated Wiki namespace. Wiki
/// pages have their own write channel (`WikiDocumentService`) that skips
/// audit metadata; the canonical MCP tools must never touch them.
pub fn reject_wiki_write(path_str: &str) -> Result<()> {
    if crate::path_policy::is_vault_wiki_relative_path(path_str) {
        return Err(anyhow::anyhow!(
            "Path '{path_str}' is inside the generated Wiki namespace and cannot be written through the canonical memory tools. Use the Wiki-specific write API instead.",
        ));
    }
    Ok(())
}

/// Resolve a relative path within `workspace_root` and validate it stays within bounds.
/// Also resolves symlinks to ensure the final path is within the vault.
pub fn resolve_vault_path(workspace_root: &Path, path_str: &str) -> Result<PathBuf> {
    validate_path(path_str)?;
    let joined = workspace_root.join(path_str);
    let canonical_root =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical = std::fs::canonicalize(&joined).unwrap_or_else(|_| joined.clone());
    if !canonical.starts_with(&canonical_root) {
        return Err(anyhow::anyhow!(
            "Path escapes vault boundary: resolved to outside workspace root"
        ));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_path_requires_markdown_extension() {
        assert!(validate_path("rules/api.md").is_ok());
        assert!(validate_path("Notes/2024-01.MD").is_ok());
        let error = validate_path("secret.txt").unwrap_err().to_string();
        assert!(error.contains("Markdown"));
        assert!(validate_path("no-extension").is_err());
    }

    #[test]
    fn reject_wiki_write_blocks_wiki_paths() {
        assert!(reject_wiki_write("wiki/index.md").is_err());
        assert!(reject_wiki_write("./Wiki/guide.md").is_err());
        assert!(reject_wiki_write("wiki\\page.md").is_err());
        assert!(reject_wiki_write("docs/wiki.md").is_ok());
    }
}
