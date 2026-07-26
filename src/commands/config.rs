use anyhow::{Result, anyhow};
use clap::Args;
use std::path::{Path, PathBuf};

const AUTO_IMPORT_STRATEGIES: [&str; 4] = ["skip", "link", "import_organize", "import"];

#[derive(Args, Debug)]
pub struct ConfigArgs {
    /// Set the master vault storage path (global)
    #[arg(long)]
    pub vault_path: Option<String>,
    /// Automatically add new projects to memory when discovered (global)
    #[arg(long)]
    pub auto_add: Option<bool>,
    /// Automatically inject IDE rules when a project is added (global)
    #[arg(long)]
    pub inject_rules: Option<bool>,
    /// Strategy for existing documents on auto-add: skip, link, import_organize, import (global)
    #[arg(long, value_name = "STRATEGY")]
    pub auto_import: Option<String>,
    /// Maximum number of index backups to keep (Write-Guard, global)
    #[arg(long, value_name = "N")]
    pub max_backups: Option<usize>,
    /// Set semantic code indexing for the current project: off, manual, or watch
    #[arg(long, value_name = "MODE")]
    pub code_index_mode: Option<String>,
    /// Comma-separated code languages to index, or `auto` for all bundled adapters
    #[arg(long, value_name = "LANGUAGES")]
    pub code_languages: Option<String>,
    /// Comma-separated vault include globs for the current project (e.g. `rules/**/*.md,**/*.md`)
    #[arg(long, value_name = "GLOBS")]
    pub include: Option<String>,
    /// Comma-separated vault exclude globs for the current project (e.g. `node_modules/**,.git/**`)
    #[arg(long, value_name = "GLOBS")]
    pub exclude: Option<String>,
    /// Allow this project's vault notes in cross-project federated search
    /// (`projects` with more than one key and `corpus=vault|all`). Default false.
    #[arg(long, value_name = "BOOL")]
    pub cross_project_vault: Option<bool>,
}

