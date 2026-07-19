use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    #[serde(default = "default_registry_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub config_revision: u64,
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub projects: HashMap<String, ProjectConfig>,
}

impl Default for Registry {
    fn default() -> Self {
        Self {
            schema_version: REGISTRY_SCHEMA_VERSION,
            config_revision: 0,
            global: GlobalConfig::default(),
            projects: HashMap::new(),
        }
    }
}

fn default_registry_schema_version() -> u32 {
    REGISTRY_SCHEMA_VERSION
}

#[derive(Deserialize, Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalConfig {
    pub global_vault_path: Option<String>,
    pub auto_add_projects: Option<bool>,
    pub inject_rules: Option<bool>,
    pub max_backups: Option<usize>,
    pub auto_import_strategy: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, PartialEq, Eq)]
pub struct ProjectConfig {
    pub code_path: String,
    pub vault_path: String,
    #[serde(default = "default_include")]
    pub include: Vec<String>,
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub code_index_mode: CodeIndexMode,
    #[serde(default = "default_code_languages")]
    pub code_languages: Vec<String>,
}

fn default_code_languages() -> Vec<String> {
    vec!["auto".to_string()]
}

#[derive(Deserialize, Serialize, Debug, Clone, Copy, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CodeIndexMode {
    #[default]
    Off,
    Manual,
    Watch,
}

impl FromStr for CodeIndexMode {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "manual" => Ok(Self::Manual),
            "watch" => Ok(Self::Watch),
            _ => Err(format!(
                "code index mode must be one of: off, manual, watch (got {value})"
            )),
        }
    }
}

fn default_include() -> Vec<String> {
    vec![
        "rules/**/*.md".to_string(),
        "decisions/**/*.md".to_string(),
        "architecture/**/*.md".to_string(),
        "artifacts/**/*.md".to_string(),
        "**/*.md".to_string(),
    ]
}

fn default_exclude() -> Vec<String> {
    vec![
        "node_modules/**".to_string(),
        "vendor/**".to_string(),
        ".git/**".to_string(),
    ]
}

pub fn base_dir() -> PathBuf {
    if let Some(base_dirs) = directories::BaseDirs::new() {
        let path = base_dirs.home_dir().join(".rms-memory");
        if std::fs::create_dir_all(&path).is_ok() {
            let test_file = path.join(".write_test");
            if std::fs::write(&test_file, b"").is_ok() {
                let _ = std::fs::remove_file(test_file);
                return path;
            } else {
                eprintln!(
                    "[WARN] ~/.rms-memory is not writable (sandbox restriction?). Falling back to temp_dir."
                );
            }
        } else {
            eprintln!(
                "[WARN] Cannot create ~/.rms-memory (sandbox restriction?). Falling back to temp_dir."
            );
        }
    } else {
        eprintln!("[WARN] Cannot find HOME directory. Falling back to temp_dir.");
    }

    let fallback = std::env::temp_dir().join("rms-memory");
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

impl Registry {
    pub fn config_path() -> PathBuf {
        base_dir().join("registry.toml")
    }

    pub fn load() -> Result<Self> {
        let path = Self::config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path)?;
        let registry: Registry = toml::from_str(&content)?;
        Ok(registry)
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        fs::write(path, content)?;
        Ok(())
    }
}

const VAULT_DIRS: &[&str] = &[
    "rules",
    "decisions",
    "architecture",
    "artifacts",
    "docs",
    "api",
];

