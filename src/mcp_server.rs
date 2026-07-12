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
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

fn spawn_sync_watcher(
    workspace: crate::workspace::Workspace,
    store: crate::store::Store,
    indexer: Arc<Mutex<Indexer>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        // Initial sync
        {
            let mut idx = indexer.lock().await;
            if let Err(e) = crate::indexer::sync_vault(&workspace, &store, &mut idx).await {
                tracing::error!("Initial sync failed: {:#}", e);
            }
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
                            && !p.contains(".bak")
                            && !p.ends_with("store.json")
                            && !p.ends_with(".log")
                        {
                            should_trigger = true;
                            break;
                        }
                    }
                    if should_trigger {
                        let _ = tx.try_send(());
                        tracing::info!(
                            "Watcher triggered by: {}",
                            event
                                .paths
                                .iter()
                                .map(|p| p.to_string_lossy())
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
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
                _ = shutdown_rx.changed() => {
                    tracing::info!("Watcher: shutdown signal received.");
                    break;
                }
                recv = rx.recv() => {
                    if recv.is_none() { break; } // channel closed
                    debounce_timer.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_secs(3));
                    pending_sync = true;
                }
                _ = &mut debounce_timer, if pending_sync => {
                    pending_sync = false;
                    let mut idx = indexer.lock().await;
                    if let Err(e) =
                        crate::indexer::sync_vault(&workspace, &store, &mut idx).await
                    {
                        tracing::error!("Background sync failed: {:#}", e);
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
        scope: Option<String>,
    ) -> Result<()> {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx_for_server = shutdown_tx.clone();
        let shared_indexer = indexer.unwrap_or_else(|| {
            Arc::new(Mutex::new(
                crate::indexer::Indexer::new().expect("Failed to initialize embedding model"),
            ))
        });
        let mut server = Self {
            ctx: AppContext {
                store,
                indexer: Some(shared_indexer.clone()),
                workspace_root,
                max_backups,
                scope,
                caller_id: "unknown".to_string(),
            },
            shutdown_tx: shutdown_tx_for_server,
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
            if line.len() > 1_048_576 {
                tracing::error!("Request exceeds 1MB size limit, rejecting.");
                let err_res = RpcResponse {
                    jsonrpc: "2.0".to_string(),
                    id: serde_json::Value::Null,
                    result: None,
                    error: Some(RpcError {
                        code: -32700,
                        message: "Request too large (max 1MB)".to_string(),
                    }),
                };
                let mut res_str = serde_json::to_string(&err_res)?;
                res_str.push('\n');
                stdout.write_all(res_str.as_bytes())?;
                stdout.flush()?;
                continue;
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
                    let err_res = RpcResponse {
                        jsonrpc: "2.0".to_string(),
                        id: serde_json::Value::Null,
                        result: None,
                        error: Some(RpcError {
                            code: -32700,
                            message: format!("Parse error: {}", e),
                        }),
                    };
                    let mut res_str = serde_json::to_string(&err_res)?;
                    res_str.push('\n');
                    stdout.write_all(res_str.as_bytes())?;
                    stdout.flush()?;
                }
            }
        }
        tracing::info!("Stdin closed (EOF). Shutting down watcher...");
        let _ = server.shutdown_tx.send(true);
        tracing::info!("Server stopped.");
        Ok(())
    }

    async fn handle_request(&mut self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "initialize" => {
                let mut path = None;
                if let Some(params_obj) = params.as_ref().and_then(|p| p.as_object()) {
                    if let Some(client_info) = params_obj.get("clientInfo")
                        && let Some(client_name) = client_info.get("name").and_then(|v| v.as_str())
                    {
                        self.ctx.caller_id = client_name.to_string();
                    }
                    if let Some(root_uri) = params_obj.get("rootUri").and_then(|v| v.as_str()) {
                        let path_str = if let Some(stripped) = root_uri.strip_prefix("file://") {
                            stripped
                        } else {
                            root_uri
                        };
                        if path_str != "/" && !path_str.is_empty() {
                            path = Some(std::path::PathBuf::from(path_str));
                        }
                    }
                }

                // Fallback to current working directory if rootUri is missing or "/"
                let path = path.unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/"))
                });

                if let Ok(workspace) = crate::workspace::Workspace::discover_with_scope(
                    self.ctx.scope.as_deref(),
                    &path,
                    None,
                ) {
                    self.ctx.workspace_root = Some(workspace.root.clone());

                    match workspace.get_store().await {
                        Ok(store) => {
                            spawn_sync_watcher(
                                workspace.clone(),
                                store.clone(),
                                self.ctx.indexer.as_ref().unwrap().clone(),
                                self.shutdown_tx.subscribe(),
                            );
                            self.ctx.store = Some(store);
                        }
                        Err(e) => {
                            tracing::error!("Failed to open LanceDB store for workspace: {:#}", e);
                        }
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
                                spawn_sync_watcher(
                                    workspace.clone(),
                                    store.clone(),
                                    self.ctx.indexer.as_ref().unwrap().clone(),
                                    self.shutdown_tx.subscribe(),
                                );
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
                        "description": "Search the local RMS Memory vector database (LanceDB) for project documentation, architectural decisions, and context rules using semantic similarity. Use this tool FIRST to understand the repository's background, past decisions, or rules before making changes. Provide a detailed semantic query. When using min_confidence: start WITHOUT it (omit the parameter) to see all available results. If you use a high threshold (e.g. 0.9) and get zero results, retry with a lower threshold or omit min_confidence entirely — low-confidence records may still contain useful information.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The semantic query string to search for." },
                                "limit": { "type": "integer", "description": "Maximum number of chunks to return. Default is 10." },
                                "include_content": { "type": "boolean", "description": "Whether to include full chunk text in results." },
                                "min_confidence": { "type": "number", "description": "Optional minimum confidence threshold (0.0–1.0). Records with NULL confidence are always included. CAUTION: do NOT use high values (e.g. 0.9+) unless you need strict filtering. If zero results, retry without this parameter." }
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
                                "mode": { "type": "string", "enum": ["create", "append", "replace"], "description": "Write mode." },
                                "confidence": { "type": "number", "description": "Optional confidence score (0.0–1.0) indicating reliability of this record." },
                                "source": { "type": "string", "description": "Optional free-text citation or source reference for this record." }
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
                    "rms_search" => crate::tools::search::execute(&self.ctx, &args).await,
                    "rms_read" => crate::tools::read::execute(&self.ctx, &args).await,
                    "rms_write" => crate::tools::write::execute(&self.ctx, &args).await,
                    _ => anyhow::bail!("Unknown tool"),
                }
            }
            _ => anyhow::bail!("Method not found"),
        }
    }
}
