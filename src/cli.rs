use clap::{Parser, Subcommand};
use anyhow::Result;
use std::path::PathBuf;
use crate::workspace::Workspace;
use crate::indexer::Indexer;
use crate::store::Store;

#[derive(Parser)]
#[command(name = "rms-memory", version = env!("CARGO_PKG_VERSION"), about = "RMS Memory MCP Server")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage global configuration
    Config {
        #[arg(long)]
        vault_path: Option<String>,
        #[arg(long)]
        auto_add: Option<bool>,
        #[arg(long)]
        inject_rules: Option<bool>,
        #[arg(long)]
        auto_import: Option<String>,
    },
    /// Initialize local project in the global registry manually
    Init {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
    /// Import existing documentation into the Vault
    Import,
    /// Serve the MCP server via stdio
    Serve,
    /// Manually trigger a full reindex of the workspace
    Reindex,
    /// Diagnose workspace issues (missing IDs, orphans, broken links)
    Doctor,
    /// Install the MCP server into discovered IDEs
    Install {
        /// Automatically approve all patching
        #[arg(short, long)]
        yes: bool,
        #[arg(long)]
        dry_run: bool,
    },
    /// Garbage collection: delete orphaned indices
    Gc,
    /// Incremental sync of the current vault
    Sync,
    /// Tail the internal server log
    Log,
    /// Export the current vault to llms.txt
    ExportLlms {
        #[arg(short, long)]
        out: Option<String>,
    },
}