impl ConfigArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let manager = crate::config_manager::ConfigManager::open()?;
        let snapshot = manager.snapshot();
        let expected_revision = snapshot.revision;
        let mut registry = snapshot.registry;

        let has_global_flags = self.vault_path.is_some()
            || self.auto_add.is_some()
            || self.inject_rules.is_some()
            || self.auto_import.is_some()
            || self.max_backups.is_some();
        let has_project_flags = self.code_index_mode.is_some()
            || self.code_languages.is_some()
            || self.include.is_some()
            || self.exclude.is_some()
            || self.cross_project_vault.is_some();

        // Any explicit flag means scripted use: apply exactly what was passed,
        // never prompt for unrelated settings.
        if has_global_flags || has_project_flags {
            let mut updated = false;
            if has_global_flags {
                updated |= self.apply_global_flags(&mut registry)?;
            }
            if has_project_flags {
                let project = registered_project_mut(&mut registry, scope.as_deref())?;
                updated |= self.apply_project_flags(project)?;
            }
            if updated {
                let snapshot = manager.replace(expected_revision, registry)?;
                println!("Configuration saved (revision {}).", snapshot.revision);
            } else {
                println!("Configuration is already current.");
            }
            return Ok(());
        }

        print_current(&registry, scope.as_deref());

        let edit = dialoguer::Confirm::new()
            .with_prompt("Do you want to edit the global settings interactively?")
            .default(false)
            .interact()?;
        if !edit {
            return Ok(());
        }

        let updated = interactive_global_edit(&mut registry)?;
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

    fn apply_global_flags(&self, registry: &mut crate::workspace::Registry) -> Result<bool> {
        let mut updated = false;
        if let Some(path) = &self.vault_path
            && registry.global.global_vault_path.as_ref() != Some(path)
        {
            registry.global.global_vault_path = Some(path.clone());
            println!("Set global_vault_path to: {path}");
            updated = true;
        }
        if let Some(auto) = self.auto_add
            && registry.global.auto_add_projects != Some(auto)
        {
            registry.global.auto_add_projects = Some(auto);
            println!("Set auto_add_projects to: {auto}");
            updated = true;
        }
        if let Some(inject) = self.inject_rules
            && registry.global.inject_rules != Some(inject)
        {
            registry.global.inject_rules = Some(inject);
            println!("Set inject_rules to: {inject}");
            updated = true;
        }
        if let Some(strategy) = &self.auto_import {
            if !AUTO_IMPORT_STRATEGIES.contains(&strategy.as_str()) {
                return Err(anyhow!(
                    "auto_import strategy must be one of: {} (got {strategy})",
                    AUTO_IMPORT_STRATEGIES.join(", ")
                ));
            }
            if registry.global.auto_import_strategy.as_ref() != Some(strategy) {
                registry.global.auto_import_strategy = Some(strategy.clone());
                println!("Set auto_import_strategy to: {strategy}");
                updated = true;
            }
        }
        if let Some(backups) = self.max_backups
            && registry.global.max_backups != Some(backups)
        {
            registry.global.max_backups = Some(backups);
            println!("Set max_backups to: {backups}");
            updated = true;
        }
        Ok(updated)
    }

    fn apply_project_flags(&self, project: &mut crate::workspace::ProjectConfig) -> Result<bool> {
        let mut updated = false;
        if let Some(mode) = &self.code_index_mode {
            let mode = mode
                .parse::<crate::workspace::CodeIndexMode>()
                .map_err(anyhow::Error::msg)?;
            if project.code_index_mode != mode {
                project.code_index_mode = mode;
                println!("Set code_index_mode to: {mode:?}");
                updated = true;
            }
        }
        if let Some(languages) = &self.code_languages {
            let languages = parse_comma_list(languages)?;
            crate::code_parser::validate_language_config(&languages)?;
            if project.code_languages != languages {
                project.code_languages = languages.clone();
                println!("Set code_languages to: {}", languages.join(", "));
                updated = true;
            }
        }
        if let Some(include) = &self.include {
            let include = parse_glob_list(include, "include")?;
            if project.include != include {
                project.include = include.clone();
                println!("Set include to: {}", include.join(", "));
                updated = true;
            }
        }
        if let Some(exclude) = &self.exclude {
            let exclude = parse_glob_list(exclude, "exclude")?;
            if project.exclude != exclude {
                project.exclude = exclude.clone();
                println!("Set exclude to: {}", exclude.join(", "));
                updated = true;
            }
        }
        if let Some(allow) = self.cross_project_vault
            && project.cross_project_vault != allow
        {
            project.cross_project_vault = allow;
            println!("Set cross_project_vault to: {allow}");
            updated = true;
        }
        Ok(updated)
    }
}

fn parse_comma_list(raw: &str) -> Result<Vec<String>> {
    let values = raw
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(anyhow!("Provide at least one comma-separated value"));
    }
    Ok(values)
}

fn parse_glob_list(raw: &str, name: &str) -> Result<Vec<String>> {
    let values = parse_comma_list(raw)
        .map_err(|_| anyhow!("Provide at least one {name} glob (comma-separated)"))?;
    for value in &values {
        glob::Pattern::new(value)
            .map_err(|error| anyhow!("Invalid {name} glob `{value}`: {error}"))?;
    }
    Ok(values)
}

fn print_current(registry: &crate::workspace::Registry, scope: Option<&str>) {
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

    let sep =
        "+-------------------+------------------------------------------------------------------+";
    println!("{sep}");
    println!(
        "| Global setting    | Value                                                            |"
    );
    println!("{sep}");
    println!("| Vault Path        | {cv:<64} |");
    println!("| Auto Add Projects | {ca:<64} |");
    println!("| Inject Rules      | {ci:<64} |");
    println!("| Max Backups       | {cb:<64} |");
    println!("| Auto Import Strat | {cs:<64} |");
    println!("{sep}");

    match locate_project(registry, scope) {
        Some((key, project)) => {
            println!("| Project           | {key:<64} |");
            println!("| Code Path         | {:<64} |", clip(&project.code_path));
            println!("| Vault Path        | {:<64} |", clip(&project.vault_path));
            println!(
                "| Code Index Mode   | {:<64} |",
                format!("{:?}", project.code_index_mode).to_lowercase()
            );
            println!(
                "| Code Languages    | {:<64} |",
                clip(&project.code_languages.join(", "))
            );
            println!(
                "| Include           | {:<64} |",
                clip(&project.include.join(", "))
            );
            println!(
                "| Exclude           | {:<64} |",
                clip(&project.exclude.join(", "))
            );
            println!(
                "| Cross-Project Vault| {:<63} |",
                project.cross_project_vault
            );
            println!("{sep}");
            println!(
                "\nProject flags: --code-index-mode, --code-languages, --include, --exclude, --cross-project-vault (with --scope <path> when outside the repo).\n"
            );
        }
        None => {
            println!(
                "\nNo registered project matches the current directory; project flags need --scope <project-path> or `rms-memory init`.\n"
            );
        }
    }
}

