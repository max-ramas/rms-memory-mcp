use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct InitArgs {
    #[arg(short, long)]
    pub dry_run: bool,
    #[arg(short, long)]
    pub force: bool,
    #[arg(long)]
    pub full: bool,
}

impl InitArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let start_dir = scope
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or(current_dir.clone());
        let start_canon = crate::workspace::canonical_or_given(&start_dir);
        let folder_name = start_canon
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("UnknownProject")
            .to_string();

        let manager = crate::config_manager::ConfigManager::open()?;
        let snapshot = manager.snapshot();
        let expected_revision = snapshot.revision;
        let mut registry = snapshot.registry;
        let Some(global_vault) = registry.global.global_vault_path.clone() else {
            println!(
                "Please set global_vault_path first using: rms-memory config --vault-path <PATH>"
            );
            return Ok(());
        };

        let registered = registry.projects.iter().find(|(_, project)| {
            crate::workspace::canonical_or_given(std::path::Path::new(&project.code_path))
                == start_canon
        });
        let vault_path = match registered {
            Some((key, project)) if project.read_only => anyhow::bail!(
                "{} is attached as the read-only vault `{key}`; `init` would make it writable. Use `rms-memory projects remove {key}` first to manage it as a project.",
                start_canon.display()
            ),
            Some((key, project)) => {
                println!("Already registered as project `{key}`; refreshing its rules.");
                project.vault_path.clone()
            }
            None => {
                if let Some((other, project)) = registry.locate_by_project_key(&folder_name) {
                    anyhow::bail!(
                        "Project key `{folder_name}` already refers to project `{other}` ({}); rename this folder or remove that project first.",
                        project.code_path
                    );
                }
                let vault_path = std::path::Path::new(&global_vault)
                    .join(&folder_name)
                    .to_string_lossy()
                    .to_string();
                self.register(
                    &manager,
                    expected_revision,
                    &mut registry,
                    &folder_name,
                    &start_canon,
                    &vault_path,
                )?;
                vault_path
            }
        };

        if registry.global.inject_rules.unwrap_or(true) || self.force {
            let opts = crate::rules_injector::InjectOptions {
                dry_run: self.dry_run,
                force: self.force,
                full: self.full,
                interactive: true,
            };
            if let Err(e) = crate::rules_injector::inject_rules(&start_canon, opts) {
                eprintln!("Warning: Failed to inject rules: {}", e);
            } else if !self.dry_run {
                println!("Successfully injected RMS Memory rules into IDE configs.");
            }
        }

        if !self.dry_run {
            let import_service = crate::import::ImportService::new(
                start_canon,
                std::path::PathBuf::from(&vault_path),
            );
            let docs = import_service.detect_existing_docs();
            if !docs.is_empty()
                && let Ok(action) = import_service.prompt_action(&docs)
                && let Err(e) = import_service.execute(action, docs)
            {
                eprintln!("Warning: Failed to import documents: {}", e);
            }
        }

        Ok(())
    }

    fn register(
        &self,
        manager: &crate::config_manager::ConfigManager,
        expected_revision: u64,
        registry: &mut crate::workspace::Registry,
        key: &str,
        code_path: &std::path::Path,
        vault_path: &str,
    ) -> Result<()> {
        let vault_dir = std::path::Path::new(vault_path);
        crate::workspace::ensure_vault_writable_in(registry, vault_dir)?;
        if self.dry_run {
            println!("[DRY-RUN] Would have saved project {key} to global registry.");
            return Ok(());
        }
        if crate::workspace::is_foreign_folder(vault_dir) {
            println!(
                "Vault folder {} already has content that is not an RMS vault; leaving it as is (no rules/, decisions/, … folders created).",
                vault_dir.display()
            );
        } else {
            crate::workspace::create_vault_dirs(vault_dir)?;
        }
        registry.projects.insert(
            key.to_string(),
            crate::workspace::ProjectConfig::new(code_path, vault_dir),
        );
        manager.replace(expected_revision, registry.clone())?;
        println!("Manually initialized project {key} in global registry.");
        Ok(())
    }
}
