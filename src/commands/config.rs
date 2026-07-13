use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct ConfigArgs {
    #[arg(long)]
    pub vault_path: Option<String>,
    #[arg(long)]
    pub auto_add: Option<bool>,
    #[arg(long)]
    pub inject_rules: Option<bool>,
    #[arg(long)]
    pub auto_import: Option<String>,
}

impl ConfigArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        let manager = crate::config_manager::ConfigManager::open()?;
        let snapshot = manager.snapshot();
        let expected_revision = snapshot.revision;
        let mut registry = snapshot.registry;
        let mut updated = false;

        if self.vault_path.is_none()
            && self.auto_add.is_none()
            && self.inject_rules.is_none()
            && self.auto_import.is_none()
        {
            let cv = registry
                .global
                .global_vault_path
                .as_deref()
                .unwrap_or("Not Set");
            let ca = registry.global.auto_add_projects.unwrap_or(true);
            let ci = registry.global.inject_rules.unwrap_or(false);
            let cb = registry.global.max_backups.unwrap_or(5);
            let cs = registry
                .global
                .auto_import_strategy
                .as_deref()
                .unwrap_or("skip");

            println!(
                "+-------------------+------------------------------------------------------------------+"
            );
            println!(
                "| Setting           | Value                                                            |"
            );
            println!(
                "+-------------------+------------------------------------------------------------------+"
            );
            println!("| Vault Path        | {:<64} |", cv);
            println!("| Auto Add Projects | {:<64} |", ca);
            println!("| Inject Rules      | {:<64} |", ci);
            println!("| Max Backups       | {:<64} |", cb);
            println!("| Auto Import Strat | {:<64} |", cs);
            println!(
                "+-------------------+------------------------------------------------------------------+\n"
            );

            let edit = dialoguer::Confirm::new()
                .with_prompt("Do you want to edit these settings interactively?")
                .default(false)
                .interact()?;

            if !edit {
                return Ok(());
            }
        }

        // 1. Vault Path
        let current_vault = registry
            .global
            .global_vault_path
            .clone()
            .unwrap_or_else(|| {
                let mut p = dirs::home_dir().unwrap_or_default();
                p.push(".rms-memory");
                p.push("vaults");
                p.to_string_lossy().to_string()
            });

        let new_vault: String = if let Some(path) = &self.vault_path {
            path.clone()
        } else {
            dialoguer::Input::new()
                .with_prompt("Path to master vault storage")
                .default(current_vault)
                .interact_text()?
        };
        if Some(&new_vault) != registry.global.global_vault_path.as_ref() {
            registry.global.global_vault_path = Some(new_vault.clone());
            println!("Set global_vault_path to: {}", new_vault);
            updated = true;
        }

        // 2. Auto Add Projects
        let current_auto = registry.global.auto_add_projects.unwrap_or(true);
        let new_auto = if let Some(auto) = self.auto_add {
            auto
        } else {
            dialoguer::Confirm::new()
                .with_prompt("Automatically add new projects to memory when discovered?")
                .default(current_auto)
                .interact()?
        };
        if registry.global.auto_add_projects != Some(new_auto) {
            registry.global.auto_add_projects = Some(new_auto);
            println!("Set auto_add_projects to: {}", new_auto);
            updated = true;
        }

        // 3. Inject Rules (False by default per user requirements)
        let current_inject = registry.global.inject_rules.unwrap_or(false);
        let new_inject = if let Some(inject) = self.inject_rules {
            inject
        } else {
            dialoguer::Confirm::new()
                .with_prompt("Automatically inject cursor/zed rules when a project is added?")
                .default(current_inject)
                .interact()?
        };
        if registry.global.inject_rules != Some(new_inject) {
            registry.global.inject_rules = Some(new_inject);
            println!("Set inject_rules to: {}", new_inject);
            updated = true;
        }

        // 4. Max Backups
        let current_backups = registry.global.max_backups.unwrap_or(5);
        let new_backups: usize = dialoguer::Input::new()
            .with_prompt("Maximum number of index backups to keep (Write-Guard)")
            .default(current_backups)
            .interact_text()?;
        if registry.global.max_backups != Some(new_backups) {
            registry.global.max_backups = Some(new_backups);
            println!("Set max_backups to: {}", new_backups);
            updated = true;
        }

        // 5. Auto Import Strategy
        let current_strategy = registry
            .global
            .auto_import_strategy
            .clone()
            .unwrap_or_else(|| "skip".to_string());
        let new_strategy = if let Some(strat) = &self.auto_import {
            strat.clone()
        } else {
            let items = vec!["skip", "link", "import_organize", "import"];
            let default_idx = items
                .iter()
                .position(|&s| s == current_strategy)
                .unwrap_or(0);
            let selection = dialoguer::Select::new()
                .with_prompt("Strategy for handling existing documents on auto-add")
                .items(&items)
                .default(default_idx)
                .interact()?;
            items[selection].to_string()
        };
        if registry.global.auto_import_strategy != Some(new_strategy.clone()) {
            registry.global.auto_import_strategy = Some(new_strategy.clone());
            println!("Set auto_import_strategy to: {}", new_strategy);
            updated = true;
        }

        if updated {
            let snapshot = manager.replace(expected_revision, registry)?;
            println!(
                "Configuration saved successfully (revision {}).",
                snapshot.revision
            );
        } else {
            println!("No changes made to configuration.");
        }

        Ok(())
    }
}
