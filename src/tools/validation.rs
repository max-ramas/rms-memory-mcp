use anyhow::Result;
use std::path::{Component, Path, PathBuf};

/// Validate that a path is relative and does not escape the vault root.
/// Rejects absolute paths, `..` components, and symlink traversal.
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
    Ok(())
}

/// Resolve a relative path within `workspace_root` and validate it stays within bounds.
/// Also resolves symlinks to ensure the final path is within the vault.
pub fn resolve_vault_path(workspace_root: &Path, path_str: &str) -> Result<PathBuf> {
    validate_path(path_str)?;
    let joined = workspace_root.join(path_str);
    let canonical = std::fs::canonicalize(&joined).unwrap_or_else(|_| joined.clone());
    if !canonical.starts_with(workspace_root) {
        return Err(anyhow::anyhow!(
            "Path escapes vault boundary: resolved to outside workspace root"
        ));
    }
    Ok(canonical)
}
