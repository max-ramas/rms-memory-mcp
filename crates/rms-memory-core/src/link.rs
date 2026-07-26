use crate::document::Document;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

fn is_safe_link(link: &str) -> bool {
    if link.is_empty() || link.starts_with('/') {
        return false;
    }
    !link.split('/').any(|c| c == "..")
}

/// Canonicalize `path` (which may point at a symlink) and require the
/// resulting real path to remain inside `vault_root`.
fn canonicalize_inside(path: &Path, vault_root: &Path) -> Result<PathBuf> {
    let canonical_root =
        std::fs::canonicalize(vault_root).unwrap_or_else(|_| vault_root.to_path_buf());
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| anyhow::anyhow!("Failed to canonicalize link target: {error}"))?;
    if !canonical.starts_with(&canonical_root) {
        bail!(
            "Link target escapes vault boundary: {}",
            canonical.display()
        );
    }
    Ok(canonical)
}

/// Resolves a linked document while guaranteeing the resolved path stays
/// inside `vault_root`. If the file at `file_path` contains a `link` in its
/// frontmatter, returns the resolved canonical path. Otherwise, returns the
/// original `file_path` unmodified.
///
/// Returns an error if the link is malformed or points outside the vault
/// (including via symlinks).
pub fn resolve_link_in_vault(file_path: &Path, vault_root: &Path) -> Result<PathBuf> {
    let Ok(doc) = Document::parse(file_path) else {
        return Ok(file_path.to_path_buf());
    };
    let Some(fm) = doc.frontmatter else {
        return Ok(file_path.to_path_buf());
    };
    let Some(link) = fm.link else {
        return Ok(file_path.to_path_buf());
    };
    if !is_safe_link(&link) {
        bail!("Unsafe link value in frontmatter: {link}");
    }
    let Some(parent) = file_path.parent() else {
        return Ok(file_path.to_path_buf());
    };
    let resolved = parent.join(&link);
    canonicalize_inside(&resolved, vault_root)
}

/// Vault-aware variant of [`resolve_link_in_vault`] that swallows errors and
/// returns the original path unchanged. Use this only in read paths where a
/// malformed link should be treated as "no link" (e.g. best-effort indexing).
/// Any escape attempt still returns the original path (not the escaping one).
pub fn resolve_link_in_vault_or_self(file_path: &Path, vault_root: &Path) -> PathBuf {
    resolve_link_in_vault(file_path, vault_root).unwrap_or_else(|_| file_path.to_path_buf())
}

/// Checks if a file is a linked document and returns the source content if so.
/// The linked source is required to live inside `vault_root`; escapes return `None`.
pub fn get_linked_content_in_vault(file_path: &Path, vault_root: &Path) -> Option<String> {
    let resolved = resolve_link_in_vault(file_path, vault_root).ok()?;
    if resolved == file_path {
        // Not a link; the caller reads the file directly.
        return None;
    }
    std::fs::read_to_string(&resolved).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_resolve_link_no_frontmatter() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        fs::write(&file_path, "# Just content").unwrap();

        let resolved = resolve_link_in_vault(&file_path, dir.path()).unwrap();
        assert_eq!(resolved.file_name(), file_path.file_name());
    }

    #[test]
    fn test_resolve_link_with_frontmatter_inside_vault() {
        let dir = tempdir().unwrap();
        let source_path = dir.path().join("source.md");
        let target_path = dir.path().join("target.md");

        fs::write(&source_path, "Source content").unwrap();
        fs::write(&target_path, "---\nlink: source.md\n---\nLinked content").unwrap();

        let resolved = resolve_link_in_vault(&target_path, dir.path()).unwrap();
        assert_eq!(resolved.file_name(), source_path.file_name());
    }

    #[test]
    fn test_resolve_link_rejects_traversal_string() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("target.md");
        fs::write(&target_path, "---\nlink: ../../etc/passwd\n---\n").unwrap();

        let error = resolve_link_in_vault(&target_path, dir.path()).unwrap_err();
        assert!(error.to_string().contains("Unsafe link"));
    }

    #[test]
    fn test_resolve_link_rejects_absolute_link() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("target.md");
        fs::write(&target_path, "---\nlink: /etc/hosts\n---\n").unwrap();

        assert!(resolve_link_in_vault(&target_path, dir.path()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_link_rejects_symlink_target_outside_vault() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_secret = outside.path().join("secret.md");
        fs::write(&outside_secret, "TOP SECRET").unwrap();

        let escape_link = vault.path().join("escape.md");
        std::os::unix::fs::symlink(&outside_secret, &escape_link).unwrap();

        let doc = vault.path().join("doc.md");
        fs::write(&doc, "---\nlink: escape.md\n---\n").unwrap();

        let error = resolve_link_in_vault(&doc, vault.path()).unwrap_err();
        assert!(
            error.to_string().contains("escapes vault boundary"),
            "unexpected error: {error}"
        );

        // The forgiving helper must still refuse to return the escaping path.
        let fallback = resolve_link_in_vault_or_self(&doc, vault.path());
        assert_eq!(fallback, doc);

        // Linked content read must return None for escaping symlink targets.
        assert!(get_linked_content_in_vault(&doc, vault.path()).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_linked_content_reads_only_inside_vault() {
        let vault = tempdir().unwrap();
        let source = vault.path().join("source.md");
        fs::write(&source, "Real source content").unwrap();
        let target = vault.path().join("target.md");
        fs::write(&target, "---\nlink: source.md\n---\n").unwrap();

        let content = get_linked_content_in_vault(&target, vault.path()).unwrap();
        assert_eq!(content, "Real source content");
    }
}
