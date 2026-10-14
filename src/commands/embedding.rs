use crate::embedding::{self, Backend, CATALOG, OLLAMA_PREFIX};
use crate::workspace::Workspace;
use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Subcommand, Debug)]
pub enum EmbeddingCommands {
    /// List the bundled embedding models and show which one is in use
    List,
    /// Choose the embedding model: a listed id or `ollama:<model>` (local Ollama).
    /// Each model keeps its own index; the next sync builds it.
    Use(UseArgs),
    /// Measure how well models find the right notes in this project's vault
    Eval(EvalArgs),
}

#[derive(Args, Debug)]
pub struct UseArgs {
    /// Model id from `rms-memory embedding list`, or `ollama:<model>`
    pub model: String,
    /// Ollama endpoint on this machine (default http://127.0.0.1:11434)
    #[arg(long)]
    pub ollama_url: Option<String>,
}

#[derive(Args, Debug)]
pub struct EvalArgs {
    /// Comma-separated model ids to compare (default: the model in use)
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,
    /// JSONL file with `{"query": "...", "expect": ["path.md"]}` per line.
    /// Without it, each note's first heading is used as its query.
    #[arg(long)]
    pub queries: Option<PathBuf>,
    /// Save the generated heading queries as JSONL to edit and reuse
    #[arg(long, conflicts_with = "queries")]
    pub write_queries: Option<PathBuf>,
    /// Cut-off for hit@k (1-10; ranks beyond the top 10 are not kept)
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u8).range(1..=10))]
    pub k: u8,
    /// Maximum number of generated queries (0 = one per note)
    #[arg(long, default_value_t = 200)]
    pub max_queries: usize,
    /// List queries whose expected note ranks lower in hybrid search than
    /// with embeddings alone
    #[arg(long)]
    pub show_misses: bool,
    /// Print the report as JSON
    #[arg(long)]
    pub json: bool,
}

impl EmbeddingCommands {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        match self {
            Self::List => list(),
            Self::Use(args) => use_model(args),
            Self::Eval(args) => eval(args, scope).await,
        }
    }
}

fn list() -> Result<()> {
    let active = embedding::active();
    let configured = embedding::configured();
    println!(
        "{:<34} {:>5}  {:<13} {:>9}  Description",
        "Model", "Dims", "Languages", "Download"
    );
    for entry in CATALOG {
        let marker = if entry.id == active.id { "*" } else { " " };
        println!(
            "{marker}{:<33} {:>5}  {:<13} {:>6} MB  {}",
            entry.id,
            entry.dimension,
            if entry.multilingual {
                "multilingual"
            } else {
                "English"
            },
            entry.download_mb,
            entry.description
        );
    }
    if let Backend::Ollama { url, .. } = &active.backend {
        println!(
            "*{:<33} {:>5}  via Ollama at {url}",
            active.id, active.dimension
        );
    }
    println!(
        "\n* in use. Local Ollama models work too: `rms-memory embedding use {OLLAMA_PREFIX}<model>`."
    );
    match configured {
        Ok(profile) if profile.id != active.id => println!(
            "Configured model `{}` takes effect after restarting RMS Memory processes.",
            profile.id
        ),
        Err(error) => println!("Configured model is invalid: {error:#}"),
        _ => {}
    }
    Ok(())
}

fn use_model(args: &UseArgs) -> Result<()> {
    let manager = crate::config_manager::ConfigManager::open()?;
    let snapshot = manager.snapshot();
    let mut registry = snapshot.registry;
    if args.model.trim().starts_with(OLLAMA_PREFIX) {
        println!("Asking Ollama for the model's vector size...");
    }
    let profile = embedding::configure(
        &mut registry.global,
        &args.model,
        args.ollama_url.as_deref(),
    )?;

    manager.replace(snapshot.revision, registry)?;
    println!(
        "Embedding model set to `{}` ({} dimensions).",
        profile.id, profile.dimension
    );
    if std::env::var(embedding::MODEL_ENV).is_ok_and(|id| !id.trim().is_empty()) {
        println!(
            "Note: {} is set in this shell and overrides this choice for programs started from it.",
            embedding::MODEL_ENV
        );
    }
    println!(
        "Restart the MCP server in your IDE and the RMS Memory app. Each project then builds a\n\
         separate index for this model on its next sync; the previous model's index is kept,\n\
         so switching back is instant. Run `rms-memory reindex --code` for code memory."
    );
    Ok(())
}