pub fn create_vault_dirs(vault_path: &Path) -> Result<()> {
    for dir in VAULT_DIRS {
        fs::create_dir_all(vault_path.join(dir))?;
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf, // This points to the vault_path
    pub code_path: PathBuf,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub code_index_mode: CodeIndexMode,
    pub code_languages: Vec<String>,
}

impl Workspace {
    /// Discover or create a workspace for an arbitrary scope identifier.
    /// For path-based scopes, falls through to the existing path-based discover logic.
    /// For opaque string scopes, creates a hash-based vault at base_dir()/vaults/<hash>.
    pub fn discover_with_scope(
        scope_override: Option<&str>,
        cwd: &Path,
        _options: Option<crate::rules_injector::InjectOptions>,
    ) -> Result<Self> {
        let identifier = Self::resolve_identifier(scope_override, cwd)?;
        let cwd_path = std::path::Path::new(&identifier);

        // If the identifier is a valid existing path, use path-based discover
        if cwd_path.exists() {
            return Self::discover(cwd_path, _options);
        }

        // Opaque scope — create hash-based vault
        let hash = Self::project_hash_for(&identifier);
        let vault_path = base_dir().join("vaults").join(&hash);
        let code_path = cwd.to_path_buf();

        // Create vault directories
        create_vault_dirs(&vault_path)?;

        Ok(Workspace {
            root: vault_path,
            code_path,
            include: default_include(),
            exclude: default_exclude(),
            code_index_mode: CodeIndexMode::Off,
            code_languages: default_code_languages(),
        })
    }

    pub fn discover(
        start_dir: &Path,
        _options: Option<crate::rules_injector::InjectOptions>,
    ) -> Result<Self> {
        let config_manager = crate::config_manager::ConfigManager::open()?;
        let config_snapshot = config_manager.snapshot();
        let registry = config_snapshot.registry;
        let start_canon = fs::canonicalize(start_dir).unwrap_or_else(|_| start_dir.to_path_buf());
        let start_str = start_canon.to_string_lossy().to_string();

        if start_str == "/" {
            return Err(anyhow::anyhow!(
                "Cannot discover or auto-add root directory '/' as a project. The MCP client must provide a valid workspace path."
            ));
        }

        // Find existing project using longest prefix match
        let mut best_match: Option<(&String, &ProjectConfig)> = None;
        for (name, project) in &registry.projects {
            // Ignore corrupted root-level catch-all projects
            if project.code_path == "/" {
                continue;
            }
            if start_str.starts_with(&project.code_path) {
                if let Some((_, best_proj)) = best_match {
                    if project.code_path.len() > best_proj.code_path.len() {
                        best_match = Some((name, project));
                    }
                } else {
                    best_match = Some((name, project));
                }
            }
        }

        if let Some((_, project)) = best_match {
            // Workspace discovery is intentionally read-only with respect to a
            // source repository. Rules are provisioned only by an explicit init
            // or rules-sync operation, never by routine CLI/MCP/GUI resolution.
            return Ok(Workspace {
                root: PathBuf::from(&project.vault_path),
                code_path: PathBuf::from(&project.code_path),
                include: project.include.clone(),
                exclude: project.exclude.clone(),
                code_index_mode: project.code_index_mode,
                code_languages: project.code_languages.clone(),
            });
        }

        Err(anyhow!(
            "Project not found in registry. Use `rms-memory init` to register '{}' as a project.",
            start_canon.display()
        ))
    }

    pub fn canonical_path(&self) -> Result<String> {
        let canon = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        Ok(canon.to_string_lossy().to_string())
    }

    pub fn project_hash(&self) -> Result<String> {
        let canon = self.canonical_path()?;
        Ok(Self::project_hash_for(&canon))
    }

    /// Compute a deterministic hash for an arbitrary scope identifier.
    /// Scope can be a filesystem path or an opaque string (e.g., "thread:12345").
    pub fn project_hash_for(identifier: &str) -> String {
        blake3::hash(identifier.as_bytes()).to_hex().to_string()
    }

    /// Resolve scope override to an identifier string.
    /// Rules:
    ///   - Absolute paths (start with /) → canonicalize
    ///   - Relative path-like (./ , ../) → resolve against cwd, canonicalize
    ///   - Opaque string → use as-is
    ///   - None → canonicalize cwd (current behavior)
    pub fn resolve_identifier(scope_override: Option<&str>, cwd: &Path) -> Result<String> {
        match scope_override {
            Some("") => Err(anyhow::anyhow!("scope must be non-empty")),
            Some(s) if s.len() > 512 => Err(anyhow::anyhow!("scope too long (max 512 characters)")),
            Some(s) if s.starts_with('/') => {
                let canonical = fs::canonicalize(s)
                    .map_err(|e| anyhow::anyhow!("scope path does not exist: {}: {}", s, e))?;
                Ok(canonical.to_string_lossy().to_string())
            }
            Some(s) if s.starts_with("./") || s.starts_with("../") => {
                let resolved = cwd.join(s);
                let canonical = fs::canonicalize(&resolved).map_err(|e| {
                    anyhow::anyhow!("scope path does not exist: {:?}: {}", resolved, e)
                })?;
                Ok(canonical.to_string_lossy().to_string())
            }
            Some(s) => {
                // Opaque identifier — use as-is
                Ok(s.to_string())
            }
            None => {
                let canonical = fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
                Ok(canonical.to_string_lossy().to_string())
            }
        }
    }

    pub fn find_markdown_files(&self) -> Result<Vec<PathBuf>> {
        use glob::{Pattern, glob};
        let mut include_patterns = Vec::new();
        for inc in &self.include {
            let pat = self.root.join(inc).to_string_lossy().to_string();
            include_patterns.push(Pattern::new(&pat)?);
        }

        let mut exclude_patterns = Vec::new();
        for exc in &self.exclude {
            let pat = self.root.join(exc).to_string_lossy().to_string();
            exclude_patterns.push(Pattern::new(&pat)?);
        }

        let mut files = Vec::new();
        for entry in glob(&self.root.join("**/*.md").to_string_lossy())? {
            match entry {
                Ok(path) => {
                    let path_str = path.to_string_lossy();
                    let included = include_patterns.iter().any(|p| p.matches(&path_str));
                    let excluded = exclude_patterns.iter().any(|p| p.matches(&path_str));
                    let canonical_excluded =
                        crate::path_policy::is_vault_wiki_path(&self.root, &path);
                    if included && !excluded && !canonical_excluded {
                        files.push(path);
                    }
                }
                Err(e) => eprintln!("Error reading glob entry: {:?}", e),
            }
        }
        Ok(files)
    }

    pub async fn get_store(&self) -> Result<crate::store::Store> {
        let hash = self.project_hash()?;
        let db_path = base_dir().join("dbs").join(hash);
        crate::store::Store::init(&db_path.to_string_lossy(), "memory").await
    }

    /// Returns the project key (registry name) for this workspace, if found.
    pub fn project_key(&self) -> Option<String> {
        if let Ok(registry) = Registry::load() {
            for (name, proj) in &registry.projects {
                if proj.vault_path == self.root.to_string_lossy()
                    || proj.code_path == self.code_path.to_string_lossy()
                {
                    return Some(name.clone());
                }
            }
        }
        None
    }
}

impl Registry {
    pub fn list_projects(&self) -> Vec<(&String, &ProjectConfig)> {
        self.projects.iter().collect()
    }

    pub fn locate_by_vault(&self, vault_path: &str) -> Option<(&String, &ProjectConfig)> {
        self.projects
            .iter()
            .find(|(_, p)| p.vault_path == vault_path)
    }

    pub fn locate_by_project(&self, key: &str) -> Option<&ProjectConfig> {
        self.projects.get(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_project_hash_regression() {
        let cwd = std::env::current_dir().unwrap();
        let old_hash = blake3::hash(fs::canonicalize(&cwd).unwrap().to_string_lossy().as_bytes())
            .to_hex()
            .to_string();

        let identifier = Workspace::resolve_identifier(None, &cwd).unwrap();
        let new_hash = Workspace::project_hash_for(&identifier);

        assert_eq!(
            old_hash, new_hash,
            "Hash regression: scope=None produces different hash than old blake3(canonicalize(cwd))"
        );
    }

    #[test]
    fn test_scope_explicit_path_equals_implicit() {
        let cwd = std::env::current_dir().unwrap();
        let canonical = fs::canonicalize(&cwd).unwrap();

        let implicit = Workspace::resolve_identifier(None, &cwd).unwrap();
        let explicit =
            Workspace::resolve_identifier(Some(&canonical.to_string_lossy()), &cwd).unwrap();

        assert_eq!(
            implicit, explicit,
            "Explicit scope with canonical path must match implicit (no --scope)"
        );
    }

    #[test]
    fn test_opaque_scope_deterministic() {
        let hash1 = Workspace::project_hash_for("thread:abc-123");
        let hash2 = Workspace::project_hash_for("thread:abc-123");
        assert_eq!(hash1, hash2, "Same scope must produce same hash");
        assert_ne!(
            hash1,
            Workspace::project_hash_for("thread:xyz-999"),
            "Different scopes must produce different hashes"
        );
    }

    #[test]
    fn test_none_scope_uses_cwd_not_server_cwd() {
        // Regression: when scope=None (no --scope flag),
        // discover_with_scope must derive identifier from the `cwd` parameter
        // (which is the MCP rootUri path), NOT from the process cwd.
        // The process cwd may differ from rootUri when the IDE spawns the server.

        let project_dir = std::env::temp_dir().join("rms-test-cwd-diff");
        std::fs::create_dir_all(&project_dir).ok();

        let cwd_elsewhere = std::env::temp_dir(); // different from project_dir

        let id_from_project = Workspace::resolve_identifier(None, &project_dir).unwrap();
        let id_from_elsewhere = Workspace::resolve_identifier(None, &cwd_elsewhere).unwrap();

        // None scope → each call uses its own cwd
        assert_ne!(
            id_from_project, id_from_elsewhere,
            "scope=None must resolve to different identifiers for different cwd paths"
        );

        // Verify an explicit project scope is independent of the process cwd.
        assert_eq!(
            id_from_project,
            Workspace::resolve_identifier(Some(&id_from_project), &cwd_elsewhere,).unwrap(),
            "Explicit scope with project path must produce same identifier regardless of cwd"
        );
        // Clean up: if someone runs discover_with_scope, it may create vault dirs
    }

    #[test]
    fn markdown_discovery_excludes_wiki_but_keeps_files_on_disk() {
        let directory = tempfile::tempdir().unwrap();
        let vault = directory.path().join("vault");
        std::fs::create_dir_all(vault.join("docs")).unwrap();
        std::fs::create_dir_all(vault.join("wiki/.generation")).unwrap();
        std::fs::create_dir_all(vault.join("wiki/_archive")).unwrap();
        std::fs::write(vault.join("docs/source.md"), "# Source\n").unwrap();
        std::fs::write(vault.join("wiki/page.md"), "# Projection\n").unwrap();
        std::fs::write(
            vault.join("wiki/.generation/context-pack.md"),
            "# Generated\n",
        )
        .unwrap();
        std::fs::write(vault.join("wiki/_archive/old.md"), "# Old\n").unwrap();

        let workspace = Workspace {
            root: vault.clone(),
            code_path: directory.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: Vec::new(),
            code_index_mode: CodeIndexMode::Off,
            code_languages: default_code_languages(),
        };

        let files = workspace.find_markdown_files().unwrap();
        assert_eq!(files, vec![vault.join("docs/source.md")]);
        assert!(vault.join("wiki/page.md").exists());
        assert!(vault.join("wiki/.generation/context-pack.md").exists());
        assert!(vault.join("wiki/_archive/old.md").exists());
    }
}
