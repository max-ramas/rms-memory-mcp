use super::CommandRunner;
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

impl CommandRunner for InitArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let start_canon =
            std::fs::canonicalize(&current_dir).unwrap_or_else(|_| current_dir.to_path_buf());
        let folder_name = start_canon
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("UnknownProject")
            .to_string();

        let mut registry = crate::workspace::Registry::load()?;
        if let Some(global_vault) = &registry.global.global_vault_path {
            let vault_path = std::path::Path::new(global_vault)
                .join(&folder_name)
                .to_string_lossy()
                .to_string();
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("rules"))?;
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("decisions"))?;
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("architecture"))?;
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("artifacts"))?;
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("docs"))?;
            std::fs::create_dir_all(std::path::Path::new(&vault_path).join("api"))?;

            registry.projects.insert(
                folder_name.clone(),
                crate::workspace::ProjectConfig {
                    code_path: start_canon.to_string_lossy().to_string(),
                    vault_path: vault_path.clone(),
                    include: vec![
                        "rules/**/*.md".to_string(),
                        "decisions/**/*.md".to_string(),
                        "architecture/**/*.md".to_string(),
                        "artifacts/**/*.md".to_string(),
                        "**/*.md".to_string(),
                    ],
                    exclude: vec![
                        "node_modules/**".to_string(),
                        "vendor/**".to_string(),
                        ".git/**".to_string(),
                    ],
                },
            );
            if !self.dry_run {
                registry.save()?;
                println!(
                    "Manually initialized project {} in global registry.",
                    folder_name
                );
            } else {
                println!(
                    "[DRY-RUN] Would have saved project {} to global registry.",
                    folder_name
                );
            }

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
        } else {
            println!(
                "Please set global_vault_path first using: rms-memory config --vault-path <PATH>"
            );
        }

        Ok(())
    }
}