fn clip(value: &str) -> String {
    if value.chars().count() <= 64 {
        value.to_string()
    } else {
        let prefix: String = value.chars().take(61).collect();
        format!("{prefix}...")
    }
}

fn locate_project<'a>(
    registry: &'a crate::workspace::Registry,
    scope: Option<&str>,
) -> Option<(String, &'a crate::workspace::ProjectConfig)> {
    let requested = match scope {
        Some(scope) => PathBuf::from(scope),
        None => std::env::current_dir().ok()?,
    };
    let requested = std::fs::canonicalize(&requested).unwrap_or(requested);
    registry
        .projects
        .iter()
        .filter(|(_, project)| requested.starts_with(Path::new(&project.code_path)))
        .max_by_key(|(_, project)| project.code_path.len())
        .map(|(key, project)| (key.clone(), project))
}

fn interactive_global_edit(registry: &mut crate::workspace::Registry) -> Result<bool> {
    let mut updated = false;

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
    let new_vault: String = dialoguer::Input::new()
        .with_prompt("Path to master vault storage")
        .default(current_vault)
        .interact_text()?;
    if Some(&new_vault) != registry.global.global_vault_path.as_ref() {
        registry.global.global_vault_path = Some(new_vault.clone());
        println!("Set global_vault_path to: {new_vault}");
        updated = true;
    }

    // 2. Auto Add Projects
    let current_auto = registry.global.auto_add_projects.unwrap_or(true);
    let new_auto = dialoguer::Confirm::new()
        .with_prompt("Automatically add new projects to memory when discovered?")
        .default(current_auto)
        .interact()?;
    if registry.global.auto_add_projects != Some(new_auto) {
        registry.global.auto_add_projects = Some(new_auto);
        println!("Set auto_add_projects to: {new_auto}");
        updated = true;
    }

    // 3. Inject Rules (false by default per user requirements)
    let current_inject = registry.global.inject_rules.unwrap_or(false);
    let new_inject = dialoguer::Confirm::new()
        .with_prompt("Automatically inject cursor/zed rules when a project is added?")
        .default(current_inject)
        .interact()?;
    if registry.global.inject_rules != Some(new_inject) {
        registry.global.inject_rules = Some(new_inject);
        println!("Set inject_rules to: {new_inject}");
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
        println!("Set max_backups to: {new_backups}");
        updated = true;
    }

    // 5. Auto Import Strategy
    let current_strategy = registry
        .global
        .auto_import_strategy
        .clone()
        .unwrap_or_else(|| "skip".to_string());
    let default_idx = AUTO_IMPORT_STRATEGIES
        .iter()
        .position(|&s| s == current_strategy)
        .unwrap_or(0);
    let selection = dialoguer::Select::new()
        .with_prompt("Strategy for handling existing documents on auto-add")
        .items(AUTO_IMPORT_STRATEGIES)
        .default(default_idx)
        .interact()?;
    let new_strategy = AUTO_IMPORT_STRATEGIES[selection].to_string();
    if registry.global.auto_import_strategy != Some(new_strategy.clone()) {
        registry.global.auto_import_strategy = Some(new_strategy.clone());
        println!("Set auto_import_strategy to: {new_strategy}");
        updated = true;
    }

    Ok(updated)
}

