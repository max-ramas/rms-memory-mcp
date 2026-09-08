use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct FileHistoryArgs {
    #[command(subcommand)]
    pub command: FileHistoryCommands,
}

#[derive(clap::Subcommand, Debug)]
pub enum FileHistoryCommands {
    /// Catch up the derived file→commit cache to HEAD (no-op if already current)
    CatchUp {
        /// Registered project key (fail-closed when omitted and cwd is ambiguous)
        #[arg(long)]
        project: Option<String>,
    },
    /// Wipe and rebuild the file→commit cache from full git history
    Reindex {
        /// Required — full rebuild is destructive to the derived cache
        #[arg(long)]
        project: String,
    },
    /// Query recent commits for a path (runs catch-up first)
    Query {
        #[arg(long)]
        project: Option<String>,
        /// Path relative to the project code_path
        path: String,
        #[arg(long, default_value_t = 20)]
        limit: usize,
        #[arg(long, default_value_t = false)]
        include_message: bool,
    },
}

impl FileHistoryArgs {
    pub async fn run(&self, scope: Option<String>) -> Result<()> {
        let _ = scope;
        match &self.command {
            FileHistoryCommands::CatchUp { project } => {
                let (workspace, store) = open_store(project.as_deref()).await?;
                let report = rms_memory_index::file_history::catch_up_file_history(
                    &store,
                    &workspace.code_path,
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "catch_up",
                        "noop": report.noop,
                        "from_sha": report.from_sha,
                        "to_sha": report.to_sha,
                        "commits_scanned": report.commits_scanned,
                        "rows_upserted": report.rows_upserted,
                    }))?
                );
            }
            FileHistoryCommands::Reindex { project } => {
                let (workspace, store) = open_store(Some(project.as_str())).await?;
                let report = rms_memory_index::file_history::reindex_file_history(
                    &store,
                    &workspace.code_path,
                )
                .await?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "action": "reindex",
                        "from_sha": report.from_sha,
                        "to_sha": report.to_sha,
                        "commits_scanned": report.commits_scanned,
                        "rows_upserted": report.rows_upserted,
                    }))?
                );
            }
            FileHistoryCommands::Query {
                project,
                path,
                limit,
                include_message,
            } => {
                let (workspace, store) = open_store(project.as_deref()).await?;
                let _ = rms_memory_index::file_history::catch_up_file_history(
                    &store,
                    &workspace.code_path,
                )
                .await?;
                let rows = store
                    .query_file_history(path, (*limit).clamp(1, 200))
                    .await?;
                let commits: Vec<_> = rows
                    .iter()
                    .map(|row| {
                        let mut m = serde_json::Map::new();
                        m.insert("sha".into(), serde_json::json!(row.commit_sha));
                        m.insert("author".into(), serde_json::json!(row.author));
                        m.insert("committed_at".into(), serde_json::json!(row.committed_at));
                        if *include_message {
                            m.insert("message".into(), serde_json::json!(row.message));
                        }
                        serde_json::Value::Object(m)
                    })
                    .collect();
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "path": path,
                        "commits": commits,
                    }))?
                );
            }
        }
        Ok(())
    }
}

async fn open_store(
    project: Option<&str>,
) -> Result<(
    rms_memory_core::workspace::Workspace,
    rms_memory_index::store::Store,
)> {
    let workspace = if let Some(key) = project {
        let registry = rms_memory_core::workspace::Registry::load()?;
        let (_, config) = registry
            .locate_by_project_key(key)
            .ok_or_else(|| anyhow::anyhow!("Unknown project key '{key}'"))?;
        rms_memory_core::workspace::Workspace::discover(
            std::path::Path::new(&config.code_path),
            None,
        )?
    } else {
        let cwd = std::env::current_dir()?;
        rms_memory_core::workspace::Workspace::discover(&cwd, None)?
    };
    let store = rms_memory_index::store::Store::for_workspace(&workspace).await?;
    Ok((workspace, store))
}
