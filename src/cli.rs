use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rms-memory", version = env!("CARGO_PKG_VERSION"), about = "RMS Memory MCP Server")]
pub struct Cli {
    /// Override the scope identifier (path, thread ID, project name, etc.)
    #[arg(long, short = 's', global = true)]
    pub scope: Option<String>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Manage global configuration
    Config(crate::commands::config::ConfigArgs),
    /// Initialize local project in the global registry manually
    Init(crate::commands::init::InitArgs),
    /// Import existing documentation into the Vault
    Import(crate::commands::simple::ImportArgs),
    /// Serve the MCP server via stdio
    Serve(crate::commands::simple::ServeArgs),
    /// Manually trigger a full reindex of the workspace
    Reindex(crate::commands::simple::ReindexArgs),
    /// Diagnose workspace issues (missing IDs, orphans, broken links)
    Doctor(crate::commands::simple::DoctorArgs),
    /// Install the MCP server into discovered IDEs
    Install(crate::commands::simple::InstallArgs),
    /// Uninstall the MCP server from discovered IDEs
    Uninstall(crate::commands::simple::UninstallArgs),
    /// Garbage collection: delete orphaned indices
    Gc(crate::commands::gc::GcArgs),
    /// Incremental sync of the current vault
    Sync(crate::commands::simple::SyncArgs),
    /// Tail the internal server log
    Log(crate::commands::simple::LogArgs),
    /// Export the current vault to llms.txt
    ExportLlms(crate::commands::simple::ExportLlmsArgs),
    /// Generate wiki context packs from vault and code index
    Wiki {
        #[command(subcommand)]
        command: crate::commands::wiki::WikiCommands,
    },
    /// List and locate registered projects
    Projects {
        #[command(subcommand)]
        command: crate::commands::projects::ProjectsCommands,
    },
}

impl Cli {
    pub async fn execute() -> Result<()> {
        let cli = Cli::parse();
        let scope = cli.scope.clone();
        match &cli.command {
            Commands::Config(args) => args.run(scope).await,
            Commands::Init(args) => args.run(scope).await,
            Commands::Import(args) => args.run(scope).await,
            Commands::Serve(args) => args.run(scope).await,
            Commands::Reindex(args) => args.run(scope).await,
            Commands::Doctor(args) => args.run(scope).await,
            Commands::Install(args) => args.run(scope).await,
            Commands::Uninstall(args) => args.run(scope).await,
            Commands::Gc(args) => args.run(scope).await,
            Commands::Sync(args) => args.run(scope).await,
            Commands::Log(args) => args.run(scope).await,
            Commands::ExportLlms(args) => args.run(scope).await,
            Commands::Wiki { command } => command.run(scope).await,
            Commands::Projects { command } => Ok(command.run()?),
        }
    }
}
