use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

#[derive(Deserialize)]
struct RpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

#[derive(Serialize)]
struct RpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}

#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}

use crate::indexer::Indexer;
use crate::tools::AppContext;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct McpServer {
    ctx: AppContext,
}

fn spawn_sync_watcher(workspace: crate::workspace::Workspace, store: crate::store::Store) {
    tokio::spawn(async move {
        // Initial sync
        if let Ok(sync_indexer) = tokio::task::spawn_blocking(crate::indexer::Indexer::new)
            .await
            .unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed")))
        {
            let _ = crate::indexer::sync_vault(&workspace, &store, sync_indexer).await;
        }

        // File Watcher
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let mut watcher = match notify::RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && matches!(
                        event.kind,
                        notify::EventKind::Modify(_)
                            | notify::EventKind::Create(_)
                            | notify::EventKind::Remove(_)
                    )
                {
                    let mut should_trigger = false;
                    for path in &event.paths {
                        let p = path.to_string_lossy();
                        if !p.contains(".lancedb")
                            && !p.ends_with("store.json")
                            && !p.ends_with(".log")
                        {
                            should_trigger = true;
                            break;
                        }
                    }
                    if should_trigger {
                        let _ = tx.blocking_send(());
                    }
                }
            },
            notify::Config::default(),
        ) {
            Ok(w) => w,
            Err(e) => {
                tracing::error!("Failed to create watcher: {}", e);
                return;
            }
        };

        use notify::Watcher;
        if let Err(e) = watcher.watch(&workspace.root, notify::RecursiveMode::Recursive) {
            tracing::error!("Failed to watch workspace: {}", e);
            return;
        }

        let debounce_timer = tokio::time::sleep(tokio::time::Duration::from_secs(3));
        tokio::pin!(debounce_timer);

        let mut pending_sync = false;

        loop {
            tokio::select! {
                recv = rx.recv() => {
                    if recv.is_none() { break; } // channel closed
                    debounce_timer.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_secs(3));
                    pending_sync = true;
                }
                _ = &mut debounce_timer, if pending_sync => {
                    pending_sync = false;
                    if let Ok(sync_indexer) = tokio::task::spawn_blocking(crate::indexer::Indexer::new).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
                        let _ = crate::indexer::sync_vault(&workspace, &store, sync_indexer).await;
                    }
                }
            }
        }

        drop(watcher);
    });
}

impl McpServer {
    pub async fn run(
        store: Option<crate::store::Store>,
        indexer: Option<Arc<Mutex<Indexer>>>,
        workspace_root: Option<std::path::PathBuf>,
        max_backups: usize,
    ) -> Result<()> {
        let mut server = Self {
            ctx: AppContext {
                store,
                indexer,
                workspace_root,
                max_backups,
            },
        };
        let stdin = io::stdin();
        let mut stdout = io::stdout();
        let mut reader = stdin.lock();
        let mut line = String::new();

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
                break; // EOF
            }

