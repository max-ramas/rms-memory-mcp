//! One-way copy of an existing Obsidian vault into an RMS Memory vault.
//!
//! The source vault is only read. Notes and attachments are copied byte for
//! byte with their folder structure, so relative Markdown links and
//! path-qualified `[[folder/Note]]` links keep resolving; no `id` or other
//! frontmatter is added (notes without `id` are indexed by path). Hidden
//! entries (`.obsidian/`, `.trash/`, `.git/`) and symlinks are skipped,
//! existing vault files are never overwritten, and text files containing a
//! known credential format are left out.

use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

/// Text files above this size are copied without a credential scan.
const SCAN_LIMIT_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Default, Serialize)]
pub struct ObsidianImportReport {
    pub source: String,
    pub destination: String,
    pub dry_run: bool,
    /// Vault-relative paths of notes that were (or would be) written.
    pub notes: Vec<String>,
    pub attachments: Vec<String>,
    /// Destination already holds identical content.
    pub unchanged: Vec<String>,
    /// Destination exists with different content; left untouched.
    pub conflicts: Vec<String>,
    /// Notes whose frontmatter is not valid YAML (copied, indexed as plain text).
    pub invalid_frontmatter: Vec<String>,
    /// `path: kind (line n), ...` for files left out because of credentials.
    pub skipped_secrets: Vec<String>,
    pub skipped_hidden: usize,
    pub skipped_symlinks: usize,
}

/// Canonicalizes the deepest existing ancestor so not-yet-created vaults still
/// compare correctly against canonical source paths.
fn canonical_lenient(path: &Path) -> PathBuf {
    let mut missing = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        if let Ok(canonical) = std::fs::canonicalize(&current) {
            return missing
                .into_iter()
                .rev()
                .fold(canonical, |acc, part| acc.join(part));
        }
        match (
            current.file_name().map(|n| n.to_os_string()),
            current.parent(),
        ) {
            (Some(name), Some(parent)) => {
                missing.push(name);
                current = parent.to_path_buf();
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// Validates `into` as a vault-relative folder without traversal.
fn destination_root(vault_root: &Path, into: &str) -> Result<PathBuf> {
    let relative = Path::new(into.trim_end_matches('/'));
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("--into must be a relative folder inside the vault, got '{into}'");
    }
    if relative
        .components()
        .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
    {
        bail!("--into must not be a hidden folder, got '{into}'");
    }
    Ok(vault_root.join(relative))
}

fn same_content(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) if ma.len() == mb.len() => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
        }
        _ => false,
    }
}

fn frontmatter_is_valid(text: &str) -> bool {
    use rms_memory_core::frontmatter;
    if !(text.starts_with("---\n") || text.starts_with("---\r\n")) {
        return true;
    }
    frontmatter::split(text).is_some_and(|split| frontmatter::parse_mapping(split.yaml).is_ok())
}

