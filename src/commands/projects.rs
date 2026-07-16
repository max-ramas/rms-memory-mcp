use crate::workspace::Registry;
use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Subcommand, Debug)]
pub enum ProjectsCommands {
    /// List all registered projects
    List,
    /// Locate a project by vault path or project key
    Locate(LocateArgs),
    /// Remove a project registration without deleting its vault files
    Remove(RemoveArgs),
}

#[derive(Args, Debug)]
pub struct LocateArgs {
    /// Find by vault path
    #[arg(long)]
    pub vault: Option<String>,
    /// Find by project key
    #[arg(long)]
    pub project: Option<String>,
}

#[derive(Args, Debug)]
pub struct RemoveArgs {
    /// Registered project key to remove
    pub project: String,
}

impl ProjectsCommands {
    pub fn run(&self) -> Result<()> {
        match self {
            ProjectsCommands::List => list(),
            ProjectsCommands::Locate(args) => locate(args),
            ProjectsCommands::Remove(args) => remove(args),
        }
    }
}

fn remove(args: &RemoveArgs) -> Result<()> {
    let manager = crate::config_manager::ConfigManager::open()?;
    let snapshot = manager.snapshot();
    let mut registry = snapshot.registry;
    let removed = registry
        .projects
        .remove(&args.project)
        .ok_or_else(|| anyhow::anyhow!("Project '{}' is not registered", args.project))?;
    manager.replace(snapshot.revision, registry)?;
    println!("Removed project '{}' from the registry.", args.project);
    println!("Vault files were preserved at: {}", removed.vault_path);
    Ok(())
}

fn list() -> Result<()> {
    let registry = Registry::load()?;
    if registry.projects.is_empty() {
        println!("No projects registered.");
        return Ok(());
    }
    println!("{:<30} {:<50} {:<50}", "KEY", "CODE PATH", "VAULT PATH");
    println!("{}", "-".repeat(130));
    let mut entries: Vec<_> = registry.list_projects();
    entries.sort_by_key(|(a, _)| *a);
    for (key, proj) in entries {
        println!("{:<30} {:<50} {:<50}", key, proj.code_path, proj.vault_path);
    }
    Ok(())
}

fn locate(args: &LocateArgs) -> Result<()> {
    let registry = Registry::load()?;

    if let Some(vault) = &args.vault {
        match registry.locate_by_vault(vault) {
            Some((key, proj)) => {
                println!("Key:       {}", key);
                println!("Code path: {}", proj.code_path);
                println!("Vault:     {}", proj.vault_path);
            }
            None => {
                println!("No project found with vault path: {}", vault);
            }
        }
    } else if let Some(key) = &args.project {
        match registry.locate_by_project(key) {
            Some(proj) => {
                println!("Key:       {}", key);
                println!("Code path: {}", proj.code_path);
                println!("Vault:     {}", proj.vault_path);
            }
            None => {
                println!("No project found with key: {}", key);
            }
        }
    } else {
        anyhow::bail!("Pass --vault <path> or --project <key> to locate a project");
    }

    Ok(())
}
