//! Archive superseded vault notes that are past a retention window.
//!
//! This is the first, non-agentic slice of "pruning": only notes already marked
//! `status: superseded` (and not `pinned`) are candidates. Cold-note heuristics
//! and autonomous consolidation remain future work.
//!
//! Default mode is dry-run. `--apply` moves files under
//! `artifacts/pruned/YYYY-MM-DD/<original-relative-path>` and appends a
//! JSONL manifest entry. Never deletes.

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;
use std::path::Path;

const PRUNE_ROOT: &str = "artifacts/pruned";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PruneOptions {
    /// Minimum age of a superseded note before it is eligible.
    pub older_than_days: u32,
    /// When false, only report candidates.
    pub apply: bool,
}

impl Default for PruneOptions {
    fn default() -> Self {
        Self {
            older_than_days: 30,
            apply: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PruneCandidate {
    pub path: String,
    pub age_days: u32,
    pub age_source: String,
    pub document_id: Option<String>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PruneAction {
    pub path: String,
    pub archived_to: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PruneReport {
    pub dry_run: bool,
    pub older_than_days: u32,
    pub candidates: Vec<PruneCandidate>,
    pub archived: Vec<PruneAction>,
    pub batch_dir: Option<String>,
}

#[derive(Debug, Serialize)]
struct ManifestEntry<'a> {
    archived_at: &'a str,
    path: &'a str,
    archived_to: &'a str,
    document_id: Option<&'a str>,
    superseded_by: Option<&'a str>,
    age_days: u32,
    age_source: &'a str,
}

/// Scan the vault and optionally archive eligible superseded notes.
pub fn prune_vault(root: &Path, options: PruneOptions) -> Result<PruneReport> {
    let root = fs::canonicalize(root)
        .with_context(|| format!("Vault root does not exist: {}", root.display()))?;
    if !root.is_dir() {
        bail!("Vault root is not a directory: {}", root.display());
    }
    if options.older_than_days == 0 {
        bail!("--older-than-days must be >= 1");
    }

    let cutoff = Utc::now() - Duration::days(i64::from(options.older_than_days));
    let mut candidates = Vec::new();

    for entry in walkdir::WalkDir::new(&root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !should_skip_dir(e.path(), &root))
        .filter_map(std::result::Result::ok)
    {
        if !entry.file_type().is_file() || !is_markdown(entry.path()) || is_backup(entry.path()) {
            continue;
        }
        let relative = relative_string(&root, entry.path())?;
        if rms_memory_core::path_policy::is_vault_wiki_relative_path(&relative)
            || is_pruned_relative(&relative)
            || relative.starts_with("trash/")
            || relative == "trash"
        {
            continue;
        }

        let Ok(doc) = rms_memory_core::document::Document::parse(entry.path()) else {
            continue;
        };
        let Some(fm) = doc.frontmatter.as_ref() else {
            continue;
        };
        if !fm
            .status
            .as_deref()
            .is_some_and(|s| s.eq_ignore_ascii_case("superseded"))
        {
            continue;
        }
        if fm.pinned == Some(true) {
            continue;
        }

        let Some((age_at, age_source)) = resolve_age(fm, entry.path()) else {
            continue;
        };
        if age_at > cutoff {
            continue;
        }
        let age_days = (Utc::now() - age_at).num_days().max(0) as u32;
        candidates.push(PruneCandidate {
            path: relative,
            age_days,
            age_source: age_source.to_string(),
            document_id: fm.id.clone(),
            superseded_by: fm.superseded_by.clone(),
        });
    }

    candidates.sort_by(|a, b| a.path.cmp(&b.path));

    let mut report = PruneReport {
        dry_run: !options.apply,
        older_than_days: options.older_than_days,
        candidates: candidates.clone(),
        archived: Vec::new(),
        batch_dir: None,
    };

    if !options.apply || candidates.is_empty() {
        return Ok(report);
    }

    let batch = Utc::now().format("%Y-%m-%d").to_string();
    let batch_rel = format!("{PRUNE_ROOT}/{batch}");
    let batch_abs = root.join(&batch_rel);
    fs::create_dir_all(&batch_abs)
        .with_context(|| format!("Failed to create prune batch dir {}", batch_abs.display()))?;
    report.batch_dir = Some(batch_rel.clone());

    let manifest_path = batch_abs.join("manifest.jsonl");
    let mut manifest = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&manifest_path)
        .with_context(|| format!("Failed to open {}", manifest_path.display()))?;

    let archived_at = Utc::now().to_rfc3339();
    for candidate in &candidates {
        let source = root.join(&candidate.path);
        let dest_rel = format!("{batch_rel}/{}", candidate.path);
        let dest = root.join(&dest_rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("Failed to create {}", parent.display()))?;
        }
        if dest.exists() {
            bail!(
                "Refuse to overwrite existing archive path {}",
                dest.display()
            );
        }
        fs::rename(&source, &dest)
            .with_context(|| format!("Failed to move {} → {}", source.display(), dest.display()))?;

        let entry = ManifestEntry {
            archived_at: &archived_at,
            path: &candidate.path,
            archived_to: &dest_rel,
            document_id: candidate.document_id.as_deref(),
            superseded_by: candidate.superseded_by.as_deref(),
            age_days: candidate.age_days,
            age_source: &candidate.age_source,
        };
        writeln!(manifest, "{}", serde_json::to_string(&entry)?)?;
        report.archived.push(PruneAction {
            path: candidate.path.clone(),
            archived_to: dest_rel,
        });
    }

    Ok(report)
}

fn resolve_age(
    fm: &rms_memory_core::document::Frontmatter,
    path: &Path,
) -> Option<(DateTime<Utc>, &'static str)> {
    for (raw, label) in [
        (fm.timestamp.as_deref(), "timestamp"),
        (fm.created_at.as_deref(), "created_at"),
        (fm.learned_at.as_deref(), "learned_at"),
        (fm.valid_until.as_deref(), "valid_until"),
    ] {
        if let Some(raw) = raw
            && let Ok(dt) = DateTime::parse_from_rfc3339(raw)
        {
            return Some((dt.with_timezone(&Utc), label));
        }
    }
    let modified = fs::metadata(path).ok()?.modified().ok()?;
    Some((DateTime::<Utc>::from(modified), "mtime"))
}

fn relative_string(root: &Path, path: &Path) -> Result<String> {
    let relative = pathdiff::diff_paths(path, root)
        .with_context(|| format!("{} is outside vault {}", path.display(), root.display()))?;
    Ok(relative.to_string_lossy().replace('\\', "/"))
}

fn should_skip_dir(path: &Path, root: &Path) -> bool {
    if path == root {
        return false;
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    matches!(name, ".git" | ".rms-memory" | ".lancedb" | "trash" | "wiki") || {
        // Skip nested prune archives so we do not re-scan archived notes.
        path.strip_prefix(root)
            .ok()
            .is_some_and(|rel| is_pruned_relative(&rel.to_string_lossy().replace('\\', "/")))
    }
}

fn is_pruned_relative(relative: &str) -> bool {
    relative == PRUNE_ROOT || relative.starts_with(&format!("{PRUNE_ROOT}/"))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn write_note(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    #[test]
    fn dry_run_lists_old_superseded_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_note(
            root,
            "decisions/old.md",
            "---\nid: old\nstatus: superseded\ntimestamp: '2020-01-01T00:00:00Z'\n---\n\n# Old\n",
        );
        write_note(
            root,
            "decisions/fresh.md",
            &format!(
                "---\nid: fresh\nstatus: superseded\ntimestamp: '{}'\n---\n\n# Fresh\n",
                Utc::now().to_rfc3339()
            ),
        );
        write_note(
            root,
            "decisions/active.md",
            "---\nid: active\nstatus: active\ntimestamp: '2020-01-01T00:00:00Z'\n---\n\n# Active\n",
        );
        write_note(
            root,
            "decisions/pinned.md",
            "---\nid: pinned\nstatus: superseded\npinned: true\ntimestamp: '2020-01-01T00:00:00Z'\n---\n\n# Pinned\n",
        );

        let report = prune_vault(
            root,
            PruneOptions {
                older_than_days: 30,
                apply: false,
            },
        )
        .unwrap();
        assert!(report.dry_run);
        assert_eq!(report.candidates.len(), 1);
        assert_eq!(report.candidates[0].path, "decisions/old.md");
        assert!(report.archived.is_empty());
        assert!(root.join("decisions/old.md").exists());
    }

    #[test]
    fn apply_moves_to_artifacts_pruned_with_manifest() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_note(
            root,
            "rules/stale.md",
            "---\nid: stale\nstatus: superseded\nsuperseded_by: newer\ntimestamp: '2019-06-01T00:00:00Z'\n---\n\n# Stale\n",
        );

        let report = prune_vault(
            root,
            PruneOptions {
                older_than_days: 7,
                apply: true,
            },
        )
        .unwrap();
        assert!(!report.dry_run);
        assert_eq!(report.archived.len(), 1);
        let archived_to = &report.archived[0].archived_to;
        assert!(archived_to.starts_with("artifacts/pruned/"));
        assert!(archived_to.ends_with("/rules/stale.md"));
        assert!(!root.join("rules/stale.md").exists());
        assert!(root.join(archived_to).exists());

        let batch = report.batch_dir.as_ref().unwrap();
        let manifest = fs::read_to_string(root.join(batch).join("manifest.jsonl")).unwrap();
        assert!(manifest.contains("\"path\":\"rules/stale.md\""));
        assert!(manifest.contains("superseded_by\":\"newer\""));
    }

    #[test]
    fn does_not_rescan_already_pruned_tree() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write_note(
            root,
            "artifacts/pruned/2020-01-01/old.md",
            "---\nid: archived\nstatus: superseded\ntimestamp: '2018-01-01T00:00:00Z'\n---\n\n# Already archived\n",
        );
        let report = prune_vault(root, PruneOptions::default()).unwrap();
        assert!(report.candidates.is_empty());
    }
}
