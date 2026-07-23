use crate::config_manager::ConfigManager;
use crate::document::{Document, Frontmatter};
use crate::index_lock::{self, LockInspection};
use crate::workspace::Registry;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LinkRepairEntry {
    pub vault_relative: String,
    pub old_link: String,
    pub new_link: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMigratePlan {
    pub old_key: String,
    pub new_key: String,
    pub rename_key: bool,
    pub old_code_path: String,
    pub new_code_path: String,
    pub old_vault_path: String,
    pub new_vault_path: String,
    pub move_vault: bool,
    pub move_db: bool,
    pub old_db_path: Option<String>,
    pub new_db_path: Option<String>,
    pub link_repairs: Vec<LinkRepairEntry>,
    pub project_stamp_updates: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMigrateOutcome {
    pub old_key: String,
    pub new_key: String,
    pub rename_key: bool,
    pub code_path: String,
    pub vault_path: String,
    pub links_repaired: usize,
    pub project_stamps_updated: usize,
    pub vault_moved: bool,
    pub db_moved: bool,
    pub redirect_created: bool,
    pub warnings: Vec<String>,
    pub restart_required: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectMigrateOptions {
    pub new_key: Option<String>,
    pub dry_run: bool,
    pub repair_links: bool,
    pub update_project_stamps: bool,
    pub strict_git: bool,
}

pub fn keys_equivalent(left: &str, right: &str) -> bool {
    Registry::keys_equivalent(left, right)
}

pub fn suggest_project_key(old_key: &str, new_code_path: &Path) -> String {
    let basename = new_code_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(old_key);
    if keys_equivalent(basename, old_key) {
        old_key.to_string()
    } else {
        basename.to_string()
    }
}

pub fn paths_equivalent(left: &Path, right: &Path) -> bool {
    let left_canon = canonical_or_original(left);
    let right_canon = canonical_or_original(right);
    if left_canon == right_canon {
        return true;
    }
    left_canon
        .to_string_lossy()
        .eq_ignore_ascii_case(&right_canon.to_string_lossy())
}

pub fn resolve_project_key<'a>(registry: &'a Registry, key: &str) -> Option<&'a str> {
    registry.resolve_project_key(key)
}

pub fn migration_redirect_message(registry: &Registry, requested: &str) -> Option<String> {
    registry.migration_redirect_message(requested)
}

pub fn coalesce_migrations(registry: &mut Registry, old_key: &str, new_key: &str) {
    registry.coalesce_migrations(old_key, new_key);
}

pub fn build_plan(
    registry: &Registry,
    old_key: &str,
    new_code_dir: &Path,
    options: &ProjectMigrateOptions,
) -> Result<ProjectMigratePlan> {
    let config = registry
        .projects
        .get(old_key)
        .cloned()
        .ok_or_else(|| anyhow!("Project '{old_key}' is not registered"))?;

    if !new_code_dir.is_dir() {
        bail!(
            "Target code path is not a directory: {}",
            new_code_dir.display()
        );
    }

    let old_code_path = PathBuf::from(&config.code_path);
    let new_code_path = canonical_or_original(new_code_dir);
    let new_key = options
        .new_key
        .clone()
        .unwrap_or_else(|| suggest_project_key(old_key, &new_code_path));
    if new_key.trim().is_empty() {
        bail!("Project key must be non-empty");
    }

    let rename_key = !keys_equivalent(old_key, &new_key);
    let mut warnings = Vec::new();

    for (key, project) in &registry.projects {
        if key == old_key {
            continue;
        }
        if rename_key && keys_equivalent(key, &new_key) {
            bail!("Project key '{new_key}' is already registered as '{key}'");
        }
        if paths_equivalent(Path::new(&project.code_path), &new_code_path) {
            bail!(
                "Code path '{}' is already registered under project '{key}'",
                new_code_path.display()
            );
        }
    }

    compare_git_remotes(
        &old_code_path,
        &new_code_path,
        options.strict_git,
        &mut warnings,
    )?;

    let master_vault = registry
        .global
        .global_vault_path
        .as_deref()
        .ok_or_else(|| anyhow!("Master vault path is not configured"))?;
    let old_vault_path = PathBuf::from(&config.vault_path);
    let new_vault_path = Path::new(master_vault).join(&new_key);
    let move_vault = rename_key && old_vault_path != new_vault_path;

    let old_db_path = index_path_for_vault(&old_vault_path);
    let new_db_path = index_path_for_vault(&new_vault_path);
    let move_db = move_vault && old_db_path.exists();

    let mut link_repairs = Vec::new();
    if options.repair_links {
        link_repairs = plan_link_repairs(
            &old_vault_path,
            &old_code_path,
            &new_code_path,
            &mut warnings,
        )?;
    }

    let mut project_stamp_updates = Vec::new();
    if rename_key && options.update_project_stamps {
        project_stamp_updates =
            plan_project_stamp_updates(&old_vault_path, old_key, &new_key, &mut warnings)?;
    }

    Ok(ProjectMigratePlan {
        old_key: old_key.to_string(),
        new_key,
        rename_key,
        old_code_path: old_code_path.to_string_lossy().to_string(),
        new_code_path: new_code_path.to_string_lossy().to_string(),
        old_vault_path: old_vault_path.to_string_lossy().to_string(),
        new_vault_path: new_vault_path.to_string_lossy().to_string(),
        move_vault,
        move_db,
        old_db_path: Some(old_db_path.to_string_lossy().to_string()),
        new_db_path: Some(new_db_path.to_string_lossy().to_string()),
        link_repairs,
        project_stamp_updates,
        warnings,
    })
}

pub fn migrate(
    manager: &ConfigManager,
    old_key: &str,
    new_code_dir: &Path,
    options: ProjectMigrateOptions,
) -> Result<ProjectMigrateOutcome> {
    let snapshot = manager.snapshot();
    let plan = build_plan(&snapshot.registry, old_key, new_code_dir, &options)?;

    if options.dry_run {
        return Ok(outcome_from_plan(&plan));
    }

    if plan.move_db {
        let storage = plan
            .old_db_path
            .as_deref()
            .context("Missing old database path")?;
        match index_lock::inspect(storage)? {
            LockInspection::Unlocked | LockInspection::StaleMetadataCleared(_) => {}
            LockInspection::Active(Some(owner)) => {
                bail!(
                    "Index at '{storage}' is locked by pid {} (acquired {}). Close MCP/GUI and retry migrate.",
                    owner.pid,
                    owner.acquired_at
                );
            }
            LockInspection::Active(None) => {
                bail!(
                    "Index at '{storage}' is locked by another process. Close MCP/GUI and retry migrate."
                );
            }
        }
    }

    let validated_links = validate_link_repairs(&plan)?;
    apply_link_repairs(&validated_links)?;
    if plan.rename_key && !plan.project_stamp_updates.is_empty() {
        let vault_root = PathBuf::from(&plan.old_vault_path);
        for relative in &plan.project_stamp_updates {
            let path = vault_root.join(relative);
            update_frontmatter(&path, |frontmatter| {
                frontmatter.project = Some(plan.new_key.clone());
            })?;
        }
    }

    let mut rollback: Vec<(PathBuf, PathBuf)> = Vec::new();
    if plan.move_vault {
        let old_vault = PathBuf::from(&plan.old_vault_path);
        let new_vault = PathBuf::from(&plan.new_vault_path);
        if new_vault.exists() {
            bail!(
                "Refusing to overwrite existing vault directory '{}'",
                new_vault.display()
            );
        }
        std::fs::rename(&old_vault, &new_vault).with_context(|| {
            format!(
                "Failed to move vault '{}' -> '{}'",
                old_vault.display(),
                new_vault.display()
            )
        })?;
        rollback.push((new_vault.clone(), old_vault.clone()));

        if plan.move_db {
            let old_db = PathBuf::from(plan.old_db_path.as_ref().unwrap());
            let new_db = PathBuf::from(plan.new_db_path.as_ref().unwrap());
            if old_db.exists() {
                if new_db.exists() {
                    bail!(
                        "Refusing to overwrite existing index directory '{}'",
                        new_db.display()
                    );
                }
                std::fs::rename(&old_db, &new_db).with_context(|| {
                    format!(
                        "Failed to move index '{}' -> '{}'",
                        old_db.display(),
                        new_db.display()
                    )
                })?;
                rollback.push((new_db.clone(), old_db.clone()));
            }
        }
    }

    let mut registry = snapshot.registry.clone();
    let mut updated = registry
        .projects
        .get(old_key)
        .cloned()
        .ok_or_else(|| anyhow!("Project '{old_key}' is not registered"))?;
    updated.code_path = plan.new_code_path.clone();
    updated.vault_path = plan.new_vault_path.clone();

    registry.projects.remove(old_key);
    if registry.projects.contains_key(&plan.new_key) && !keys_equivalent(old_key, &plan.new_key) {
        rollback_filesystem(&rollback)?;
        bail!("Project key '{}' is already registered", plan.new_key);
    }
    registry.projects.insert(plan.new_key.clone(), updated);

    let redirect_created = plan.rename_key;
    if redirect_created {
        registry.coalesce_migrations(old_key, &plan.new_key);
    }

    if let Err(error) = manager.replace(snapshot.revision, registry) {
        rollback_filesystem(&rollback)?;
        return Err(error);
    }

    Ok(ProjectMigrateOutcome {
        old_key: plan.old_key,
        new_key: plan.new_key,
        rename_key: plan.rename_key,
        code_path: plan.new_code_path,
        vault_path: plan.new_vault_path,
        links_repaired: plan.link_repairs.len(),
        project_stamps_updated: plan.project_stamp_updates.len(),
        vault_moved: plan.move_vault,
        db_moved: plan.move_db,
        redirect_created,
        warnings: plan.warnings,
        restart_required: true,
    })
}

fn outcome_from_plan(plan: &ProjectMigratePlan) -> ProjectMigrateOutcome {
    ProjectMigrateOutcome {
        old_key: plan.old_key.clone(),
        new_key: plan.new_key.clone(),
        rename_key: plan.rename_key,
        code_path: plan.new_code_path.clone(),
        vault_path: plan.new_vault_path.clone(),
        links_repaired: plan.link_repairs.len(),
        project_stamps_updated: plan.project_stamp_updates.len(),
        vault_moved: plan.move_vault,
        db_moved: plan.move_db,
        redirect_created: plan.rename_key,
        warnings: plan.warnings.clone(),
        restart_required: plan.rename_key,
    }
}

fn rollback_filesystem(moves: &[(PathBuf, PathBuf)]) -> Result<()> {
    let mut errors = Vec::new();
    for (from, to) in moves.iter().rev() {
        if from.exists()
            && !to.exists()
            && let Err(error) = std::fs::rename(from, to)
        {
            errors.push(format!(
                "Failed to rollback '{}' -> '{}': {error}",
                from.display(),
                to.display()
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(errors.join("; ")))
    }
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn index_path_for_vault(vault: &Path) -> PathBuf {
    let canon = canonical_or_original(vault);
    let hash = blake3::hash(canon.to_string_lossy().as_bytes())
        .to_hex()
        .to_string();
    crate::workspace::base_dir().join("dbs").join(hash)
}

fn git_remote_url(dir: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C", &dir.to_string_lossy(), "remote", "get-url", "origin"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}

fn compare_git_remotes(
    old_code: &Path,
    new_code: &Path,
    strict: bool,
    warnings: &mut Vec<String>,
) -> Result<()> {
    let old_remote = git_remote_url(old_code);
    let new_remote = git_remote_url(new_code);
    match (old_remote, new_remote) {
        (Some(old_url), Some(new_url)) if old_url != new_url => {
            let message =
                format!("Git remote mismatch: old origin '{old_url}' vs new origin '{new_url}'");
            if strict {
                bail!(message);
            }
            warnings.push(message);
        }
        (Some(_), None) | (None, Some(_)) => {
            warnings
                .push("Git remote could not be verified on one side of the migration".to_string());
        }
        _ => {}
    }
    Ok(())
}

fn iter_vault_markdown(vault_root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !vault_root.exists() {
        return Ok(files);
    }
    for entry in walkdir::WalkDir::new(vault_root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
    {
        let path = entry.into_path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let relative = path
            .strip_prefix(vault_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        if crate::path_policy::is_vault_wiki_relative_path(&relative) {
            continue;
        }
        files.push(path);
    }
    files.sort();
    Ok(files)
}

fn plan_link_repairs(
    vault_root: &Path,
    old_code_path: &Path,
    new_code_path: &Path,
    warnings: &mut Vec<String>,
) -> Result<Vec<LinkRepairEntry>> {
    let old_code = canonical_or_original(old_code_path);
    let new_code = canonical_or_original(new_code_path);
    let mut repairs = Vec::new();

    for file_path in iter_vault_markdown(vault_root)? {
        let doc = match Document::parse(&file_path) {
            Ok(doc) => doc,
            Err(error) => {
                warnings.push(format!(
                    "Skipping unreadable vault document {}: {error:#}",
                    file_path.display()
                ));
                continue;
            }
        };
        let Some(link) = doc.frontmatter.as_ref().and_then(|fm| fm.link.clone()) else {
            continue;
        };
        let Some(parent) = file_path.parent() else {
            continue;
        };
        let resolved = parent.join(&link);
        let resolved = match std::fs::canonicalize(&resolved) {
            Ok(path) => path,
            Err(_) => {
                warnings.push(format!(
                    "Skipping link repair for {}: target '{}' is missing",
                    file_path.display(),
                    resolved.display()
                ));
                continue;
            }
        };
        let Ok(repo_relative) = resolved.strip_prefix(&old_code) else {
            warnings.push(format!(
                "Skipping link repair for {}: target is outside old code path",
                file_path.display()
            ));
            continue;
        };
        let new_target = new_code.join(repo_relative);
        let new_link = pathdiff::diff_paths(&new_target, parent).unwrap_or(new_target);
        let vault_relative = file_path
            .strip_prefix(vault_root)
            .unwrap_or(&file_path)
            .to_string_lossy()
            .replace('\\', "/");
        repairs.push(LinkRepairEntry {
            vault_relative,
            old_link: link,
            new_link: new_link.to_string_lossy().replace('\\', "/"),
        });
    }
    Ok(repairs)
}

fn validate_link_repairs(plan: &ProjectMigratePlan) -> Result<Vec<(PathBuf, String)>> {
    let vault_root = PathBuf::from(&plan.old_vault_path);
    let mut writes = Vec::new();
    for repair in &plan.link_repairs {
        let path = vault_root.join(&repair.vault_relative);
        if !path.is_file() {
            bail!("Link repair target missing: {}", path.display());
        }
        writes.push((path, repair.new_link.clone()));
    }
    Ok(writes)
}

fn apply_link_repairs(writes: &[(PathBuf, String)]) -> Result<()> {
    for (path, new_link) in writes {
        update_frontmatter(path, |frontmatter| {
            frontmatter.link = Some(new_link.clone());
        })?;
    }
    Ok(())
}

fn plan_project_stamp_updates(
    vault_root: &Path,
    old_key: &str,
    new_key: &str,
    warnings: &mut Vec<String>,
) -> Result<Vec<String>> {
    let mut updates = Vec::new();
    for file_path in iter_vault_markdown(vault_root)? {
        let doc = match Document::parse(&file_path) {
            Ok(doc) => doc,
            Err(error) => {
                warnings.push(format!(
                    "Skipping project stamp scan for {}: {error:#}",
                    file_path.display()
                ));
                continue;
            }
        };
        let needs_update = doc
            .frontmatter
            .as_ref()
            .and_then(|fm| fm.project.as_deref())
            .is_some_and(|project| keys_equivalent(project, old_key))
            || doc
                .frontmatter
                .as_ref()
                .map(|fm| fm.project.is_none())
                .unwrap_or(true);
        if needs_update {
            updates.push(
                file_path
                    .strip_prefix(vault_root)
                    .unwrap_or(&file_path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    if updates.is_empty() {
        return Ok(updates);
    }
    let _ = new_key;
    Ok(updates)
}

fn update_frontmatter(path: &Path, update: impl FnOnce(&mut Frontmatter)) -> Result<()> {
    let doc = Document::parse(path)?;
    let mut frontmatter = doc.frontmatter.unwrap_or(Frontmatter {
        memory_version: None,
        id: None,
        alias: None,
        doc_type: None,
        status: None,
        link: None,
        last_modified_by: None,
        timestamp: None,
        created_at: None,
        confidence: None,
        source: None,
        project: None,
    });
    update(&mut frontmatter);
    let yaml = serde_yaml::to_string(&frontmatter).context("Failed to serialize frontmatter")?;
    let rebuilt = format!("---\n{yaml}---\n{}", doc.content);
    std::fs::write(path, rebuilt)
        .with_context(|| format!("Failed to update frontmatter in {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{GlobalConfig, MigrationRedirect, ProjectConfig, Registry};
    use std::collections::HashMap;

    #[test]
    fn keys_equivalent_is_case_insensitive() {
        assert!(keys_equivalent("Foo", "foo"));
        assert!(!keys_equivalent("Foo", "bar"));
    }

    #[test]
    fn suggest_project_key_preserves_old_key_on_case_only_basename_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("foo");
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(suggest_project_key("Foo", &path), "Foo");
    }

    #[test]
    fn suggest_project_key_uses_basename_when_name_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bar");
        std::fs::create_dir_all(&path).unwrap();
        assert_eq!(suggest_project_key("Foo", &path), "bar");
    }

    #[test]
    fn coalesce_migrations_rewrites_chain() {
        let mut registry = Registry {
            migrations: HashMap::from([(
                "A".to_string(),
                MigrationRedirect {
                    to: "B".to_string(),
                    migrated_at: "t1".to_string(),
                },
            )]),
            ..Registry::default()
        };
        coalesce_migrations(&mut registry, "B", "C");
        assert_eq!(registry.migrations.get("A").unwrap().to, "C");
        assert_eq!(registry.migrations.get("B").unwrap().to, "C");
    }

    #[test]
    fn resolve_project_key_follows_redirect() {
        let registry = Registry {
            projects: HashMap::from([(
                "C".to_string(),
                ProjectConfig {
                    code_path: "/code".into(),
                    vault_path: "/vault".into(),
                    include: vec![],
                    exclude: vec![],
                    code_index_mode: Default::default(),
                    code_languages: vec![],
                },
            )]),
            migrations: HashMap::from([(
                "A".to_string(),
                MigrationRedirect {
                    to: "C".to_string(),
                    migrated_at: "t".into(),
                },
            )]),
            ..Registry::default()
        };
        assert_eq!(resolve_project_key(&registry, "A"), Some("C"));
    }

    #[test]
    fn plan_uses_relocate_only_for_case_only_key_choice() {
        let dir = tempfile::tempdir().unwrap();
        let master = dir.path().join("vaults");
        let vault = master.join("Foo");
        let old_code = dir.path().join("old");
        let new_code = dir.path().join("new").join("foo");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&new_code).unwrap();
        let registry = Registry {
            global: GlobalConfig {
                global_vault_path: Some(master.to_string_lossy().to_string()),
                ..GlobalConfig::default()
            },
            projects: HashMap::from([(
                "Foo".to_string(),
                ProjectConfig {
                    code_path: old_code.to_string_lossy().to_string(),
                    vault_path: vault.to_string_lossy().to_string(),
                    include: vec![],
                    exclude: vec![],
                    code_index_mode: Default::default(),
                    code_languages: vec![],
                },
            )]),
            ..Registry::default()
        };
        let plan = build_plan(
            &registry,
            "Foo",
            &new_code,
            &ProjectMigrateOptions {
                new_key: None,
                dry_run: true,
                repair_links: false,
                update_project_stamps: false,
                strict_git: false,
            },
        )
        .unwrap();
        assert!(!plan.rename_key);
        assert!(!plan.move_vault);
        assert_eq!(plan.new_key, "Foo");
    }
}