pub fn import_obsidian_vault(
    source: &Path,
    vault_root: &Path,
    into: &str,
    dry_run: bool,
) -> Result<ObsidianImportReport> {
    let source = std::fs::canonicalize(source)
        .with_context(|| format!("Obsidian vault not found: {}", source.display()))?;
    if !source.is_dir() {
        bail!("Obsidian vault is not a directory: {}", source.display());
    }
    let vault = canonical_lenient(vault_root);
    if source.starts_with(&vault) || vault.starts_with(&source) {
        bail!(
            "Obsidian vault {} and RMS vault {} must not contain each other",
            source.display(),
            vault.display()
        );
    }
    if !dry_run {
        rms_memory_core::workspace::ensure_vault_writable(vault_root)?;
    }
    let destination = destination_root(&vault, into)?;
    let mut report = ObsidianImportReport {
        source: source.display().to_string(),
        destination: destination.display().to_string(),
        dry_run,
        ..Default::default()
    };

    let mut walker = walkdir::WalkDir::new(&source)
        .follow_links(false)
        .sort_by_file_name()
        .into_iter();
    while let Some(entry) = walker.next() {
        let entry = entry.with_context(|| format!("Failed to scan {}", source.display()))?;
        if entry.depth() == 0 {
            continue;
        }
        if entry.file_name().to_string_lossy().starts_with('.') {
            report.skipped_hidden += 1;
            if entry.file_type().is_dir() {
                walker.skip_current_dir();
            }
            continue;
        }
        if entry.path_is_symlink() {
            report.skipped_symlinks += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(&source)?.to_path_buf();
        let target = destination.join(&relative);
        let display = target
            .strip_prefix(&vault)
            .unwrap_or(&target)
            .to_string_lossy()
            .replace('\\', "/");
        let is_note = relative
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));

        let size = entry.metadata().map(|m| m.len()).unwrap_or(u64::MAX);
        if size <= SCAN_LIMIT_BYTES
            && let Ok(bytes) = std::fs::read(entry.path())
            && let Ok(text) = std::str::from_utf8(&bytes)
        {
            let findings = rms_memory_core::secrets::scan(text);
            if !findings.is_empty() {
                report.skipped_secrets.push(format!(
                    "{display}: {}",
                    rms_memory_core::secrets::describe(&findings)
                ));
                continue;
            }
            if is_note && !frontmatter_is_valid(text) {
                report.invalid_frontmatter.push(display.clone());
            }
        }

        if target.exists() {
            if same_content(entry.path(), &target) {
                report.unchanged.push(display);
            } else {
                report.conflicts.push(display);
            }
            continue;
        }
        if !dry_run {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("Failed to create {}", parent.display()))?;
            }
            std::fs::copy(entry.path(), &target).with_context(|| {
                format!(
                    "Failed to copy {} to {}",
                    entry.path().display(),
                    target.display()
                )
            })?;
        }
        if is_note {
            report.notes.push(display);
        } else {
            report.attachments.push(display);
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obsidian_fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::create_dir_all(root.join(".trash")).unwrap();
        std::fs::create_dir_all(root.join("Projects/assets")).unwrap();
        std::fs::write(root.join(".obsidian/app.json"), "{}").unwrap();
        std::fs::write(root.join(".trash/Old.md"), "old").unwrap();
        std::fs::write(
            root.join("Projects/Payments.md"),
            "---\ntags: [finance]\naliases: Billing\n---\nSee [[Inbox]] ![[diagram.png]]\n",
        )
        .unwrap();
        std::fs::write(root.join("Inbox.md"), "# Inbox\n").unwrap();
        std::fs::write(root.join("Board.canvas"), "{\"nodes\":[]}").unwrap();
        std::fs::write(root.join("Broken.md"), "---\ntags: [unclosed\n---\nbody\n").unwrap();
        std::fs::write(root.join("Projects/assets/diagram.png"), b"png").unwrap();
        dir
    }

    fn tree_digest(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files: Vec<_> = walkdir::WalkDir::new(root)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| {
                (
                    e.path().strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(e.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    }

    #[test]
    fn copies_notes_verbatim_and_never_touches_the_source() {
        let source = obsidian_fixture();
        let before = tree_digest(source.path());
        let vault = tempfile::tempdir().unwrap();

        let report = import_obsidian_vault(source.path(), vault.path(), "obsidian", false).unwrap();

        assert_eq!(report.notes.len(), 3);
        assert_eq!(
            report.attachments,
            vec![
                "obsidian/Board.canvas",
                "obsidian/Projects/assets/diagram.png"
            ]
        );
        assert_eq!(report.invalid_frontmatter, vec!["obsidian/Broken.md"]);
        assert_eq!(report.skipped_hidden, 2);
        assert!(!vault.path().join("obsidian/.obsidian").exists());
        assert!(!vault.path().join("obsidian/.trash").exists());
        assert_eq!(
            std::fs::read(vault.path().join("obsidian/Projects/Payments.md")).unwrap(),
            std::fs::read(source.path().join("Projects/Payments.md")).unwrap()
        );
        assert_eq!(tree_digest(source.path()), before);
    }

    #[test]
    fn files_with_credentials_are_left_out() {
        let source = obsidian_fixture();
        std::fs::create_dir_all(source.path().join("Credentials")).unwrap();
        std::fs::write(
            source.path().join("Credentials/Server.md"),
            "# Server\n\ntoken ghp_abcdefghijklmnopqrstuvwxyz0123456789\n",
        )
        .unwrap();
        std::fs::write(source.path().join("Credentials/plain.md"), "password: x\n").unwrap();

        let vault = tempfile::tempdir().unwrap();
        let report = import_obsidian_vault(source.path(), vault.path(), "obsidian", false).unwrap();
        assert_eq!(
            report.skipped_secrets,
            vec!["obsidian/Credentials/Server.md: github_token (line 3)"]
        );
        assert!(!vault.path().join("obsidian/Credentials/Server.md").exists());
        assert!(vault.path().join("obsidian/Credentials/plain.md").exists());
    }

    #[test]
    fn rerun_is_idempotent_and_never_overwrites() {
        let source = obsidian_fixture();
        let vault = tempfile::tempdir().unwrap();
        import_obsidian_vault(source.path(), vault.path(), "obsidian", false).unwrap();
        std::fs::write(vault.path().join("obsidian/Inbox.md"), "edited in RMS\n").unwrap();

        let report = import_obsidian_vault(source.path(), vault.path(), "obsidian", false).unwrap();

        assert!(report.notes.is_empty());
        assert_eq!(report.conflicts, vec!["obsidian/Inbox.md"]);
        assert_eq!(report.unchanged.len(), 4);
        assert_eq!(
            std::fs::read_to_string(vault.path().join("obsidian/Inbox.md")).unwrap(),
            "edited in RMS\n"
        );
    }

    #[test]
    fn dry_run_writes_nothing_and_rejects_unsafe_destinations() {
        let source = obsidian_fixture();
        let vault = tempfile::tempdir().unwrap();

        let report = import_obsidian_vault(source.path(), vault.path(), "obsidian", true).unwrap();
        assert_eq!(report.notes.len(), 3);
        assert!(!vault.path().join("obsidian").exists());

        for into in ["../outside", "/abs", ".hidden"] {
            assert!(import_obsidian_vault(source.path(), vault.path(), into, false).is_err());
        }
        assert!(
            import_obsidian_vault(vault.path(), &vault.path().join("nested"), "x", false).is_err()
        );
    }
}
