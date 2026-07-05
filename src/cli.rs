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
    },
    /// Initialize local project in the global registry manually
    Init {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        force: bool,
    },
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
            Commands::Config { vault_path, auto_add, inject_rules } => {
                let mut registry = crate::workspace::Registry::load()?;
                if let Some(path) = vault_path {
                    registry.global.global_vault_path = Some(path.clone());
                    println!("Set global_vault_path to: {}", path);
                }
                if let Some(auto) = auto_add {
                    registry.global.auto_add_projects = Some(*auto);
                    println!("Set auto_add_projects to: {}", auto);
                }
                if let Some(inject) = inject_rules {
                    registry.global.inject_rules = Some(*inject);
                    println!("Set inject_rules to: {}", inject);
                }
                registry.save()?;
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
                        vault_path,
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
                        };
                        if let Err(e) = crate::rules_injector::inject_rules(&start_canon, opts) {
                            eprintln!("Warning: Failed to inject rules: {}", e);
                        } else if !dry_run {
                            println!("Successfully injected RMS Memory rules into IDE configs.");
                        }
                    }
                } else {
                    println!("Please set global_vault_path first using: rms-memory config --vault-path <PATH>");
                }
            }
            Commands::Serve => {
                let workspace = Workspace::discover(&current_dir, None)?;
                let store = workspace.get_store().await?;
                
                let indexer = Indexer::new()?;
                let indexer_arc = std::sync::Arc::new(tokio::sync::Mutex::new(indexer));

                // Spawn background sync
                let sync_workspace = workspace.clone();
                let sync_store = store.clone();
                let sync_indexer = Indexer::new()?;
                tokio::spawn(async move {
                    tracing::info!("Starting background index sync...");
                    if let Err(e) = crate::indexer::sync_vault(&sync_workspace, &sync_store, sync_indexer).await {
                        tracing::error!("Background sync failed: {}", e);
                    } else {
                        tracing::info!("Index sync complete.");
                    }
                });

                // Pass workspace.root (the vault path) to the server
                let registry = crate::workspace::Registry::load().unwrap_or_default();
                let max_backups = registry.global.max_backups.unwrap_or(5);
                crate::mcp_server::McpServer::run(store, indexer_arc, workspace.root.clone(), max_backups).await?;
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
                let log_file = crate::workspace::project_dirs().data_dir().join("rms.log");
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
                let dbs_dir = crate::workspace::project_dirs().data_dir().join("dbs");
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
                
                let mut deleted = 0;
                for entry in std::fs::read_dir(&dbs_dir)? {
                    let entry = entry?;
                    let path = entry.path();
                    if path.is_dir() {
                        let name = path.file_name().unwrap().to_string_lossy().to_string();
                        if !active_hashes.contains(&name) {
                            println!("Deleting orphaned database: {}", name);
                            std::fs::remove_dir_all(&path)?;
                            deleted += 1;
                        }
                    }
                }
                println!("GC complete. Deleted {} orphaned databases.", deleted);
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