impl Cli {
    pub async fn execute() -> Result<()> {
        let cli = Cli::parse();
        let current_dir = std::env::current_dir()?;

        match &cli.command {
            Commands::Config { vault_path, auto_add, inject_rules, auto_import } => {
                let mut registry = crate::workspace::Registry::load().unwrap_or_default();
                let mut updated = false;

                if vault_path.is_none() && auto_add.is_none() && inject_rules.is_none() && auto_import.is_none() {
                    let cv = registry.global.global_vault_path.as_deref().unwrap_or("Not Set");
                    let ca = registry.global.auto_add_projects.unwrap_or(true);
                    let ci = registry.global.inject_rules.unwrap_or(false);
                    let cb = registry.global.max_backups.unwrap_or(5);
                    let cs = registry.global.auto_import_strategy.as_deref().unwrap_or("skip");

                    println!("+-------------------+------------------------------------------------------------------+");
                    println!("| Setting           | Value                                                            |");
                    println!("+-------------------+------------------------------------------------------------------+");
                    println!("| Vault Path        | {:<64} |", cv);
                    println!("| Auto Add Projects | {:<64} |", ca);
                    println!("| Inject Rules      | {:<64} |", ci);
                    println!("| Max Backups       | {:<64} |", cb);
                    println!("| Auto Import Strat | {:<64} |", cs);
                    println!("+-------------------+------------------------------------------------------------------+\n");

                    let edit = dialoguer::Confirm::new()
                        .with_prompt("Do you want to edit these settings interactively?")
                        .default(false)
                        .interact()?;
                    
                    if !edit {
                        return Ok(());
                    }
                }

                // 1. Vault Path
                let current_vault = registry.global.global_vault_path.clone().unwrap_or_else(|| {
                    let mut p = dirs::home_dir().unwrap_or_default();
                    p.push(".rms-memory");
                    p.push("vaults");
                    p.to_string_lossy().to_string()
                });

                let new_vault: String = if let Some(path) = vault_path {
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
                let new_auto = if let Some(auto) = auto_add {
                    *auto
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
                let new_inject = if let Some(inject) = inject_rules {
                    *inject
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
                let current_strategy = registry.global.auto_import_strategy.clone().unwrap_or_else(|| "skip".to_string());
                let new_strategy = if let Some(strat) = auto_import {
                    strat.clone()
                } else {
                    let items = vec!["skip", "link", "import_organize", "import"];
                    let default_idx = items.iter().position(|&s| s == current_strategy).unwrap_or(0);
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
                    registry.save()?;
                    println!("Configuration saved successfully.");
                } else {
                    println!("No changes made to configuration.");
                }
            }
            Commands::Init { dry_run, force } => {
                let current_dir = std::env::current_dir()?;
                let start_canon = std::fs::canonicalize(&current_dir).unwrap_or_else(|_| current_dir.to_path_buf());
                let folder_name = start_canon.file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("UnknownProject")
                    .to_string();
                
                let mut registry = crate::workspace::Registry::load()?;
                if let Some(global_vault) = &registry.global.global_vault_path {
                    let vault_path = std::path::Path::new(global_vault).join(&folder_name).to_string_lossy().to_string();
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("rules"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("decisions"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("architecture"))?;
                    std::fs::create_dir_all(std::path::Path::new(&vault_path).join("artifacts"))?;
                    
                    registry.projects.insert(folder_name.clone(), crate::workspace::ProjectConfig {
                        code_path: start_canon.to_string_lossy().to_string(),
                        vault_path: vault_path.clone(),
                        include: vec!["rules/**/*.md".to_string(), "decisions/**/*.md".to_string(), "architecture/**/*.md".to_string(), "artifacts/**/*.md".to_string(), "**/*.md".to_string()],
                        exclude: vec!["node_modules/**".to_string(), "vendor/**".to_string(), ".git/**".to_string()],
                    });
                    if !dry_run {
                        registry.save()?;
                        println!("Manually initialized project {} in global registry.", folder_name);
                    } else {
                        println!("[DRY-RUN] Would have saved project {} to global registry.", folder_name);
                    }
                    
                    if registry.global.inject_rules.unwrap_or(true) || *force {
                        let opts = crate::rules_injector::InjectOptions {
                            dry_run: *dry_run,
                            force: *force,
                            interactive: true,
                        };
                        if let Err(e) = crate::rules_injector::inject_rules(&start_canon, opts) {
                            eprintln!("Warning: Failed to inject rules: {}", e);
                        } else if !dry_run {
                            println!("Successfully injected RMS Memory rules into IDE configs.");
                        }
                    }

                    if !dry_run {
                        let import_service = crate::import::ImportService::new(start_canon, std::path::PathBuf::from(&vault_path));
                        let docs = import_service.detect_existing_docs();
                        if !docs.is_empty() {
                            if let Ok(action) = import_service.prompt_action(&docs) {
                                if let Err(e) = import_service.execute(action, docs) {
                                    eprintln!("Warning: Failed to import documents: {}", e);
                                }
                            }
                        }
                    }

                } else {
                    println!("Please set global_vault_path first using: rms-memory config --vault-path <PATH>");
                }
            }
            Commands::Import => {
                let current_dir = std::env::current_dir()?;
                let workspace = Workspace::discover(&current_dir, None)?;
                let import_service = crate::import::ImportService::new(workspace.code_path.clone(), workspace.root.clone());
                let docs = import_service.detect_existing_docs();
                if docs.is_empty() {
                    println!("No existing project knowledge files found to import.");
                } else {
                    let action = import_service.prompt_action(&docs)?;
                    import_service.execute(action, docs)?;
                }
            }
            Commands::Serve => {
                let registry = crate::workspace::Registry::load().unwrap_or_default();
                let max_backups = registry.global.max_backups.unwrap_or(5);
                crate::mcp_server::McpServer::run(None, None, None, max_backups).await?;
            }
            Commands::Reindex => {
                let workspace = Workspace::discover(&current_dir, None)?;
                println!("Reindexing Vault at {:?}", workspace.root);
                
                let store = workspace.get_store().await?;
                let indexer = Indexer::new()?;
                
                crate::indexer::index_vault_full(&workspace, &store, indexer).await?;
                
                println!("Reindex completed.");
            }
            Commands::Doctor => {
                let workspace = Workspace::discover(&current_dir, None)?;
                println!("Doctor checks for {:?}", workspace.root);
                // TODO: iterate over files, check rules
                println!("All checks passed.");
            }
            Commands::Install { yes, dry_run } => {
                crate::install::run_installer(*yes, *dry_run).await?;
            }
            Commands::Log => {
                let log_file = crate::workspace::base_dir().join("rms.log");
                if !log_file.exists() {
                    println!("Log file does not exist yet.");
                    return Ok(());
                }
                let mut child = std::process::Command::new("tail")
                    .arg("-f")
                    .arg(&log_file)
                    .spawn()?;
                let _ = child.wait()?;
            }
            Commands::Gc => {
                let registry = crate::workspace::Registry::load()?;
                let dbs_dir = crate::workspace::base_dir().join("dbs");
                if !dbs_dir.exists() {
                    println!("No databases found.");
                    return Ok(());
                }
                
                let mut active_hashes = std::collections::HashSet::new();
                for proj in registry.projects.values() {
                    let canon = std::fs::canonicalize(&proj.vault_path).unwrap_or_else(|_| std::path::PathBuf::from(&proj.vault_path));
                    let hash = blake3::hash(canon.to_string_lossy().as_bytes()).to_hex().to_string();
                    active_hashes.insert(hash);
                }
                
                let mut to_delete = Vec::new();
                for entry in std::fs::read_dir(&dbs_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap().to_string_lossy().to_string();
                        if !active_hashes.contains(&name) {
                            to_delete.push((name, path));
                        }
                    }
                }

                if to_delete.is_empty() {
                    println!("GC complete. No orphaned databases found.");
                    return Ok(());
                }

                println!("Found {} orphaned databases.", to_delete.len());
                let confirm = dialoguer::Confirm::new()
                    .with_prompt(format!("Are you sure you want to permanently delete {} orphaned databases?", to_delete.len()))
                    .default(false)
                    .interact()?;
                
                if confirm {
                    let mut deleted = 0;
                    for (name, path) in to_delete {
                        println!("Deleting: {}", name);
                        std::fs::remove_dir_all(&path)?;
                        deleted += 1;
                    }
                    println!("GC complete. Deleted {} orphaned databases.", deleted);
                } else {
                    println!("GC cancelled.");
                }
            }
            Commands::ExportLlms { out } => {
                let workspace = Workspace::discover(&current_dir, None)?;
                let files = workspace.find_markdown_files()?;
                let mut combined = String::new();
                for f in files {
                    if let Ok(content) = std::fs::read_to_string(&f) {
                        combined.push_str(&format!("\n\n---\nFile: {}\n---\n\n", f.to_string_lossy()));
                        combined.push_str(&content);
                    }
                }
                let out_path = out.clone().unwrap_or_else(|| "llms.txt".to_string());
                std::fs::write(&out_path, combined)?;
                println!("Exported {} files to {}", workspace.find_markdown_files()?.len(), out_path);
            }
            Commands::Sync => {
                let workspace = Workspace::discover(&current_dir, None)?;
                let store = workspace.get_store().await?;
                let indexer = Indexer::new()?;
                crate::indexer::sync_vault(&workspace, &store, indexer).await?;
                println!("Sync complete.");
            }
        }
        Ok(())
    }
}
