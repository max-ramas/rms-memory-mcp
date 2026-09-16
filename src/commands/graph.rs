use anyhow::Result;
use clap::{Args, Subcommand};

#[derive(Debug, Args)]
pub struct GraphArgs {
    #[command(subcommand)]
    pub command: GraphCommands,
}

#[derive(Debug, Subcommand)]
pub enum GraphCommands {
    /// Show node/edge counts
    Status {
        #[arg(long)]
        project: Option<String>,
    },
    /// Reconcile vault Markdown links into the durable graph when empty (or --force)
    Ensure {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
    /// List neighbors for a node key or vault path
    Neighbors {
        #[arg(long)]
        project: Option<String>,
        /// Node key (vault:… / code:…) or vault-relative path
        #[arg(long)]
        node: String,
        #[arg(long, default_value_t = 16)]
        limit: usize,
    },
    /// Shortest undirected path between two nodes/paths
    Path {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        from: String,
        #[arg(long)]
        to: String,
        #[arg(long, default_value_t = 8)]
        max_depth: usize,
    },
    /// Dump a bounded graph snapshot as JSON
    Snapshot {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 500)]
        limit: usize,
    },
    /// Export Graphviz DOT
    ExportDot {
        #[arg(long)]
        project: Option<String>,
        #[arg(long, default_value_t = 2000)]
        limit: usize,
    },
    /// Create a user edge (requires --project)
    CreateEdge {
        #[arg(long)]
        project: String,
        #[arg(long)]
        source: String,
        #[arg(long)]
        target: String,
        #[arg(long, default_value = "links_to")]
        relation: String,
    },
    /// Suppress or restore an edge (requires --project)
    EdgeOverride {
        #[arg(long)]
        project: String,
        #[arg(long)]
        edge_key: String,
        #[arg(long, default_value = "suppress")]
        override_action: String,
        #[arg(long, default_value_t = 0)]
        expected_revision: u64,
    },
}

impl GraphArgs {
    pub async fn run(&self, _scope: Option<String>) -> Result<()> {
        match &self.command {
            GraphCommands::Status { project } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({ "action": "status" }),
                )
                .await
            }
            GraphCommands::Ensure { project, force } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({ "action": "ensure", "force": force }),
                )
                .await
            }
            GraphCommands::Neighbors {
                project,
                node,
                limit,
            } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({
                        "action": "neighbors",
                        "node": node,
                        "limit": limit,
                    }),
                )
                .await
            }
            GraphCommands::Path {
                project,
                from,
                to,
                max_depth,
            } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({
                        "action": "path",
                        "from": from,
                        "to": to,
                        "max_depth": max_depth,
                    }),
                )
                .await
            }
            GraphCommands::Snapshot { project, limit } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({ "action": "snapshot", "limit": limit }),
                )
                .await
            }
            GraphCommands::ExportDot { project, limit } => {
                print_tool(
                    project.as_deref(),
                    serde_json::json!({ "action": "export_dot", "limit": limit }),
                )
                .await
            }
            GraphCommands::CreateEdge {
                project,
                source,
                target,
                relation,
            } => {
                print_tool(
                    Some(project.as_str()),
                    serde_json::json!({
                        "action": "create_edge",
                        "project": project,
                        "source": source,
                        "target": target,
                        "relation": relation,
                    }),
                )
                .await
            }
            GraphCommands::EdgeOverride {
                project,
                edge_key,
                override_action,
                expected_revision,
            } => {
                print_tool(
                    Some(project.as_str()),
                    serde_json::json!({
                        "action": "edge_override",
                        "project": project,
                        "edge_key": edge_key,
                        "override_action": override_action,
                        "expected_revision": expected_revision,
                    }),
                )
                .await
            }
        }
    }
}

async fn print_tool(project: Option<&str>, mut args: serde_json::Value) -> Result<()> {
    if let Some(key) = project
        && let Some(obj) = args.as_object_mut()
    {
        obj.entry("project".to_string())
            .or_insert_with(|| serde_json::json!(key));
    }
    let map = args
        .as_object()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("internal: graph args must be an object"))?;
    let (workspace, store) = open_store(project).await?;
    let ctx = crate::tools::AppContext {
        store: Some(store),
        indexer: Some(std::sync::Arc::new(tokio::sync::Mutex::new(
            crate::indexer::Indexer::new()?,
        ))),
        workspace_root: Some(workspace.root.clone()),
        code_path: Some(workspace.code_path.clone()),
        max_backups: 0,
        scope: None,
        caller_id: "cli".into(),
        project_key: project
            .map(str::to_string)
            .or_else(|| workspace.project_key()),
    };
    let response = crate::tools::graph::execute(&ctx, &map).await?;
    if let Some(text) = response.pointer("/content/0/text").and_then(|v| v.as_str()) {
        println!("{text}");
    } else if let Some(structured) = response.get("structuredContent") {
        println!("{}", serde_json::to_string_pretty(structured)?);
    } else {
        println!("{}", serde_json::to_string_pretty(&response)?);
    }
    Ok(())
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