            let req: Result<RpcRequest, _> = serde_json::from_str(&line);
            match req {
                Ok(request) => {
                    if let Some(id) = request.id {
                        let response = server.handle_request(&request.method, request.params).await;
                        let rpc_res = match response {
                            Ok(res) => RpcResponse {
                                jsonrpc: "2.0".to_string(),
                                id,
                                result: Some(res),
                                error: None,
                            },
                            Err(e) => RpcResponse {
                                jsonrpc: "2.0".to_string(),
                                id,
                                result: None,
                                error: Some(RpcError {
                                    code: -32603,
                                    message: e.to_string(),
                                }),
                            },
                        };
                        let mut res_str = serde_json::to_string(&rpc_res)?;
                        res_str.push('\n');
                        stdout.write_all(res_str.as_bytes())?;
                        stdout.flush()?;
                    } else {
                        // Notification (no id)
                        if request.method == "notifications/initialized" {
                            // ignore
                        }
                    }
                }
                Err(e) => {
                    tracing::error!("Failed to parse JSON-RPC: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn handle_request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "initialize" => {
                let mut path = None;
                if let Some(params_obj) = params.as_ref().and_then(|p| p.as_object())
                    && let Some(root_uri) = params_obj.get("rootUri").and_then(|v| v.as_str())
                {
                    let path_str = if let Some(stripped) = root_uri.strip_prefix("file://") {
                        stripped
                    } else {
                        root_uri
                    };
                    if path_str != "/" && !path_str.is_empty() {
                        path = Some(std::path::PathBuf::from(path_str));
                    }
                }

                // Fallback to current working directory if rootUri is missing or "/"
                let path = path.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"))
                });

                if let Ok(workspace) = crate::workspace::Workspace::discover(&path, None) {
                    self.ctx.workspace_root = Some(workspace.root.clone());

                    match workspace.get_store().await {
                        Ok(store) => {
                            spawn_sync_watcher(workspace.clone(), store.clone());
                            self.ctx.store = Some(store);
                        }
                        Err(_e) => {}
                    }
                } else {
                    // Fallback: use global vault when no project is registered for this path
                    tracing::warn!(
                        "No project registered for path: {:?}. Trying global vault fallback.",
                        path
                    );
                    if let Ok(registry) = crate::workspace::Registry::load()
                        && let Some(global_vault) = &registry.global.global_vault_path
                    {
                        let vault_path = std::path::PathBuf::from(global_vault);
                        if vault_path.exists() {
                            self.ctx.workspace_root = Some(vault_path.clone());
                            let workspace = crate::workspace::Workspace {
                                root: vault_path,
                                code_path: path.clone(),
                                include: vec!["**/*.md".to_string()],
                                exclude: vec!["node_modules/**".to_string(), ".git/**".to_string()],
                            };
                            if let Ok(store) = workspace.get_store().await {
                                spawn_sync_watcher(workspace.clone(), store.clone());
                                self.ctx.store = Some(store);
                                tracing::info!("Initialized with global vault fallback.");
                            }
                        }
                    }
                }

                Ok(json!({
                    "protocolVersion": "2024-11-05",
                    "serverInfo": {
                        "name": "rms-memory",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "capabilities": {
                        "tools": {}
                    }

                }))
            }
            "tools/list" => Ok(json!({
                "tools": [
                    {
                        "name": "rms_search",
                        "description": "Search the local RMS Memory vector database (LanceDB) for project documentation, architectural decisions, and context rules using semantic similarity. Use this tool FIRST to understand the repository's background, past decisions, or rules before making changes. Provide a detailed semantic query.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The semantic query string to search for." },
                                "limit": { "type": "integer", "description": "Maximum number of chunks to return. Default is 10." },
                                "include_content": { "type": "boolean", "description": "Whether to include full chunk text in results." }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "rms_read",
                        "description": "Read the full contents of a markdown document from the RMS Memory vault. Provide the relative path (e.g., 'rules/api.md'). Use this to retrieve the full context of a document found via rms_search.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "path": { "type": "string", "description": "Relative path to the markdown document in the vault." }
                            },
                            "required": ["path"]
                        }
                    },
                    {
                        "name": "rms_write",
                        "description": "Save new architectural decisions, constraints, development rules, or project context to the RMS Memory vault. Use this tool PROACTIVELY at the end of a task if you learned a new user preference, solved a tricky bug, or made a new architectural decision.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "id": { "type": "string" },
                                "path": { "type": "string", "description": "Relative path to save the document (e.g., 'decisions/001-db.md')." },
                                "content": { "type": "string", "description": "The markdown content to write." },
                                "mode": { "type": "string", "enum": ["create", "append", "replace"], "description": "Write mode." }
                            },
                            "required": ["path", "mode", "content"]
                        }
                    }
                ]
            })),
            "tools/call" => {
                let params = params.unwrap_or(json!({}));
                let name = params["name"].as_str().unwrap_or("");
                let args = params["arguments"].as_object().cloned().unwrap_or_default();

                match name {
                    "rms_search" => crate::tools::search::execute(&mut self.ctx, &args).await,
                    "rms_read" => crate::tools::read::execute(&self.ctx, &args).await,
                    "rms_write" => crate::tools::write::execute(&self.ctx, &args).await,
                    _ => anyhow::bail!("Unknown tool"),
                }
            }
            _ => anyhow::bail!("Method not found"),
        }
    }
}
