use crate::document::Document;
use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

fn is_safe_link(link: &str) -> bool {
    !link.is_empty() && !link.starts_with('/')
}

fn canonical_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Canonicalize `path` (which may point at a symlink) and require the
/// resulting real path to remain inside one of the allowed roots.
fn canonicalize_inside_allowed(path: &Path, allowed_roots: &[PathBuf]) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path)
        .map_err(|error| anyhow::anyhow!("Failed to canonicalize link target: {error}"))?;
    if allowed_roots
        .iter()
        .any(|root| canonical.starts_with(root))
    {
        return Ok(canonical);
    }
    bail!(
        "Link target escapes allowed project boundaries: {}",
        canonical.display()
    );
}

fn allowed_roots(vault_root: &Path, code_path: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = vec![canonical_root(vault_root)];
    if let Some(code_path) = code_path {
        roots.push(canonical_root(code_path));
    }
    roots
}

/// Resolves a linked document while guaranteeing the resolved path stays
/// inside the vault or the registered project code path. If the file at
/// `file_path` contains a `link` in its frontmatter, returns the resolved
/// canonical path. Otherwise, returns the original `file_path` unmodified.
///
/// Returns an error if the link is malformed or points outside allowed roots
/// (including via symlinks).
pub fn resolve_link_in_vault(
    file_path: &Path,
    vault_root: &Path,
    code_path: Option<&Path>,
) -> Result<PathBuf> {
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
    canonicalize_inside_allowed(&resolved, &allowed_roots(vault_root, code_path))
}

/// Vault-aware variant of [`resolve_link_in_vault`] that swallows errors and
/// returns the original path unchanged. Use this only in read paths where a
/// malformed link should be treated as "no link" (e.g. best-effort indexing).
/// Any escape attempt still returns the original path (not the escaping one).
pub fn resolve_link_in_vault_or_self(
    file_path: &Path,
    vault_root: &Path,
    code_path: Option<&Path>,
) -> PathBuf {
    resolve_link_in_vault(file_path, vault_root, code_path)
        .unwrap_or_else(|_| file_path.to_path_buf())
}

/// Checks if a file is a linked document and returns the source content if so.
/// The linked source must live inside the vault or registered code path;
/// escapes return `None`.
pub fn get_linked_content_in_vault(
    file_path: &Path,
    vault_root: &Path,
    code_path: Option<&Path>,
) -> Option<String> {
    let resolved = resolve_link_in_vault(file_path, vault_root, code_path).ok()?;
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

        let resolved = resolve_link_in_vault(&file_path, dir.path(), None).unwrap();
        assert_eq!(resolved.file_name(), file_path.file_name());
    }

    #[test]
    fn test_resolve_link_with_frontmatter_inside_vault() {
        let dir = tempdir().unwrap();
        let source_path = dir.path().join("source.md");
        let target_path = dir.path().join("target.md");

        fs::write(&source_path, "Source content").unwrap();
        fs::write(&target_path, "---\nlink: source.md\n---\nLinked content").unwrap();

        let resolved = resolve_link_in_vault(&target_path, dir.path(), None).unwrap();
        assert_eq!(resolved.file_name(), source_path.file_name());
    }

    #[test]
    fn test_resolve_link_follows_code_path_via_parent_traversal() {
        let layout = tempdir().unwrap();
        let code = layout.path().join("repo");
        let vault = layout.path().join("vault");
        fs::create_dir_all(&code).unwrap();
        fs::create_dir_all(vault.join("guides")).unwrap();
        fs::write(code.join("README.md"), "# Project README").unwrap();
        let stub = vault.join("guides/README.md");
        fs::write(
            &stub,
            "---\nlink: ../../repo/README.md\n---\n",
        )
        .unwrap();

        let resolved =
            resolve_link_in_vault(&stub, &vault, Some(&code)).expect("repo link should resolve");
        assert_eq!(resolved, fs::canonicalize(code.join("README.md")).unwrap());

        let content = get_linked_content_in_vault(&stub, &vault, Some(&code)).unwrap();
        assert_eq!(content, "# Project README");
    }

    #[test]
    fn test_resolve_link_rejects_traversal_outside_allowed_roots() {
        let layout = tempdir().unwrap();
        let vault = layout.path().join("vault");
        let outside = layout.path().join("outside");
        fs::create_dir_all(&vault).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(outside.join("secret.md"), "nope").unwrap();
        let target_path = vault.join("target.md");
        fs::write(&target_path, "---\nlink: ../outside/secret.md\n---\n").unwrap();

        let error = resolve_link_in_vault(&target_path, &vault, None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("escapes allowed project boundaries"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn test_resolve_link_rejects_absolute_link() {
        let dir = tempdir().unwrap();
        let target_path = dir.path().join("target.md");
        fs::write(&target_path, "---\nlink: /etc/hosts\n---\n").unwrap();

        assert!(resolve_link_in_vault(&target_path, dir.path(), None).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_resolve_link_rejects_symlink_target_outside_allowed_roots() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_secret = outside.path().join("secret.md");
        fs::write(&outside_secret, "TOP SECRET").unwrap();

        let escape_link = vault.path().join("escape.md");
        std::os::unix::fs::symlink(&outside_secret, &escape_link).unwrap();

        let doc = vault.path().join("doc.md");
        fs::write(&doc, "---\nlink: escape.md\n---\n").unwrap();

        let error = resolve_link_in_vault(&doc, vault.path(), None).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("escapes allowed project boundaries"),
            "unexpected error: {error}"
        );

        let fallback = resolve_link_in_vault_or_self(&doc, vault.path(), None);
        assert_eq!(fallback, doc);

        assert!(get_linked_content_in_vault(&doc, vault.path(), None).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn test_linked_content_reads_only_inside_allowed_roots() {
        let vault = tempdir().unwrap();
        let source = vault.path().join("source.md");
        fs::write(&source, "Real source content").unwrap();
        let target = vault.path().join("target.md");
        fs::write(&target, "---\nlink: source.md\n---\n").unwrap();

        let content = get_linked_content_in_vault(&target, vault.path(), None).unwrap();
        assert_eq!(content, "Real source content");
    }
}
