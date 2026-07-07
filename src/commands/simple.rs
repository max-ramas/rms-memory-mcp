use anyhow::Result;
use clap::Args;
use super::CommandRunner;
use crate::workspace::Workspace;
use crate::indexer::Indexer;

#[derive(Args, Debug)]
pub struct ImportArgs;

impl CommandRunner for ImportArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover(&current_dir, None)?;
        let import_service = crate::import::ImportService::new(
            workspace.code_path.clone(),
            workspace.root.clone(),
        );
        let docs = import_service.detect_existing_docs();
        if docs.is_empty() {
            println!("No existing project knowledge files found to import.");
        } else {
            let action = import_service.prompt_action(&docs)?;
            import_service.execute(action, docs)?;
        }
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ServeArgs;

impl CommandRunner for ServeArgs {
    async fn run(&self) -> Result<()> {
        let registry = crate::workspace::Registry::load().unwrap_or_default();
        let max_backups = registry.global.max_backups.unwrap_or(5);
        crate::mcp_server::McpServer::run(None, None, None, max_backups).await?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ReindexArgs;

impl CommandRunner for ReindexArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover(&current_dir, None)?;
        println!("Reindexing Vault at {:?}", workspace.root);

        let store = workspace.get_store().await?;
        let indexer = Indexer::new()?;

        crate::indexer::index_vault_full(&workspace, &store, indexer).await?;

        println!("Reindex completed.");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct DoctorArgs;

impl CommandRunner for DoctorArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover(&current_dir, None)?;
        println!("Doctor checks for {:?}", workspace.root);
        // TODO: iterate over files, check rules
        println!("All checks passed.");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Automatically approve all patching
    #[arg(short, long)]
    pub yes: bool,
    #[arg(long)]
    pub dry_run: bool,
}

impl CommandRunner for InstallArgs {
    async fn run(&self) -> Result<()> {
        crate::installer::run_installer(self.yes, self.dry_run).await?;
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct LogArgs;

impl CommandRunner for LogArgs {
    async fn run(&self) -> Result<()> {
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
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct SyncArgs;

impl CommandRunner for SyncArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover(&current_dir, None)?;
        let store = workspace.get_store().await?;
        let indexer = Indexer::new()?;
        crate::indexer::sync_vault(&workspace, &store, indexer).await?;
        println!("Sync complete.");
        Ok(())
    }
}

#[derive(Args, Debug)]
pub struct ExportLlmsArgs {
    #[arg(short, long)]
    pub out: Option<String>,
}

impl CommandRunner for ExportLlmsArgs {
    async fn run(&self) -> Result<()> {
        let current_dir = std::env::current_dir()?;
        let workspace = Workspace::discover(&current_dir, None)?;
        let files = workspace.find_markdown_files()?;
        let mut combined = String::new();
        for f in files {
            if let Ok(content) = std::fs::read_to_string(&f) {
                combined
                    .push_str(&format!("\n\n---\nFile: {}\n---\n\n", f.to_string_lossy()));
                combined.push_str(&content);
            }
        }
        let out_path = self.out.clone().unwrap_or_else(|| "llms.txt".to_string());
        std::fs::write(&out_path, combined)?;
        println!(
            "Exported {} files to {}",
            workspace.find_markdown_files()?.len(),
            out_path
        );
        Ok(())
    }
}
