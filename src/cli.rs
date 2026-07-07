use crate::commands::CommandRunner;
use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "rms-memory", version = env!("CARGO_PKG_VERSION"), about = "RMS Memory MCP Server")]
pub struct Cli {
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
    /// Garbage collection: delete orphaned indices
    Gc(crate::commands::gc::GcArgs),
    /// Incremental sync of the current vault
    Sync(crate::commands::simple::SyncArgs),
    /// Tail the internal server log
    Log(crate::commands::simple::LogArgs),
    /// Export the current vault to llms.txt
    ExportLlms(crate::commands::simple::ExportLlmsArgs),
}

impl Cli {
    pub async fn execute() -> Result<()> {
        let cli = Cli::parse();
        match &cli.command {
            Commands::Config(args) => args.run().await,
            Commands::Init(args) => args.run().await,
            Commands::Import(args) => args.run().await,
            Commands::Serve(args) => args.run().await,
            Commands::Reindex(args) => args.run().await,
            Commands::Doctor(args) => args.run().await,
            Commands::Install(args) => args.run().await,
            Commands::Gc(args) => args.run().await,
            Commands::Sync(args) => args.run().await,
            Commands::Log(args) => args.run().await,
            Commands::ExportLlms(args) => args.run().await,
        }
    }
}
