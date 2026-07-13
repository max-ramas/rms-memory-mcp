use crate::indexer::Indexer;
use crate::retrieval::RetrievalService;
use crate::wiki::WikiService;
use crate::workspace::Workspace;
use anyhow::Result;
use clap::{Args, Subcommand};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Subcommand, Debug)]
pub enum WikiCommands {
    Generate(GenerateArgs),
    Init(InitArgs),
    Clean,
}

#[derive(Args, Debug)]
pub struct GenerateArgs {
    #[arg(long)]
    pub manifest: Option<String>,
    #[arg(long)]
    pub refresh_code: bool,
    #[arg(long)]
    pub stdout: bool,
    #[arg(long, short = 's')]
    pub scope: Option<String>,
}

#[derive(Args, Debug)]
pub struct InitArgs {
    #[arg(short, long, default_value = "wiki.yaml")]
    pub out: String,
    #[arg(long)]
    pub force: bool,
}

impl WikiCommands {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        match self {
            WikiCommands::Generate(args) => generate(args, scope).await,
            WikiCommands::Init(args) => init_manifest(args).await,
            WikiCommands::Clean => clean(scope).await,
        }
    }
}

async fn resolve_service(scope: Option<String>) -> Result<(WikiService, std::path::PathBuf)> {
    let current_dir = std::env::current_dir()?;
    let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
    let store = Arc::new(workspace.get_store().await?);
    let indexer = Arc::new(Mutex::new(Indexer::new()?));
    let retrieval = RetrievalService::new(store, indexer);
    let wiki_root = workspace.root.join("wiki");
    let service = WikiService::new(
        retrieval,
        workspace.root.clone(),
        scope.unwrap_or_else(|| workspace.canonical_path().unwrap_or_default()),
    );
    Ok((service, wiki_root))
}

async fn generate(args: &GenerateArgs, scope: Option<String>) -> Result<()> {
    let (service, _) = resolve_service(scope).await?;

    let manifest = if let Some(path) = &args.manifest {
        crate::wiki::WikiManifest::from_file(std::path::Path::new(path))?
    } else {
        service.manifest_init("")
    };

    let request = crate::wiki::WikiGenerateRequest {
        manifest,
        refresh_code: args.refresh_code,
    };

    let result = service.generate(request).await?;

    if args.stdout {
        let content = std::fs::read_to_string(&result.context_pack_path)?;
        println!("{content}");
    } else {
        println!("✅ Wiki context pack generated");
        println!("   Pack ID:      {}", result.pack_id);
        println!("   Context pack: {}", result.context_pack_path.display());
        println!("   Agent task:   {}", result.agent_task_path.display());
        println!("   Sources:      {}", result.sources_path.display());
        println!("   Wiki root:    {}", result.wiki_root.display());
        println!("   Total chars:  {}", result.total_chars);
        println!("   Sections:     {}", result.sections_generated);
    }

    Ok(())
}

async fn init_manifest(args: &InitArgs) -> Result<()> {
    let path = std::path::Path::new(&args.out);
    if path.exists() && !args.force {
        anyhow::bail!(
            "File already exists: {}. Use --force to overwrite.",
            args.out
        );
    }
    let manifest = crate::wiki::WikiManifest::default_manifest();
    std::fs::write(path, manifest.to_yaml()?)?;
    println!("✅ Wiki manifest template created at: {}", path.display());
    Ok(())
}

async fn clean(scope: Option<String>) -> Result<()> {
    let (service, _) = resolve_service(scope).await?;
    service.clean().await?;
    println!("✅ Wiki generation artifacts cleaned");
    Ok(())
}
