use std::path::{Path, PathBuf};
use serde::{Deserialize, Serialize};
use std::fs;
use std::collections::HashMap;
use anyhow::{anyhow, Context, Result};
use glob::Pattern;

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct Registry {
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub projects: HashMap<String, ProjectConfig>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct GlobalConfig {
    pub global_vault_path: Option<String>,
    pub auto_add_projects: Option<bool>,
    pub inject_rules: Option<bool>,
    pub max_backups: Option<usize>,
    pub auto_import_strategy: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct ProjectConfig {
    pub code_path: String,
    pub vault_path: String,
    #[serde(default = "default_include")]
    pub include: Vec<String>,
    #[serde(default = "default_exclude")]
    pub exclude: Vec<String>,
}

fn default_include() -> Vec<String> {
    vec!["rules/**/*.md".to_string(), "decisions/**/*.md".to_string(), "architecture/**/*.md".to_string(), "artifacts/**/*.md".to_string(), "**/*.md".to_string()]
}

fn default_exclude() -> Vec<String> {
    vec!["node_modules/**".to_string(), "vendor/**".to_string(), ".git/**".to_string()]
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
                eprintln!("[WARN] ~/.rms-memory is not writable (sandbox restriction?). Falling back to temp_dir.");
            }
        } else {
            eprintln!("[WARN] Cannot create ~/.rms-memory (sandbox restriction?). Falling back to temp_dir.");
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

#[derive(Debug, Clone)]
pub struct Workspace {
    pub root: PathBuf, // This points to the vault_path
    pub code_path: PathBuf,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Workspace {
    pub fn discover(start_dir: &Path, options: Option<crate::rules_injector::InjectOptions>) -> Result<Self> {
        let mut registry = Registry::load()?;
        let start_canon = fs::canonicalize(start_dir).unwrap_or_else(|_| start_dir.to_path_buf());
        let start_str = start_canon.to_string_lossy().to_string();

        if start_str == "/" {
            return Err(anyhow::anyhow!("Cannot discover or auto-add root directory '/' as a project. The MCP client must provide a valid workspace path."));
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
            // For existing projects, check if we need to re-inject rules
            if registry.global.inject_rules.unwrap_or(false) {
                let inject_opts = options.unwrap_or_default();
                let proj_path = PathBuf::from(&project.code_path);
                if proj_path.exists() {
                    let _ = crate::rules_injector::inject_rules(&proj_path, inject_opts);
                }
            }
            
            return Ok(Workspace {
                root: PathBuf::from(&project.vault_path),
                code_path: PathBuf::from(&project.code_path),
                include: project.include.clone(),
                exclude: project.exclude.clone(),
            });
        }

        // Auto-add logic
        if registry.global.auto_add_projects == Some(true) {
            let global_vault = registry.global.global_vault_path
                .as_ref()
                .ok_or_else(|| anyhow!("global_vault_path is not configured in registry.toml"))?;
                
            let folder_name = start_canon.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("UnknownProject")
                .to_string();
                
            let vault_path = Path::new(global_vault).join(&folder_name).to_string_lossy().to_string();
            
            // Create the vault folders
            fs::create_dir_all(Path::new(&vault_path).join("rules"))?;
            fs::create_dir_all(Path::new(&vault_path).join("decisions"))?;
            fs::create_dir_all(Path::new(&vault_path).join("architecture"))?;
            fs::create_dir_all(Path::new(&vault_path).join("artifacts"))?;

            let project_config = ProjectConfig {
                code_path: start_str.clone(),
                vault_path: vault_path.clone(),
                include: default_include(),
                exclude: default_exclude(),
            };
            
            registry.projects.insert(folder_name, project_config.clone());
            registry.save()?;

            if registry.global.inject_rules.unwrap_or(false) {
                let inject_opts = options.unwrap_or_default();
                if let Err(e) = crate::rules_injector::inject_rules(&start_canon, inject_opts) {
                    eprintln!("Warning: Failed to inject rules into repository: {}", e);
                } else {
                    println!("Successfully injected RMS Memory rules into IDE configs.");
                }
            } else {
                println!("[INFO] Rules injection disabled by default, skipping auto-configuration of repository rules. Run `rms-memory init` to explicitly enable.");
            }
            
            if let Some(strategy) = &registry.global.auto_import_strategy {
                if strategy != "skip" {
                    let import_service = crate::import::ImportService::new(start_canon.clone(), PathBuf::from(&vault_path));
                    let docs = import_service.detect_existing_docs();
                    if !docs.is_empty() {
                        let action = match strategy.as_str() {
                            "link" => crate::import::ImportAction::LinkOnly,
                            "import_organize" => crate::import::ImportAction::ImportAndOrganize,
                            "import" => crate::import::ImportAction::Import,
                            _ => crate::import::ImportAction::Skip,
                        };
                        if let Err(e) = import_service.execute(action, docs) {
                            tracing::warn!("Auto-import failed: {}", e);
                        } else {
                            tracing::info!("Auto-import completed using strategy: {}", strategy);
                        }
                    }
                }
            } else {
                tracing::info!("Auto-initialized vault. Run 'rms-memory import' to import existing docs.");
            }
            
            Ok(Workspace {
                root: PathBuf::from(&project_config.vault_path),
                code_path: PathBuf::from(&project_config.code_path),
                include: project_config.include.clone(),
                exclude: project_config.exclude.clone(),
            })
        } else {
            Err(anyhow!("Project not found in registry and auto_add_projects is false or unset"))
        }
    }

    pub fn canonical_path(&self) -> Result<String> {
        let canon = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        Ok(canon.to_string_lossy().to_string())
    }

    pub fn project_hash(&self) -> Result<String> {
        let canon = self.canonical_path()?;
        let hash = blake3::hash(canon.as_bytes());
        Ok(hash.to_hex().to_string())
    }

    pub fn find_markdown_files(&self) -> Result<Vec<PathBuf>> {
        use glob::{glob, Pattern};
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
                    if included && !excluded {
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
}
