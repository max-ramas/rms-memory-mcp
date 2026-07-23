//! Safe document CRUD for desktop and API clients.
//!
//! This is deliberately separate from the MCP tool handlers: every caller gets
//! the same vault-boundary checks, linked-document behaviour, audit metadata,
//! conflict detection and rolling snapshots.
//!
//! # Canonical vs. Wiki namespaces
//!
//! The canonical methods (`list`, `read`, `write`, `create`, `rename`,
//! `delete`) refuse to touch anything in the generated Wiki namespace. Wiki
//! pages have their own set of methods (`read_wiki`, `write_wiki`,
//! `create_wiki`) that skip audit metadata injection so managed regions stay
//! intact. This keeps the corpus of "human memory" and the "generated wiki
//! output" strictly separate at the API layer.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

#[derive(Debug, Clone)]
pub struct DocumentService {
    root: PathBuf,
    caller_id: String,
    project_key: Option<String>,
    max_backups: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DocumentEntry {
    pub path: String,
    pub modified_at: Option<String>,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentRead {
    /// Requested vault-relative path (the link file when the document is linked).
    pub path: String,
    /// Content that will be edited. For a link this is the linked source content.
    pub content: String,
    /// BLAKE3 of the content that will be replaced; pass it back when saving.
    pub etag: String,
    pub metadata: Option<crate::document::Frontmatter>,
    pub linked_target: Option<String>,
    pub modified_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentWriteRequest {
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub expected_etag: Option<String>,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentWriteResult {
    pub path: String,
    pub etag: String,
    pub created: bool,
    pub linked_target: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentDeleteResult {
    pub path: String,
    pub trashed_path: String,
}

impl DocumentService {
    pub fn new(
        root: impl AsRef<Path>,
        caller_id: impl Into<String>,
        project_key: Option<String>,
        max_backups: usize,
    ) -> Result<Self> {
        let root = fs::canonicalize(root.as_ref())
            .with_context(|| format!("Vault root does not exist: {}", root.as_ref().display()))?;
        if !root.is_dir() {
            bail!("Vault root is not a directory: {}", root.display());
        }
        Ok(Self {
            root,
            caller_id: caller_id.into(),
            project_key,
            max_backups,
        })
    }

    /// List canonical memory documents. Wiki-owned files are excluded because
    /// generated Wiki output must never appear as canonical memory input.
    pub fn list(&self) -> Result<Vec<DocumentEntry>> {
        let mut documents = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root)
            .follow_links(false)
            .into_iter()
            .filter_entry(|entry| {
                !is_internal_directory(entry.path())
                    && !crate::path_policy::is_vault_wiki_path(&self.root, entry.path())
            })
            .filter_map(std::result::Result::ok)
        {
            if !entry.file_type().is_file() || !is_markdown(entry.path()) || is_backup(entry.path())
            {
                continue;
            }
            // Extra defence: even if the walker missed it, drop wiki-relative
            // paths before they leave the service.
            let relative = self.relative_string(entry.path())?;
            if crate::path_policy::is_vault_wiki_relative_path(&relative) {
                continue;
            }
            let metadata = entry.metadata().ok();
            documents.push(DocumentEntry {
                path: relative,
                modified_at: metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok())
                    .map(format_time),
                size_bytes: metadata.map_or(0, |metadata| metadata.len()),
            });
        }
        documents.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(documents)
    }

    pub fn read(&self, path: &str) -> Result<DocumentRead> {
        reject_wiki(path)?;
        self.read_any(path)
    }

    /// Wiki-only read. Returns the raw wiki page bytes; skips audit metadata
    /// (there is none to inject). Rejects non-wiki paths.
    pub fn read_wiki(&self, path: &str) -> Result<DocumentRead> {
        require_wiki(path)?;
        self.read_any(path)
    }

    fn read_any(&self, path: &str) -> Result<DocumentRead> {
        let requested = self.resolve_existing(path)?;
        let (editable, linked_target) = self.resolve_edit_target(&requested)?;
        let content = fs::read_to_string(&editable)
            .with_context(|| format!("Failed to read {}", editable.display()))?;
        let metadata = crate::document::Document::parse(&requested)
            .ok()
            .and_then(|document| document.frontmatter);
        let modified_at = fs::metadata(&editable)
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .map(format_time);
        Ok(DocumentRead {
            path: self.relative_string(&requested)?,
            etag: etag(&content),
            content,
            metadata,
            linked_target,
            modified_at,
        })
    }

    pub fn create(&self, request: DocumentWriteRequest) -> Result<DocumentWriteResult> {
        reject_wiki(&request.path)?;
        self.create_internal(request, /* inject_audit = */ true)
    }

    /// Create a new Wiki page. Skips audit metadata injection to keep managed
    /// regions and provenance stamps intact.
    pub fn create_wiki(&self, request: DocumentWriteRequest) -> Result<DocumentWriteResult> {
        require_wiki(&request.path)?;
        self.create_internal(request, /* inject_audit = */ false)
    }

    fn create_internal(
        &self,
        request: DocumentWriteRequest,
        inject_audit: bool,
    ) -> Result<DocumentWriteResult> {
        let target = self.resolve_new(&request.path)?;
        if target.exists() {
            bail!("Document already exists: {}", request.path);
        }
        self.replace_at(target, request, true, None, inject_audit)
    }

    pub fn write(&self, request: DocumentWriteRequest) -> Result<DocumentWriteResult> {
        reject_wiki(&request.path)?;
        self.write_internal(request, /* inject_audit = */ true)
    }

    /// Wiki-safe overwrite. Skips audit metadata injection and writes exactly
    /// the bytes provided; still enforces vault containment, `.md` policy,
    /// symlink-safe ancestor canonicalisation, and the ETag conflict guard.
    /// Rejects non-wiki paths.
    pub fn write_wiki(&self, request: DocumentWriteRequest) -> Result<DocumentWriteResult> {
        require_wiki(&request.path)?;
        self.write_internal(request, /* inject_audit = */ false)
    }

    fn write_internal(
        &self,
        request: DocumentWriteRequest,
        inject_audit: bool,
    ) -> Result<DocumentWriteResult> {
        let requested = self.resolve_existing(&request.path)?;
        let (target, linked_target) = self.resolve_edit_target(&requested)?;
        self.replace_at(target, request, false, linked_target, inject_audit)
    }

    /// Renames a document without silently replacing an existing destination.
    /// A linked document is moved as the link document; its source is untouched.
    pub fn rename(&self, from: &str, to: &str) -> Result<DocumentEntry> {
        reject_wiki(from)?;
        reject_wiki(to)?;
        let source = self.resolve_existing(from)?;
        let destination = self.resolve_new(to)?;
        if destination.exists() {
            bail!("Destination already exists: {to}");
        }
        let parent = destination
            .parent()
            .context("Document path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::rename(&source, &destination)
            .with_context(|| format!("Failed to rename {} to {}", from, to))?;
        let metadata = fs::metadata(&destination)?;
        Ok(DocumentEntry {
            path: self.relative_string(&destination)?,
            modified_at: metadata.modified().ok().map(format_time),
            size_bytes: metadata.len(),
        })
    }

    /// Moves a document to an internal vault trash directory. It never performs
    /// an irreversible delete, and does not recursively delete directories.
    pub fn delete(&self, path: &str) -> Result<DocumentDeleteResult> {
        reject_wiki(path)?;
        self.delete_internal(path)
    }

    /// Wiki-only variant of [`Self::delete`]. Used by AI apply rollback when a
    /// newly created wiki page needs to be undone.
    pub fn delete_wiki(&self, path: &str) -> Result<DocumentDeleteResult> {
        require_wiki(path)?;
        self.delete_internal(path)
    }

    fn delete_internal(&self, path: &str) -> Result<DocumentDeleteResult> {
        let source = self.resolve_existing(path)?;
        let relative = source
            .strip_prefix(&self.root)
            .expect("validated root prefix");
        let bucket = Utc::now().format("%Y%m%dT%H%M%S%.9fZ").to_string();
        let destination = self
            .root
            .join(".rms-memory")
            .join("trash")
            .join(bucket)
            .join(relative);
        let parent = destination
            .parent()
            .context("Trash document path has no parent")?;
        fs::create_dir_all(parent)?;
        fs::rename(&source, &destination)
            .with_context(|| format!("Failed to move {} to trash", source.display()))?;
        Ok(DocumentDeleteResult {
            path: relative.to_string_lossy().replace('\\', "/"),
            trashed_path: self.relative_string(&destination)?,
        })
    }

    fn replace_at(
        &self,
        target: PathBuf,
        request: DocumentWriteRequest,
        created: bool,
        linked_target: Option<String>,
        inject_audit: bool,
    ) -> Result<DocumentWriteResult> {
        if !created {
            let current = fs::read_to_string(&target)
                .with_context(|| format!("Failed to read {} before writing", target.display()))?;
            if let Some(expected) = request.expected_etag.as_deref()
                && expected != etag(&current)
            {
                bail!(
                    "Document conflict: '{}' was changed by another writer",
                    request.path
                );
            }
            if inject_audit {
                self.create_backup(&target)?;
            }
        }

        let content = if inject_audit {
            let mut metadata_args = Map::new();
            if let Some(confidence) = request.confidence {
                metadata_args.insert("confidence".to_owned(), Value::from(confidence));
            }
            if let Some(source) = request.source.as_ref() {
                metadata_args.insert("source".to_owned(), Value::from(source.clone()));
            }
            crate::tools::write::inject_audit_metadata(
                &request.content,
                &self.caller_id,
                self.project_key.as_deref(),
                &metadata_args,
            )?
        } else {
            request.content.clone()
        };
        atomic_replace(&target, content.as_bytes())?;
        Ok(DocumentWriteResult {
            path: request.path,
            etag: etag(&content),
            created,
            linked_target,
        })
    }

    fn create_backup(&self, target: &Path) -> Result<()> {
        if self.max_backups == 0 {
            return Ok(());
        }
        let parent = target.parent().context("Document path has no parent")?;
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .context("Invalid document name")?;
        let prefix = format!("{name}.bak.");
        let mut backups = fs::read_dir(parent)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().starts_with(&prefix))
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        backups.sort_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok()
        });
        while backups.len() >= self.max_backups {
            if let Some(oldest) = backups.first() {
                fs::remove_file(oldest)?;
            }
            backups.remove(0);
        }
        let backup = parent.join(format!(
            "{name}.bak.{}",
            Utc::now().format("%Y%m%d%H%M%S%.9f")
        ));
        fs::copy(target, backup)?;
        Ok(())
    }

    fn resolve_existing(&self, path: &str) -> Result<PathBuf> {
        let candidate = self.resolve_new(path)?;
        if !candidate.exists() {
            bail!("Document does not exist: {path}");
        }
        let canonical = fs::canonicalize(&candidate)?;
        self.ensure_inside_root(&canonical)?;
        if !canonical.is_file() {
            bail!("Document is not a regular file: {path}");
        }
        Ok(canonical)
    }

    fn resolve_new(&self, path: &str) -> Result<PathBuf> {
        validate_document_path(path)?;
        let candidate = self.root.join(path);
        // Canonicalise the nearest existing ancestor. This catches a symlink in
        // an otherwise-new path before `create_dir_all` can follow it outside.
        let mut ancestor = candidate.as_path();
        while !ancestor.exists() {
            ancestor = ancestor.parent().context("Document path has no parent")?;
        }
        let canonical_ancestor = fs::canonicalize(ancestor)?;
        self.ensure_inside_root(&canonical_ancestor)?;
        Ok(candidate)
    }

    fn resolve_edit_target(&self, requested: &Path) -> Result<(PathBuf, Option<String>)> {
        // Vault-aware link resolution: if the requested document is a link,
        // the returned target is guaranteed to be inside the vault (both by
        // parsing and by canonicalisation-plus-symlink check).
        let target =
            crate::link::resolve_link_in_vault(requested, &self.root).with_context(|| {
                format!(
                    "Linked document target does not exist or escapes vault for {}",
                    requested.display()
                )
            })?;
        // `resolve_link_in_vault` returns a canonicalised path when it followed
        // a link, or the original path (which is already canonical because the
        // caller went through `resolve_existing`) when there was no link.
        self.ensure_inside_root(&target)?;
        if !target.is_file() || !is_markdown(&target) {
            bail!(
                "Linked target is not a vault Markdown document: {}",
                target.display()
            );
        }
        let linked_target = if target != requested {
            Some(self.relative_string(&target)?)
        } else {
            None
        };
        Ok((target, linked_target))
    }

    fn ensure_inside_root(&self, path: &Path) -> Result<()> {
        if !path.starts_with(&self.root) {
            bail!("Path escapes vault boundary: {}", path.display());
        }
        Ok(())
    }

    fn relative_string(&self, path: &Path) -> Result<String> {
        let relative = path
            .strip_prefix(&self.root)
            .context("Path is outside vault")?;
        Ok(relative.to_string_lossy().replace('\\', "/"))
    }
}

fn validate_document_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        bail!("Document path must not be empty");
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() || !is_markdown(parsed) {
        bail!("Document path must be a relative Markdown (.md) path");
    }
    if parsed
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("Document path must not contain '.' or '..' components");
    }
    Ok(())
}

