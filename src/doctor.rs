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

/// Run the seven doctor checks and return a structured report.
pub async fn diagnose(workspace: &Workspace, options: DiagnoseOptions) -> Result<DoctorReport> {
    let mut checks = Vec::with_capacity(7);
    let mut issue_count = 0u32;

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
        for dir in required {
            if !workspace.root.join(dir).exists() {
                issues.push(format!("{dir}/ missing"));
            }
        }
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 1,
            name: "vault_directory_structure".into(),
            ok: issues.is_empty(),
            issues,
            notes: vec![],
        });
    }

    let files = workspace.find_markdown_files().unwrap_or_default();

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
        let mut issues = missing_ids;
        issues.extend(invalid_frontmatter);
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 2,
            name: "document_ids".into(),
            ok: issues.is_empty(),
            issues,
            notes: repaired
                .into_iter()
                .map(|p| format!("repaired frontmatter ids: {p}"))
                .collect(),
        });
    }

    // 3. Cross-document links
    {
        let file_set: std::collections::HashSet<_> = files
            .iter()
            .filter_map(|f| {
                f.strip_prefix(&workspace.root)
                    .ok()
                    .map(|r| r.to_string_lossy().replace('\\', "/"))
            })
            .collect();
        let mut broken = Vec::new();
        for f in &files {
            if let Ok(doc) = rms_memory_core::document::Document::parse(f) {
                for link in doc.extract_links() {
                    let target = workspace.root.join(&link);
                    if !target.exists() && !file_set.contains(&link.replace('\\', "/")) {
                        broken.push(format!("{} → {link}", rel(workspace, f)));
                    }
                }
            }
        }
        issue_count += broken.len() as u32;
        checks.push(DoctorCheck {
            id: 3,
            name: "cross_document_links".into(),
            ok: broken.is_empty(),
            issues: broken,
            notes: vec![],
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
        issues.extend(stale_learned);
        issues.extend(broken_supersession);
        issue_count += issues.len() as u32;
        checks.push(DoctorCheck {
            id: 7,
            name: "knowledge_freshness".into(),
            ok: issues.is_empty(),
            issues,
            notes: vec![],
        });
    }

    Ok(DoctorReport {
        project_key: workspace.project_key(),
        vault_root: workspace.root.display().to_string(),
        issue_count,
        healthy: issue_count == 0,
        checks,
    })
}

fn rel(workspace: &Workspace, path: &Path) -> String {
    path.strip_prefix(&workspace.root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
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
