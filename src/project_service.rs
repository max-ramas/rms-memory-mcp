use crate::config_manager::ConfigManager;
use crate::workspace::{ProjectConfig, Registry, base_dir};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectRemovalMode {
    Unregister,
    DeleteData,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRemovalOutcome {
    pub project: String,
    pub vault_path: String,
    pub index_path: Option<String>,
    pub registry_removed: bool,
    pub vault_deleted: bool,
    pub index_deleted: bool,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct ProjectService {
    manager: ConfigManager,
    data_root: PathBuf,
}

impl ProjectService {
    pub fn open() -> Result<Self> {
        Ok(Self::new(ConfigManager::open()?, base_dir()))
    }

    pub fn new(manager: ConfigManager, data_root: PathBuf) -> Self {
        Self { manager, data_root }
    }

    pub fn unregister(&self, project: &str) -> Result<ProjectRemovalOutcome> {
        self.remove(project, ProjectRemovalMode::Unregister)
    }

    pub fn delete_data(&self, project: &str, confirmation: &str) -> Result<ProjectRemovalOutcome> {
        if confirmation != project {
            bail!("Confirmation must exactly match project key '{project}'");
        }
        self.remove(project, ProjectRemovalMode::DeleteData)
    }

    fn remove(&self, project: &str, mode: ProjectRemovalMode) -> Result<ProjectRemovalOutcome> {
        if project.trim().is_empty() {
            bail!("Project key must be non-empty");
        }
        let snapshot = self.manager.snapshot();
        let config = snapshot
            .registry
            .projects
            .get(project)
            .cloned()
            .ok_or_else(|| anyhow!("Project '{project}' is not registered"))?;

        let deletion = if mode == ProjectRemovalMode::DeleteData {
            Some(self.validate_deletion(&snapshot.registry, &config)?)
        } else {
            None
        };

        let mut registry = snapshot.registry;
        registry.projects.remove(project);
        self.manager.replace(snapshot.revision, registry)?;

        let mut outcome = ProjectRemovalOutcome {
            project: project.to_string(),
            vault_path: config.vault_path,
            index_path: deletion
                .as_ref()
                .map(|paths| paths.index.to_string_lossy().to_string()),
            registry_removed: true,
            vault_deleted: false,
            index_deleted: false,
            warnings: Vec::new(),
        };

        let Some(paths) = deletion else {
            return Ok(outcome);
        };
        outcome.vault_deleted = remove_directory(&paths.vault, "vault", &mut outcome.warnings);
        outcome.index_deleted = remove_directory(&paths.index, "index", &mut outcome.warnings);
        Ok(outcome)
    }

    pub fn migrate(
        &self,
        project: &str,
        new_code_dir: &Path,
        options: crate::project_migrate::ProjectMigrateOptions,
    ) -> Result<crate::project_migrate::ProjectMigrateOutcome> {
        crate::project_migrate::migrate(&self.manager, project, new_code_dir, options)
    }

    pub fn migrate_plan(
        &self,
        project: &str,
        new_code_dir: &Path,
        options: &crate::project_migrate::ProjectMigrateOptions,
    ) -> Result<crate::project_migrate::ProjectMigratePlan> {
        let snapshot = self.manager.snapshot();
        crate::project_migrate::build_plan(&snapshot.registry, project, new_code_dir, options)
    }

    fn validate_deletion(
        &self,
        registry: &Registry,
        project: &ProjectConfig,
    ) -> Result<DeletionPaths> {
        let configured_vault = PathBuf::from(&project.vault_path);
        if !configured_vault.is_absolute() {
            bail!("Refusing to delete a non-absolute vault path");
        }
        if configured_vault
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            bail!("Refusing to delete a vault whose root is a symlink");
        }

        let master = registry
            .global
            .global_vault_path
            .as_deref()
            .ok_or_else(|| anyhow!("Master vault path is not configured; unregister the project and delete its data manually"))?;
        let master = canonical_or_original(Path::new(master));
        let vault = canonical_or_original(&configured_vault);
        let code = canonical_or_original(Path::new(&project.code_path));
        let home = dirs::home_dir().map(|path| canonical_or_original(&path));

        if vault == Path::new("/")
            || vault == master
            || vault == code
            || home.as_ref().is_some_and(|home| home == &vault)
            || !vault.starts_with(&master)
        {
            bail!(
                "Refusing unsafe vault deletion outside a dedicated child of master vault '{}'",
                master.display()
            );
        }

        let hash = blake3::hash(vault.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let index_root = self.data_root.join("dbs");
        let index = index_root.join(hash);
        if index == index_root || !index.starts_with(&index_root) {
            bail!("Refusing unsafe index deletion path");
        }
        Ok(DeletionPaths { vault, index })
    }
}

struct DeletionPaths {
    vault: PathBuf,
    index: PathBuf,
}

fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn remove_directory(path: &Path, label: &str, warnings: &mut Vec<String>) -> bool {
    if !path.exists() {
        return true;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => true,
        Err(error) => {
            warnings.push(format!(
                "Project was unregistered, but {label} data at '{}' could not be deleted: {error}",
                path.display()
            ));
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{CodeIndexMode, GlobalConfig, ProjectConfig, Registry};
    use std::collections::HashMap;

    fn project(code: &Path, vault: &Path) -> ProjectConfig {
        ProjectConfig {
            code_path: code.to_string_lossy().to_string(),
            vault_path: vault.to_string_lossy().to_string(),
            include: vec!["**/*.md".to_string()],
            exclude: Vec::new(),
            code_index_mode: CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
        }
    }

    fn service_fixture() -> (tempfile::TempDir, ProjectService, PathBuf, PathBuf) {
        let directory = tempfile::tempdir().unwrap();
        let master = directory.path().join("vaults");
        let vault = master.join("demo");
        let code = directory.path().join("code");
        std::fs::create_dir_all(&vault).unwrap();
        std::fs::create_dir_all(&code).unwrap();
        std::fs::write(vault.join("note.md"), "# Note\n").unwrap();

        let manager = ConfigManager::at(directory.path().join("registry.toml")).unwrap();
        let registry = Registry {
            global: GlobalConfig {
                global_vault_path: Some(master.to_string_lossy().to_string()),
                ..GlobalConfig::default()
            },
            projects: HashMap::from([("demo".to_string(), project(&code, &vault))]),
            ..Registry::default()
        };
        manager.replace(0, registry).unwrap();
        let service = ProjectService::new(manager, directory.path().join("data"));
        (directory, service, vault, code)
    }

    #[test]
    fn unregister_preserves_vault() {
        let (_directory, service, vault, _code) = service_fixture();
        let outcome = service.unregister("demo").unwrap();
        assert!(outcome.registry_removed);
        assert!(!outcome.vault_deleted);
        assert!(vault.join("note.md").exists());
        assert!(service.manager.snapshot().registry.projects.is_empty());
    }

    #[test]
    fn delete_requires_exact_confirmation_and_removes_vault_and_index() {
        let (_directory, service, vault, _code) = service_fixture();
        assert!(service.delete_data("demo", "wrong").is_err());
        assert!(
            service
                .manager
                .snapshot()
                .registry
                .projects
                .contains_key("demo")
        );

        let canonical_vault = canonical_or_original(&vault);
        let hash = blake3::hash(canonical_vault.to_string_lossy().as_bytes())
            .to_hex()
            .to_string();
        let index = service.data_root.join("dbs").join(hash);
        std::fs::create_dir_all(&index).unwrap();
        std::fs::write(index.join("metadata.json"), "{}").unwrap();

        let outcome = service.delete_data("demo", "demo").unwrap();
        assert!(outcome.vault_deleted);
        assert!(outcome.index_deleted);
        assert!(outcome.warnings.is_empty());
        assert!(!vault.exists());
        assert!(!index.exists());
    }

    #[test]
    fn destructive_delete_rejects_master_vault_root() {
        let (directory, _service, _vault, code) = service_fixture();
        let master = directory.path().join("vaults");
        let manager = ConfigManager::at(directory.path().join("unsafe-registry.toml")).unwrap();
        let registry = Registry {
            global: GlobalConfig {
                global_vault_path: Some(master.to_string_lossy().to_string()),
                ..GlobalConfig::default()
            },
            projects: HashMap::from([("unsafe".to_string(), project(&code, &master))]),
            ..Registry::default()
        };
        manager.replace(0, registry).unwrap();
        let service = ProjectService::new(manager, directory.path().join("data"));
        assert!(service.delete_data("unsafe", "unsafe").is_err());
        assert!(master.exists());
        assert!(
            service
                .manager
                .snapshot()
                .registry
                .projects
                .contains_key("unsafe")
        );
    }
}
