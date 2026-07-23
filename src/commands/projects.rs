use crate::project_migrate::{ProjectMigrateOptions, ProjectMigrateOutcome};
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
    /// Migrate a project to a new repository path
    Migrate(MigrateArgs),
    /// Resolve a project key through migration redirects
    ResolveKey(ResolveKeyArgs),
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

#[derive(Args, Debug)]
pub struct MigrateArgs {
    /// Registered project key to migrate
    pub project: String,
    /// New repository directory
    #[arg(long)]
    pub to: String,
    /// Explicit new project key (defaults to basename with case-preserving suggestion)
    #[arg(long)]
    pub new_key: Option<String>,
    /// Preview changes without applying them
    #[arg(long)]
    pub dry_run: bool,
    /// Skip vault link repair
    #[arg(long)]
    pub no_repair_links: bool,
    /// Fail when git remotes differ
    #[arg(long)]
    pub strict_git: bool,
}

#[derive(Args, Debug)]
pub struct ResolveKeyArgs {
    pub project: String,
}

impl ProjectsCommands {
    pub fn run(&self) -> Result<()> {
        match self {
            ProjectsCommands::List => list(),
            ProjectsCommands::Locate(args) => locate(args),
            ProjectsCommands::Remove(args) => remove(args),
            ProjectsCommands::Migrate(args) => migrate(args),
            ProjectsCommands::ResolveKey(args) => resolve_key(args),
        }
    }
}

fn remove(args: &RemoveArgs) -> Result<()> {
    let removed = crate::project_service::ProjectService::open()?.unregister(&args.project)?;
    println!("Removed project '{}' from the registry.", args.project);
    println!("Vault files were preserved at: {}", removed.vault_path);
    Ok(())
}

fn migrate(args: &MigrateArgs) -> Result<()> {
    let service = crate::project_service::ProjectService::open()?;
    let options = ProjectMigrateOptions {
        new_key: args.new_key.clone(),
        dry_run: args.dry_run,
        repair_links: !args.no_repair_links,
        update_project_stamps: true,
        strict_git: args.strict_git,
    };
    let outcome = service.migrate(&args.project, std::path::Path::new(&args.to), options)?;
    print_outcome(&outcome, args.dry_run);
    Ok(())
}

fn resolve_key(args: &ResolveKeyArgs) -> Result<()> {
    let registry = Registry::load()?;
    if let Some((resolved, config)) = registry.locate_by_project_key(&args.project) {
        println!("Resolved key: {resolved}");
        println!("Code path: {}", config.code_path);
        println!("Vault:     {}", config.vault_path);
        return Ok(());
    }
    if let Some(message) = registry.migration_redirect_message(&args.project) {
        anyhow::bail!(message);
    }
    anyhow::bail!("No registered project matches '{}'", args.project);
}

fn print_outcome(outcome: &ProjectMigrateOutcome, dry_run: bool) {
    if dry_run {
        println!("Dry run — no changes applied.");
    }
    println!("Project key: {} -> {}", outcome.old_key, outcome.new_key);
    println!("Code path:   {}", outcome.code_path);
    println!("Vault path:  {}", outcome.vault_path);
    if outcome.links_repaired > 0 {
        println!("Link repairs: {}", outcome.links_repaired);
    }
    if outcome.project_stamps_updated > 0 {
        println!("Project stamps: {}", outcome.project_stamps_updated);
    }
    if outcome.vault_moved {
        println!("Vault directory moved.");
    }
    if outcome.db_moved {
        println!("Index directory moved.");
    }
    if outcome.redirect_created {
        println!(
            "Migration redirect created for legacy key '{}'.",
            outcome.old_key
        );
    }
    if !outcome.warnings.is_empty() {
        println!("Warnings:");
        for warning in &outcome.warnings {
            println!("  - {warning}");
        }
    }
    if outcome.restart_required && !dry_run {
        println!("Restart MCP/GUI so watchers pick up the new code path.");
    }
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
        match registry.locate_by_project_key(key) {
            Some((resolved, proj)) => {
                println!("Key:       {}", resolved);
                println!("Code path: {}", proj.code_path);
                println!("Vault:     {}", proj.vault_path);
            }
            None => {
                if let Some(message) = registry.migration_redirect_message(key) {
                    anyhow::bail!(message);
                }
                println!("No project found with key: {}", key);
            }
        }
    } else {
        anyhow::bail!("Pass --vault <path> or --project <key> to locate a project");
    }

    Ok(())
}
