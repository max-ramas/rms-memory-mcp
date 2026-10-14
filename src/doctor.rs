//! Shared read-only vault health diagnostics (CLI + MCP).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rms_memory_core::workspace::Workspace;
use serde::Serialize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub project_key: Option<String>,
    pub vault_root: String,
    pub issue_count: u32,
    pub checks: Vec<DoctorCheck>,
    pub healthy: bool,
    /// Update / restart advice; informational, not counted as issues.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notices: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorCheck {
    pub id: u8,
    pub name: String,
    pub ok: bool,
    pub issues: Vec<String>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnoseOptions {
    pub repair_frontmatter: bool,
}

/// Run the eight doctor checks and return a structured report.
pub async fn diagnose(workspace: &Workspace, options: DiagnoseOptions) -> Result<DoctorReport> {
    let mut checks = Vec::with_capacity(8);
    let mut issue_count = 0u32;
    let read_only = workspace.is_read_only().unwrap_or(false);
    if options.repair_frontmatter {
        workspace.ensure_writable()?;
    }
    let (files, discovery_error) = match workspace.find_markdown_files() {
        Ok(files) => (files, None),
        Err(error) => (Vec::new(), Some(error)),
    };

    // 1. Vault directory structure
    {
        let required = [
            "rules",
            "decisions",
            "architecture",
            "artifacts",
            "docs",
            "api",
        ];
        let mut issues = Vec::new();
        let mut notes = Vec::new();
        for dir in required {
            if !workspace.root.join(dir).exists() {
                if read_only {
                    notes.push(format!("{dir}/ absent (read-only vault, not created)"));
                } else {
                    issues.push(format!("{dir}/ missing"));
                }
            }
        }
        if let Some(error) = &discovery_error {
            issues.push(format!(
                "markdown discovery failed, document checks skipped: {error:#}"
            ));
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 1,
            name: "vault_directory_structure".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 2. Document IDs
    {
        let mut missing_ids = Vec::new();
        let mut invalid_frontmatter = Vec::new();
        let mut repaired = Vec::new();
        for f in &files {
            match rms_memory_core::document::Document::parse(f) {
                Ok(doc) => {
                    if doc
                        .frontmatter
                        .as_ref()
                        .and_then(|fm| fm.id.as_ref())
                        .is_none()
                    {
                        if options.repair_frontmatter {
                            match rms_memory_core::document::Document::repair_duplicate_ids(f) {
                                Ok(true) => {
                                    repaired.push(rel(workspace, f));
                                    continue;
                                }
                                Ok(false) => missing_ids.push(rel(workspace, f)),
                                Err(e) => invalid_frontmatter
                                    .push(format!("{}: repair failed: {e:#}", rel(workspace, f))),
                            }
                        } else {
                            missing_ids.push(rel(workspace, f));
                        }
                    }
                }
                Err(error) => {
                    if options.repair_frontmatter {
                        match rms_memory_core::document::Document::repair_duplicate_ids(f) {
                            Ok(true) => {
                                repaired.push(rel(workspace, f));
                                continue;
                            }
                            Ok(false) => invalid_frontmatter
                                .push(format!("{}: {error:#}", rel(workspace, f))),
                            Err(repair_error) => invalid_frontmatter.push(format!(
                                "{}: {error:#} (repair failed: {repair_error:#})",
                                rel(workspace, f)
                            )),
                        }
                    } else {
                        invalid_frontmatter.push(format!("{}: {error:#}", rel(workspace, f)));
                    }
                }
            }
        }
        let issues = invalid_frontmatter;
        issue_count += issues.len() as u32;
        let mut notes: Vec<String> = repaired
            .into_iter()
            .map(|p| format!("repaired frontmatter ids: {p}"))
            .collect();
        if !issues.is_empty() {
            notes.push(if read_only {
                "notes with invalid frontmatter are indexed as plain text".into()
            } else {
                "notes with invalid frontmatter are indexed as plain text; `rms-memory doctor --repair-frontmatter` fixes duplicate id keys".into()
            });
        }
        notes.extend(missing_id_notes(&missing_ids));
        checks.push(DoctorCheck {
            id: 2,
            name: "document_ids".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 3. Cross-document links
    {
        let (wiki, markdown): (Vec<_>, Vec<_>) = broken_links(workspace, &files)
            .into_iter()
            .partition(|link| link.wiki);
        let broken: Vec<String> = markdown
            .iter()
            .map(|link| format!("{} → {}", link.source, link.target))
            .collect();
        issue_count += broken.len() as u32;
        checks.push(DoctorCheck {
            id: 3,
            name: "cross_document_links".into(),
            ok: broken.is_empty(),
            issues: broken,
            notes: unresolved_wikilink_notes(&wiki),
        });
    }

    // 4. LanceDB store
    {
        let mut issues = Vec::new();
        let in_use = rms_memory_index::embedding::active();
        let mut notes = vec![format!(
            "embedding model: {} ({} dimensions)",
            in_use.id, in_use.dimension
        )];
        match rms_memory_index::embedding::configured() {
            Ok(configured) if configured.id != in_use.id => notes.push(format!(
                "configured embedding model is {}; restart this process to use it",
                configured.id
            )),
            Ok(_) => {}
            Err(error) => issues.push(format!(
                "configured embedding model is invalid, using {}: {error:#}",
                in_use.id
            )),
        }
        match rms_memory_index::store::Store::for_workspace(workspace).await {
            Ok(store) => {
                let inspection = rms_memory_index::index_lock::inspect(&store.storage_path);
                let writer_active = matches!(
                    inspection,
                    Ok(rms_memory_index::index_lock::LockInspection::Active(_))
                );
                match inspection {
                    Ok(rms_memory_index::index_lock::LockInspection::Active(Some(owner))) => {
                        notes.push(format!(
                            "index writer active: PID {} (since {})",
                            owner.pid, owner.acquired_at
                        ));
                    }
                    Ok(rms_memory_index::index_lock::LockInspection::Active(None)) => {
                        notes.push("index writer active (owner metadata unavailable)".into());
                    }
                    Ok(rms_memory_index::index_lock::LockInspection::StaleMetadataCleared(
                        owner,
                    )) => {
                        notes.push(format!(
                            "cleared stale lock metadata for PID {} (recorded {})",
                            owner.pid, owner.acquired_at
                        ));
                    }
                    Ok(rms_memory_index::index_lock::LockInspection::Unlocked) => {
                        notes.push("index writer lock is free".into());
                    }
                    Err(e) => issues.push(format!("cannot inspect index writer lock: {e}")),
                }
                if let Err(e) = store.open_table().await {
                    issues.push(format!("LanceDB table not accessible: {e}"));
                } else {
                    match rms_memory_index::indexer::vault_freshness(workspace, &store).await {
                        Ok(freshness) => notes.push(freshness.describe()),
                        Err(e) => notes.push(format!("index freshness unavailable: {e:#}")),
                    }
                    match fts_coverage_finding(&store, writer_active).await {
                        Some((true, message)) => issues.push(message),
                        Some((false, message)) => notes.push(message),
                        None => {}
                    }
                }
            }
            Err(e) => issues.push(format!("cannot connect to LanceDB: {e}")),
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 4,
            name: "lancedb_store".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 5. Wiki index isolation
    {
        let mut issues = Vec::new();
        let mut notes = Vec::new();
        match rms_memory_index::store::Store::for_workspace(workspace).await {
            Ok(store) => {
                let leaked_vault = match store.open_table().await {
                    Ok(table) => store
                        .get_file_timestamps(&table)
                        .await
                        .map(|paths| {
                            paths
                                .into_keys()
                                .filter(|path| {
                                    rms_memory_core::path_policy::is_vault_wiki_relative_path(path)
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                let leaked_code = store
                    .indexed_code_file_paths()
                    .await
                    .map(|paths| {
                        paths
                            .into_iter()
                            .filter(|path| {
                                rms_memory_core::path_policy::is_vault_wiki_path(
                                    &workspace.root,
                                    &workspace.code_path.join(path),
                                )
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                for path in leaked_vault {
                    issues.push(format!("wiki path in vault index: {path}"));
                }
                for path in leaked_code {
                    issues.push(format!("wiki path in code index: {path}"));
                }
                if !issues.is_empty() {
                    notes.push("a normal sync purges these records; `rms-memory reindex --code` rebuilds code memory now".into());
                }
            }
            Err(e) => issues.push(format!("cannot verify wiki isolation: {e}")),
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 5,
            name: "wiki_index_isolation".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 6. Registry coherence
    {
        let mut issues = Vec::new();
        let mut notes = Vec::new();
        match rms_memory_core::config_manager::load_registry() {
            Ok(registry) => {
                use rms_memory_core::workspace::canonical_or_given;
                let vault = canonical_or_given(&workspace.root);
                let code = canonical_or_given(&workspace.code_path);
                if registry
                    .projects
                    .values()
                    .any(|project| canonical_or_given(Path::new(&project.vault_path)) == vault)
                {
                    notes.push("project registered in registry.toml".into());
                } else if registry
                    .projects
                    .values()
                    .any(|project| canonical_or_given(Path::new(&project.code_path)) == code)
                {
                    notes.push("project matched by code_path".into());
                } else {
                    issues.push("workspace not found in registry.toml; run `rms-memory projects migrate` if the repository was moved or renamed".into());
                }
                let basename = workspace
                    .code_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or_default();
                if let Some(key) = workspace.project_key()
                    && !read_only
                    && !rms_memory_core::workspace::Registry::keys_equivalent(basename, &key)
                {
                    notes.push(format!(
                        "folder name `{basename}` differs from project key `{key}`; `rms-memory projects migrate` realigns them (never recreate .git)"
                    ));
                }
            }
            Err(error) => issues.push(format!("cannot load registry.toml: {error:#}")),
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 6,
            name: "registry_coherence".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 7. Knowledge freshness
    {
        let now = Utc::now();
        let mut expired = Vec::new();
        let mut stale_learned = Vec::new();
        let mut broken_supersession = Vec::new();
        let mut id_to_path: HashMap<String, String> = HashMap::new();
        for f in &files {
            if let Ok(doc) = rms_memory_core::document::Document::parse(f) {
                let relative = rel(workspace, f);
                if let Some(fm) = &doc.frontmatter {
                    if let Some(id) = &fm.id {
                        id_to_path.insert(id.clone(), relative.clone());
                    }
                    if let Some(until) = fm.valid_until.as_deref()
                        && let Some(ts) = parse_fm_datetime(until)
                        && ts < now
                    {
                        expired.push(format!("{relative} (valid_until={until})"));
                    }
                    if fm.valid_until.is_none()
                        && let Some(learned) = fm.learned_at.as_deref()
                        && let Some(ts) = parse_fm_datetime(learned)
                    {
                        let age_days = (now - ts).num_days();
                        if age_days > 365 {
                            stale_learned.push(format!(
                                "{relative} (learned_at={learned}, {age_days}d old)"
                            ));
                        }
                    }
                    if fm
                        .status
                        .as_deref()
                        .is_some_and(|s| s.eq_ignore_ascii_case("superseded"))
                        && fm.superseded_by.is_none()
                    {
                        broken_supersession.push(format!(
                            "{relative} (status=superseded without superseded_by)"
                        ));
                    }
                }
            }
        }
        let vault_paths = supersession_paths(files.iter().map(|f| rel(workspace, f)));
        for f in &files {
            if let Ok(doc) = rms_memory_core::document::Document::parse(f) {
                let Some(fm) = &doc.frontmatter else {
                    continue;
                };
                let Some(target) = fm.supersedes.as_deref() else {
                    continue;
                };
                if !supersession_target_exists(
                    target,
                    |id| id_to_path.contains_key(id),
                    |path| vault_paths.contains(path),
                ) {
                    broken_supersession.push(format!(
                        "{} (supersedes={target} matches no vault note id or path)",
                        rel(workspace, f)
                    ));
                }
            }
        }
        let mut issues = expired;
        issues.extend(broken_supersession);
        issue_count += issues.len() as u32;
        let mut notes: Vec<String> = stale_learned
            .into_iter()
            .map(|line| format!("learned_at older than 365d without valid_until: {line}"))
            .collect();
        notes.extend(stale_checkpoint_notes(workspace));
        checks.push(DoctorCheck {
            id: 7,
            name: "knowledge_freshness".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    // 8. Credentials stored in notes (report only, never rewritten)
    {
        let issues: Vec<String> = secrets_in_vault(workspace, &files)
            .into_iter()
            .map(|(path, found)| format!("{path}: {found}"))
            .collect();
        issue_count += issues.len() as u32;
        let notes = if issues.is_empty() {
            Vec::new()
        } else {
            vec!["values are not shown and nothing is changed; revoke the keys and edit the notes (git history keeps old values)".into()]
        };
        checks.push(DoctorCheck {
            id: 8,
            name: "credentials".into(),
            ok: issues.is_empty(),
            issues,
            notes,
        });
    }

    let notices = tokio::task::spawn_blocking(crate::update_check::doctor_notices)
        .await
        .unwrap_or_default();

    Ok(DoctorReport {
        project_key: workspace.project_key(),
        vault_root: workspace.root.display().to_string(),
        issue_count,
        healthy: issue_count == 0,
        checks,
        notices,
    })
}

/// `(note, "kind (line n), ...")` for notes containing credentials with a
/// known provider prefix. Values are never included.
pub fn secrets_in_vault(
    workspace: &Workspace,
    files: &[std::path::PathBuf],
) -> Vec<(String, String)> {
    files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).ok()?;
            let findings = rms_memory_core::secrets::scan(&text);
            (!findings.is_empty()).then(|| {
                (
                    rel(workspace, f),
                    rms_memory_core::secrets::describe(&findings),
                )
            })
        })
        .collect()
}

/// Whether keyword search covers every vault row. Syncs by 1.3.0 always fold
/// new rows in; uncovered rows mean a running or interrupted sync, or a sync
/// or reindex by a release before 1.3.0 (which built the index on an empty
/// table). Returns `(is_issue, message)`.
async fn fts_coverage_finding(
    store: &rms_memory_index::store::Store,
    writer_active: bool,
) -> Option<(bool, String)> {
    let table = store.open_table().await.ok()?;
    let rows = table.count_rows(None).await.ok()?;
    match store.fts_coverage(&table).await {
        Ok(Some((_, 0))) => None,
        Ok(Some((indexed, unindexed))) if writer_active => Some((
            false,
            format!(
                "full-text index covers {indexed} of {} rows; the running sync adds the rest",
                indexed + unindexed
            ),
        )),
        Ok(Some((indexed, unindexed))) => Some((
            true,
            format!(
                "full-text index covers {indexed} of {} rows, so keyword search misses relevance order; run `rms-memory sync` (an older release or an interrupted sync left it behind)",
                indexed + unindexed
            ),
        )),
        Ok(None) if rows > 0 && !writer_active => Some((
            true,
            "vault table has no full-text index; run `rms-memory sync` to build it".to_string(),
        )),
        Ok(None) => None,
        Err(error) => Some((
            false,
            format!("full-text index stats unavailable: {error:#}"),
        )),
    }
}

/// Vault-relative note paths a `supersedes` path may name, including the
/// original paths of notes archived under `artifacts/pruned/<date>/`.
fn supersession_paths(relative_paths: impl IntoIterator<Item = String>) -> HashSet<String> {
    let mut known = HashSet::new();
    for path in relative_paths {
        if let Some(original) = path
            .strip_prefix("artifacts/pruned/")
            .and_then(|rest| rest.split_once('/'))
            .map(|(_, original)| original.to_string())
        {
            known.insert(original);
        }
        known.insert(path);
    }
    known
}

/// `rms_write` stores the predecessor's id; hand-written notes often name
/// its vault-relative path instead (optionally as `path:<path>`).
fn supersession_target_exists(
    target: &str,
    id_exists: impl Fn(&str) -> bool,
    path_exists: impl Fn(&str) -> bool,
) -> bool {
    let path = target.strip_prefix("path:").unwrap_or(target);
    id_exists(target) || path_exists(path.trim_start_matches("./"))
}

fn rel(workspace: &Workspace, path: &Path) -> String {
    path.strip_prefix(&workspace.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn capped_notes(header: String, items: impl ExactSizeIterator<Item = String>) -> Vec<String> {
    const SHOWN: usize = 20;
    let total = items.len();
    if total == 0 {
        return Vec::new();
    }
    let mut notes = vec![header];
    notes.extend(items.take(SHOWN));
    if total > SHOWN {
        notes.push(format!("… and {} more", total - SHOWN));
    }
    notes
}

/// Notes without an `id` are valid (indexed by path), so they are reported
/// as information rather than issues.
fn missing_id_notes(missing: &[String]) -> Vec<String> {
    capped_notes(
        format!(
            "{} note(s) have no frontmatter `id` and are indexed by path; an `id` only keeps links stable across renames",
            missing.len()
        ),
        missing.iter().map(|p| format!("no id: {p}")),
    )
}

/// Unresolved `[[wikilinks]]` are placeholders for notes not written yet in
/// Obsidian, so they are information rather than issues.
fn unresolved_wikilink_notes(links: &[BrokenLink]) -> Vec<String> {
    capped_notes(
        format!(
            "{} [[wikilink(s)]] point to notes that do not exist yet",
            links.len()
        ),
        links
            .iter()
            .map(|link| format!("{} → [[{}]]", link.source, link.target)),
    )
}

/// A link that resolves to neither a vault note nor an existing file.
#[derive(Debug, Clone)]
struct BrokenLink {
    source: String,
    /// Target as written.
    target: String,
    /// `[[wikilink]]` / `![[embed]]`. Obsidian treats an unresolved wikilink as
    /// a note that does not exist yet, so these are informational.
    wiki: bool,
}

fn broken_links(workspace: &Workspace, files: &[std::path::PathBuf]) -> Vec<BrokenLink> {
    use rms_memory_core::vault_links::{LinkResolution, LinkSyntax, VaultLinkIndex};
    let documents: Vec<_> = files
        .iter()
        .filter_map(|f| {
            rms_memory_core::document::Document::parse(f)
                .ok()
                .map(|doc| (rel(workspace, f), doc))
        })
        .collect();
    let mut resolver = VaultLinkIndex::from_files(&workspace.root, files);
    for (path, doc) in &documents {
        if let Some(frontmatter) = &doc.frontmatter {
            resolver.add_aliases(path, &frontmatter.all_aliases());
        }
    }
    let mut broken = Vec::new();
    for (path, doc) in &documents {
        for link in doc.vault_links() {
            if let LinkResolution::Missing(_) = resolver.resolve(path, &link) {
                broken.push(BrokenLink {
                    source: path.clone(),
                    wiki: link.syntax == LinkSyntax::Wiki,
                    target: link.target,
                });
            }
        }
    }
    broken
}

/// Active checkpoints nobody touched for a while; details via `rms_hygiene`.
fn stale_checkpoint_notes(workspace: &Workspace) -> Vec<String> {
    crate::tools::hygiene::stale_checkpoints(&workspace.root, Utc::now())
        .unwrap_or_default()
        .into_iter()
        .map(|note| {
            format!(
                "active checkpoint untouched for {}d: {} (close with rms_checkpoint_done or refresh it)",
                note.age_days, note.path
            )
        })
        .collect()
}

pub fn parse_fm_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    DateTime::parse_from_rfc3339(raw)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
                .ok()
                .and_then(|d| d.and_hms_opt(0, 0, 0))
                .map(|ndt| DateTime::<Utc>::from_naive_utc_and_offset(ndt, Utc))
        })
}

#[cfg(test)]
mod tests {
    use super::{parse_fm_datetime, supersession_paths, supersession_target_exists};

    #[test]
    fn supersedes_accepts_an_id_or_an_existing_vault_path() {
        let ids = ["old-note-1"];
        let paths = ["decisions/old.md"];
        let check = |target: &str| {
            supersession_target_exists(target, |id| ids.contains(&id), |p| paths.contains(&p))
        };
        assert!(check("old-note-1"));
        assert!(check("decisions/old.md"));
        assert!(check("path:decisions/old.md"));
        assert!(check("./decisions/old.md"));
        assert!(!check("decisions/missing.md"));
        assert!(!check("missing-id"));
    }

    #[test]
    fn supersedes_may_name_a_note_that_was_archived_by_prune() {
        let known = supersession_paths([
            "decisions/new.md".to_string(),
            "artifacts/pruned/2026-10-14/decisions/old.md".to_string(),
            "artifacts/pruned/manifest-only.md".to_string(),
        ]);
        assert!(known.contains("decisions/new.md"));
        assert!(known.contains("decisions/old.md"));
        assert!(known.contains("artifacts/pruned/2026-10-14/decisions/old.md"));
        assert!(!known.contains("manifest-only.md"));
    }

    #[test]
    fn parses_rfc3339_and_date_only() {
        assert!(parse_fm_datetime("2020-01-01T00:00:00Z").is_some());
        assert!(parse_fm_datetime("2020-01-01").is_some());
        assert!(parse_fm_datetime("not-a-date").is_none());
    }
}