fn reject_wiki(path: &str) -> Result<()> {
    if crate::path_policy::is_vault_wiki_relative_path(path) {
        bail!(
            "Path '{path}' is inside the generated Wiki namespace. Canonical memory tools cannot touch Wiki output; use the *_wiki DocumentService methods instead."
        );
    }
    Ok(())
}

fn require_wiki(path: &str) -> Result<()> {
    if !crate::path_policy::is_vault_wiki_relative_path(path) {
        bail!(
            "Path '{path}' is not inside the Wiki namespace. Wiki-safe methods only accept wiki/ paths."
        );
    }
    Ok(())
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn is_backup(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains(".bak."))
}

fn is_internal_directory(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| matches!(name, ".git" | ".rms-memory" | ".lancedb"))
}

fn etag(content: &str) -> String {
    blake3::hash(content.as_bytes()).to_hex().to_string()
}

fn format_time(time: std::time::SystemTime) -> String {
    DateTime::<Utc>::from(time).to_rfc3339()
}

fn atomic_replace(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().context("Document path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(content)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn service(root: &Path) -> DocumentService {
        DocumentService::new(root, "gui", Some("project-a".to_string()), 2).unwrap()
    }

    #[test]
    fn create_write_and_backup_keep_metadata_and_detect_conflicts() {
        let directory = tempdir().unwrap();
        let service = service(directory.path());
        let created = service
            .create(DocumentWriteRequest {
                path: "notes/a.md".to_string(),
                content: "# First".to_string(),
                expected_etag: None,
                confidence: Some(0.8),
                source: Some("gui".to_string()),
            })
            .unwrap();
        assert!(created.created);
        let opened = service.read("notes/a.md").unwrap();
        assert!(opened.content.contains("project: project-a"));
        let written = service
            .write(DocumentWriteRequest {
                path: "notes/a.md".to_string(),
                content: "# Second".to_string(),
                expected_etag: Some(opened.etag.clone()),
                confidence: None,
                source: None,
            })
            .unwrap();
        assert_ne!(opened.etag, written.etag);
        assert_eq!(
            fs::read_dir(directory.path().join("notes"))
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().contains(".bak."))
                .count(),
            1
        );
        assert!(
            service
                .write(DocumentWriteRequest {
                    path: "notes/a.md".to_string(),
                    content: "# Lost update".to_string(),
                    expected_etag: Some(opened.etag),
                    confidence: None,
                    source: None,
                })
                .unwrap_err()
                .to_string()
                .contains("conflict")
        );
    }

    #[test]
    fn rejects_traversal_and_symlink_escapes() {
        let directory = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let service = service(directory.path());
        assert!(
            service
                .create(DocumentWriteRequest {
                    path: "../escape.md".into(),
                    content: String::new(),
                    expected_etag: None,
                    confidence: None,
                    source: None
                })
                .is_err()
        );
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(outside.path(), directory.path().join("escape")).unwrap();
            assert!(
                service
                    .create(DocumentWriteRequest {
                        path: "escape/nope.md".into(),
                        content: String::new(),
                        expected_etag: None,
                        confidence: None,
                        source: None
                    })
                    .is_err()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn create_with_directory_symlink_escape_is_blocked() {
        // Regression: if a symlink appears in the middle of an otherwise-new
        // path (e.g. `parent/notes/a.md` where `parent` is a directory symlink
        // pointing outside), ancestor canonicalisation must catch it before
        // create_dir_all can follow the link outside the vault.
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), vault.path().join("escape")).unwrap();
        let service = service(vault.path());
        let error = service
            .create(DocumentWriteRequest {
                path: "escape/new/notes/a.md".into(),
                content: "x".into(),
                expected_etag: None,
                confidence: None,
                source: None,
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("escapes vault"), "got: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_edit_target_rejects_link_that_escapes_via_symlink() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let outside_source = outside.path().join("secret.md");
        fs::write(&outside_source, "top secret").unwrap();

        // A symlink inside the vault that points at the outside file.
        std::os::unix::fs::symlink(&outside_source, vault.path().join("escape.md")).unwrap();
        // A vault document whose frontmatter links to the symlink.
        fs::write(
            vault.path().join("doc.md"),
            "---\nlink: escape.md\n---\ncontent",
        )
        .unwrap();

        let service = service(vault.path());
        let error = service.read("doc.md").unwrap_err().to_string();
        assert!(
            error.contains("escapes vault") || error.contains("does not exist"),
            "got: {error}"
        );

        // Writing through the same escaping link must also fail.
        let error = service
            .write(DocumentWriteRequest {
                path: "doc.md".into(),
                content: "clobber".into(),
                expected_etag: None,
                confidence: None,
                source: None,
            })
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("escapes vault") || error.contains("does not exist"),
            "got: {error}"
        );

        // The outside file must remain untouched.
        assert_eq!(fs::read_to_string(&outside_source).unwrap(), "top secret");
    }

    #[test]
    fn delete_is_recoverable_and_list_hides_internal_files() {
        let directory = tempdir().unwrap();
        let service = service(directory.path());
        service
            .create(DocumentWriteRequest {
                path: "a.md".into(),
                content: "A".into(),
                expected_etag: None,
                confidence: None,
                source: None,
            })
            .unwrap();
        let deleted = service.delete("a.md").unwrap();
        assert!(!directory.path().join("a.md").exists());
        assert!(directory.path().join(&deleted.trashed_path).exists());
        assert!(service.list().unwrap().is_empty());
    }

    #[test]
    fn canonical_methods_reject_wiki_paths() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("wiki")).unwrap();
        fs::write(directory.path().join("wiki/page.md"), "hi").unwrap();
        let service = service(directory.path());

        for error in [
            service.read("wiki/page.md").unwrap_err().to_string(),
            service
                .write(DocumentWriteRequest {
                    path: "wiki/page.md".into(),
                    content: "no".into(),
                    expected_etag: None,
                    confidence: None,
                    source: None,
                })
                .unwrap_err()
                .to_string(),
            service
                .create(DocumentWriteRequest {
                    path: "wiki/new.md".into(),
                    content: "no".into(),
                    expected_etag: None,
                    confidence: None,
                    source: None,
                })
                .unwrap_err()
                .to_string(),
            service
                .rename("wiki/page.md", "wiki/other.md")
                .unwrap_err()
                .to_string(),
            service.delete("wiki/page.md").unwrap_err().to_string(),
        ] {
            assert!(
                error.contains("Wiki"),
                "expected wiki rejection, got: {error}"
            );
        }
    }

    #[test]
    fn list_excludes_wiki_and_hidden_directories() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("wiki/sub")).unwrap();
        fs::create_dir_all(directory.path().join(".rms-memory")).unwrap();
        fs::write(directory.path().join("wiki/index.md"), "wiki root").unwrap();
        fs::write(directory.path().join("wiki/sub/page.md"), "wiki child").unwrap();
        fs::write(
            directory.path().join(".rms-memory/notes.md"),
            "internal note",
        )
        .unwrap();
        fs::write(directory.path().join("notes.md"), "canonical").unwrap();
        let service = service(directory.path());

        let listed = service.list().unwrap();
        let paths: Vec<_> = listed.iter().map(|entry| entry.path.as_str()).collect();
        assert_eq!(paths, vec!["notes.md"]);
    }

    #[test]
    fn wiki_write_preserves_managed_regions_without_audit_metadata() {
        let directory = tempdir().unwrap();
        fs::create_dir_all(directory.path().join("wiki")).unwrap();
        let service = service(directory.path());

        let body = "<!-- RMS-WIKI:BEGIN managed -->\ncontent\n<!-- RMS-WIKI:END managed -->\n";
        service
            .create_wiki(DocumentWriteRequest {
                path: "wiki/index.md".into(),
                content: body.into(),
                expected_etag: None,
                confidence: None,
                source: None,
            })
            .unwrap();

        let on_disk = fs::read_to_string(directory.path().join("wiki/index.md")).unwrap();
        assert_eq!(on_disk, body, "audit metadata must not be injected");

        let read = service.read_wiki("wiki/index.md").unwrap();
        assert_eq!(read.content, body);
        assert!(
            !read.content.contains("last_modified_by"),
            "audit must not be injected"
        );

        let updated = "<!-- RMS-WIKI:BEGIN managed -->\nnew\n<!-- RMS-WIKI:END managed -->\n";
        service
            .write_wiki(DocumentWriteRequest {
                path: "wiki/index.md".into(),
                content: updated.into(),
                expected_etag: Some(read.etag),
                confidence: None,
                source: None,
            })
            .unwrap();
        let after = fs::read_to_string(directory.path().join("wiki/index.md")).unwrap();
        assert_eq!(after, updated);
    }

    #[test]
    fn wiki_methods_reject_non_wiki_paths() {
        let directory = tempdir().unwrap();
        let service = service(directory.path());

        assert!(
            service
                .create_wiki(DocumentWriteRequest {
                    path: "notes/a.md".into(),
                    content: "x".into(),
                    expected_etag: None,
                    confidence: None,
                    source: None,
                })
                .unwrap_err()
                .to_string()
                .contains("Wiki")
        );
        assert!(
            service
                .write_wiki(DocumentWriteRequest {
                    path: "notes/a.md".into(),
                    content: "x".into(),
                    expected_etag: None,
                    confidence: None,
                    source: None,
                })
                .unwrap_err()
                .to_string()
                .contains("Wiki")
        );
        assert!(
            service
                .read_wiki("notes/a.md")
                .unwrap_err()
                .to_string()
                .contains("Wiki")
        );
    }
}