fn registered_project_mut<'a>(
    registry: &'a mut crate::workspace::Registry,
    scope: Option<&str>,
) -> Result<&'a mut crate::workspace::ProjectConfig> {
    let requested = match scope {
        Some(scope) => PathBuf::from(scope),
        None => std::env::current_dir()?,
    };
    let requested = std::fs::canonicalize(&requested).unwrap_or(requested);
    let key = registry
        .projects
        .iter()
        .filter(|(_, project)| requested.starts_with(Path::new(&project.code_path)))
        .max_by_key(|(_, project)| project.code_path.len())
        .map(|(key, _)| key.clone())
        .ok_or_else(|| {
            anyhow!(
                "No registered project matches {}. Run `rms-memory init` first or pass --scope <project-path>.",
                requested.display()
            )
        })?;
    registry
        .projects
        .get_mut(&key)
        .ok_or_else(|| anyhow!("Registered project disappeared while updating configuration"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn project(code_path: &str, vault_path: &str) -> crate::workspace::ProjectConfig {
        crate::workspace::ProjectConfig {
            code_path: code_path.to_string(),
            vault_path: vault_path.to_string(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![".git/**".to_string()],
            code_index_mode: crate::workspace::CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
            cross_project_vault: false,
        }
    }

    fn args() -> ConfigArgs {
        ConfigArgs {
            vault_path: None,
            auto_add: None,
            inject_rules: None,
            auto_import: None,
            max_backups: None,
            code_index_mode: None,
            code_languages: None,
            include: None,
            exclude: None,
            cross_project_vault: None,
        }
    }

    #[test]
    fn selects_the_most_specific_registered_project() {
        let mut registry = crate::workspace::Registry {
            projects: HashMap::from([
                ("parent".to_string(), project("/projects", "/vaults/parent")),
                (
                    "child".to_string(),
                    project("/projects/rms-memory", "/vaults/child"),
                ),
            ]),
            ..Default::default()
        };

        let project = registered_project_mut(&mut registry, Some("/projects/rms-memory/src"))
            .expect("child project must match");
        assert_eq!(project.vault_path, "/vaults/child");
    }

    #[test]
    fn global_flags_apply_without_prompts_and_validate_strategy() {
        let mut registry = crate::workspace::Registry::default();
        let mut config = args();
        config.max_backups = Some(9);
        config.auto_import = Some("link".to_string());
        config.inject_rules = Some(true);

        let updated = config.apply_global_flags(&mut registry).expect("apply");
        assert!(updated);
        assert_eq!(registry.global.max_backups, Some(9));
        assert_eq!(
            registry.global.auto_import_strategy.as_deref(),
            Some("link")
        );
        assert_eq!(registry.global.inject_rules, Some(true));

        // Re-applying identical values is a no-op.
        assert!(!config.apply_global_flags(&mut registry).expect("noop"));

        let mut bad = args();
        bad.auto_import = Some("everything".to_string());
        let error = bad
            .apply_global_flags(&mut registry)
            .unwrap_err()
            .to_string();
        assert!(error.contains("auto_import strategy must be one of"));
    }

    #[test]
    fn project_flags_set_include_exclude_with_glob_validation() {
        let mut target = project("/projects/app", "/vaults/app");
        let mut config = args();
        config.include = Some("rules/**/*.md, decisions/**/*.md".to_string());
        config.exclude = Some("node_modules/**,dist/**".to_string());

        let updated = config.apply_project_flags(&mut target).expect("apply");
        assert!(updated);
        assert_eq!(
            target.include,
            vec!["rules/**/*.md".to_string(), "decisions/**/*.md".to_string()]
        );
        assert_eq!(
            target.exclude,
            vec!["node_modules/**".to_string(), "dist/**".to_string()]
        );

        let mut bad = args();
        bad.include = Some("[".to_string());
        let error = bad
            .apply_project_flags(&mut target)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Invalid include glob"), "got: {error}");

        let mut empty = args();
        empty.exclude = Some(" , ".to_string());
        let error = empty
            .apply_project_flags(&mut target)
            .unwrap_err()
            .to_string();
        assert!(error.contains("at least one exclude glob"), "got: {error}");
    }
}