async fn eval(args: &EvalArgs, scope: Option<String>) -> Result<()> {
    let current_dir = std::env::current_dir()?;
    let workspace = Workspace::discover_with_scope(scope.as_deref(), &current_dir, None)?;
    let global = crate::config_manager::load_registry()?.global;
    let profiles = if args.models.is_empty() {
        vec![embedding::active().clone()]
    } else {
        args.models
            .iter()
            // Probes Ollama models for their vector size; the config is not saved.
            .map(|id| embedding::configure(&mut global.clone(), id, None))
            .collect::<Result<Vec<_>>>()?
    };

    let (cases, source) = match &args.queries {
        Some(path) => (crate::eval::load_cases(path)?, path.display().to_string()),
        None => (
            crate::eval::title_cases(&workspace, args.max_queries)?,
            "note headings".to_string(),
        ),
    };
    if cases.is_empty() {
        bail!(
            "No evaluation queries: the vault has no notes with text, or the queries file is empty"
        );
    }
    if let Some(path) = &args.write_queries {
        let lines = cases
            .iter()
            .map(serde_json::to_string)
            .collect::<Result<Vec<_>, _>>()?;
        std::fs::write(path, lines.join("\n") + "\n")?;
        eprintln!("Wrote {} queries to {}", cases.len(), path.display());
    }

    let scratch = crate::workspace::base_dir().join("cache").join("eval");
    let mut results = Vec::new();
    for profile in &profiles {
        eprintln!("Evaluating {} on {} queries...", profile.id, cases.len());
        results.push(
            crate::eval::evaluate_model(&workspace, profile, &cases, usize::from(args.k), &scratch)
                .await?,
        );
    }

    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "queries": cases.len(),
                "source": source,
                "k": args.k,
                "models": results,
            }))?
        );
        return Ok(());
    }
    println!(
        "\n{} queries from {source}. hit@1 / hit@{k} / MRR, embeddings only and hybrid (as rms_search).\n",
        cases.len(),
        k = args.k
    );
    println!(
        "{:<32} {:>22} {:>22} {:>9} {:>8}",
        "Model", "embeddings", "hybrid", "index", "query"
    );
    for result in &results {
        let triple = |m: &crate::eval::Metrics| {
            format!("{:.2} / {:.2} / {:.2}", m.hit_at_1, m.hit_at_k, m.mrr)
        };
        println!(
            "{:<32} {:>22} {:>22} {:>8.0}s {:>6.0}ms",
            result.model,
            triple(&result.vector),
            triple(&result.hybrid),
            result.index_seconds,
            result.mean_query_ms
        );
    }
    if args.show_misses {
        let rank = |rank: Option<usize>| rank.map_or("-".to_string(), |rank| rank.to_string());
        for result in &results {
            println!("\n{}: rank with embeddings / hybrid", result.model);
            for (case, (vector, hybrid)) in cases.iter().zip(&result.ranks) {
                if hybrid.unwrap_or(usize::MAX) > vector.unwrap_or(usize::MAX) {
                    println!(
                        "  {:>2} / {:<2} {}",
                        rank(*vector),
                        rank(*hybrid),
                        case.query
                    );
                }
            }
        }
    }
    let reused: Vec<&str> = results
        .iter()
        .filter(|result| result.reused_index)
        .map(|result| result.model.as_str())
        .collect();
    if !reused.is_empty() {
        println!(
            "\nExisting project index synced and reused (index time is the sync only): {}",
            reused.join(", ")
        );
    }
    if args.queries.is_none() {
        println!(
            "\nHeading queries are easier than real questions. For a sharper comparison write your own\n\
             queries (--write-queries saves these as a starting point, --queries reads them back)."
        );
    }
    Ok(())
}
