//! Shared read-only vault health diagnostics (CLI + MCP).

use anyhow::Result;
use chrono::{DateTime, Utc};
use rms_memory_core::workspace::Workspace;
use serde::Serialize;
use std::collections::HashMap;
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
    let read_only = workspace.ensure_writable().is_err();
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
        let mut notes = Vec::new();
        match rms_memory_index::store::Store::for_workspace(workspace).await {
            Ok(store) => {
                match rms_memory_index::index_lock::inspect(&store.storage_path) {
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
            }
            Err(e) => issues.push(format!("cannot verify wiki isolation: {e}")),
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 5,
            name: "wiki_index_isolation".into(),
            ok: issues.is_empty(),
            issues,
            notes: vec![],
        });
    }

    // 6. Registry coherence
    {
        let mut issues = Vec::new();
        let mut notes = Vec::new();
        if let Ok(registry) = rms_memory_core::config_manager::load_registry() {
            let vault_canon =
                std::fs::canonicalize(&workspace.root).unwrap_or_else(|_| workspace.root.clone());
            let vault_str = vault_canon.to_string_lossy().to_string();
            let mut found = false;
            for proj in registry.projects.values() {
                if let Ok(p) = std::fs::canonicalize(&proj.vault_path)
                    && p.to_string_lossy() == vault_str
                {
                    found = true;
                    notes.push("project registered in registry.toml".into());
                    break;
                }
            }
            if !found {
                for proj in registry.projects.values() {
                    if let Ok(p) = std::fs::canonicalize(&proj.code_path)
                        && p == std::fs::canonicalize(&workspace.code_path)
                            .unwrap_or_else(|_| workspace.code_path.clone())
                    {
                        found = true;
                        notes.push("project matched by code_path".into());
                        break;
                    }
                }
            }
            if !found {
                issues.push("workspace not found in registry.toml".into());
            }
        } else {
            issues.push("cannot load registry.toml".into());
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
        for f in &files {
            if let Ok(doc) = rms_memory_core::document::Document::parse(f) {
                let Some(fm) = &doc.frontmatter else {
                    continue;
                };
                let Some(target) = fm.supersedes.as_deref() else {
                    continue;
                };
                if !id_to_path.contains_key(target) && !target.starts_with("path:") {
                    broken_supersession.push(format!(
                        "{} (supersedes={target} not found among vault ids)",
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
        checks.push(DoctorCheck {
            id: 8,
            name: "credentials".into(),
            ok: issues.is_empty(),
            issues,
            notes: vec![],
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
pub fn missing_id_notes(missing: &[String]) -> Vec<String> {
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
pub fn unresolved_wikilink_notes(links: &[BrokenLink]) -> Vec<String> {
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
pub struct BrokenLink {
    pub source: String,
    /// Target as written.
    pub target: String,
    /// `[[wikilink]]` / `![[embed]]`. Obsidian treats an unresolved wikilink as
    /// a note that does not exist yet, so these are informational.
    pub wiki: bool,
}

pub fn broken_links(workspace: &Workspace, files: &[std::path::PathBuf]) -> Vec<BrokenLink> {
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
pub fn stale_checkpoint_notes(workspace: &Workspace) -> Vec<String> {
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
    use super::parse_fm_datetime;

    #[test]
    fn parses_rfc3339_and_date_only() {
        assert!(parse_fm_datetime("2020-01-01T00:00:00Z").is_some());
        assert!(parse_fm_datetime("2020-01-01").is_some());
        assert!(parse_fm_datetime("not-a-date").is_none());
    }
}
