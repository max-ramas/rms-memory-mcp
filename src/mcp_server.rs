use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};
use anyhow::Result;

#[derive(Deserialize)]
struct RpcRequest {
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

use tokio::sync::Mutex;
use std::sync::Arc;
use crate::indexer::Indexer;
use crate::store::VectorStore;

pub struct McpServer {
    store: Option<crate::store::Store>,
    indexer: Option<Arc<Mutex<Indexer>>>,
    workspace_root: Option<std::path::PathBuf>,
    max_backups: usize,
}

impl McpServer {
    pub async fn run(store: Option<crate::store::Store>, indexer: Option<Arc<Mutex<Indexer>>>, workspace_root: Option<std::path::PathBuf>, max_backups: usize) -> Result<()> {
        let mut server = Self { store, indexer, workspace_root, max_backups };
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
                if let Some(params_obj) = params.as_ref().and_then(|p| p.as_object()) {
                    if let Some(root_uri) = params_obj.get("rootUri").and_then(|v| v.as_str()) {
                        let path_str = if root_uri.starts_with("file://") {
                            &root_uri[7..]
                        } else {
                            root_uri
                        };
                        if path_str != "/" && !path_str.is_empty() {
                            path = Some(std::path::PathBuf::from(path_str));
                        }
                    }
                }
                
                // Fallback to current working directory if rootUri is missing or "/"
                let path = path.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("/")));
                
                if let Ok(workspace) = crate::workspace::Workspace::discover(&path, None) {
                    self.workspace_root = Some(workspace.root.clone());
                    
                    match workspace.get_store().await {
                        Ok(store) => {
                            let sync_workspace = workspace.clone();
                            let sync_store = store.clone();
                            tokio::spawn(async move {
                                if let Ok(sync_indexer) = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
                                    let _ = crate::indexer::sync_vault(&sync_workspace, &sync_store, sync_indexer).await;
                                }
                            });
                            self.store = Some(store);
                        }
                        Err(e) => {
                        }
                    }
                } else {
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
            "tools/list" => {
                Ok(json!({
                    "tools": [
                        {
                            "name": "search_memory",
                            "description": "Search the knowledge graph.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "query": { "type": "string" },
                                    "type": { "type": "string" },
                                    "limit": { "type": "integer" },
                                    "include_content": { "type": "boolean" }
                                },
                                "required": ["query"]
                            }
                        },
                        {
                            "name": "read",
                            "description": "Read a markdown document.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "path": { "type": "string" }
                                }
                            }
                        },
                        {
                            "name": "write",
                            "description": "Write a markdown document.",
                            "inputSchema": {
                                "type": "object",
                                "properties": {
                                    "id": { "type": "string" },
                                    "path": { "type": "string" },
                                    "content": { "type": "string" },
                                    "mode": { "type": "string", "enum": ["create", "append", "replace"] }
                                },
                                "required": ["path", "mode", "content"]
                            }
                        }
                    ]
                }))
            }
            "tools/call" => {
                let params = params.unwrap_or(json!({}));
                let name = params["name"].as_str().unwrap_or("");
                let args = params["arguments"].as_object().cloned().unwrap_or_default();
                
                match name {
                    "search_memory" => {
                        let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
                        let query_str = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                        
                        let query_vector = {
                            if self.indexer.is_none() {
                                let idx = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or_else(|_| Err(anyhow::anyhow!("Indexer spawn blocked")))?;
                                self.indexer = Some(Arc::new(Mutex::new(idx)));
                            }
                            let mut indexer = self.indexer.as_ref().unwrap().lock().await;
                            let embeddings = indexer.embed(&[query_str.to_string()]).map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
                            embeddings.into_iter().next().unwrap_or_default()
                        };

                        let results = store.search(query_vector, query_str.to_string(), limit).await?;
                        
                        Ok(json!({
                            "content": [{"type": "text", "text": serde_json::to_string(&results)? }]
                        }))
                    }
                    "read" => {
                        let workspace_root = self.workspace_root.as_ref().ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
                        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let file_path = workspace_root.join(path_str);
                        
                        if let Some(linked_content) = crate::link::get_linked_content(&file_path) {
                            Ok(json!({
                                "content": [{"type": "text", "text": linked_content}]
                            }))
                        } else {
                            let store = self.store.as_ref().ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
                            let full_text = store.read_document(path_str).await?;
                            Ok(json!({
                                "content": [{"type": "text", "text": full_text}]
                            }))
                        }
                    }
                    "write" => {
                        let workspace_root = self.workspace_root.as_ref().ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
                        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
                        
                        let initial_file_path = workspace_root.join(path_str);
                        let file_path = crate::link::resolve_link(&initial_file_path);
                        
                        // WRITE-GUARD: Backup file if it exists
                        if file_path.exists() && self.max_backups > 0 {
                            let mut backups = Vec::new();
                            let parent = file_path.parent().unwrap_or(std::path::Path::new(""));
                            let base_name = file_path.file_name().unwrap_or_default().to_string_lossy();
                            
                            // Discover existing backups
                            if let Ok(entries) = std::fs::read_dir(parent) {
                                for entry in entries.flatten() {
                                    let name = entry.file_name().to_string_lossy().to_string();
                                    if name.starts_with(&format!("{}.bak.", base_name)) {
                                        backups.push(entry.path());
                                    }
                                }
                            }
                            
                            // Sort by modification time (oldest first)
                            backups.sort_by_key(|a| std::fs::metadata(a).and_then(|m| m.modified()).unwrap_or(std::time::SystemTime::UNIX_EPOCH));
                            
                            // Keep up to max_backups - 1 before adding the new one
                            while backups.len() >= self.max_backups {
                                if let Some(oldest) = backups.first() {
                                    let _ = std::fs::remove_file(oldest);
                                }
                                backups.remove(0);
                            }
                            
                            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S");
                            let bak_path = parent.join(format!("{}.bak.{}", base_name, timestamp));
                            
                            if let Err(e) = std::fs::copy(&file_path, &bak_path) {
                                tracing::error!("Write-Guard: Failed to create snapshot for {:?}: {}", file_path, e);
                            } else {
                                tracing::info!("Write-Guard: Created snapshot at {:?}", bak_path);
                            }
                        }

                        match mode {
                            "append" => {
                                use std::io::Write;
                                let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&file_path)?;
                                f.write_all(content.as_bytes())?;
                            }
                            _ => {
                                std::fs::write(&file_path, content)?;
                            }
                        }
                        Ok(json!({
                            "content": [{"type": "text", "text": format!("Successfully wrote to {}", path_str)}]
                        }))
                    }
                    _ => anyhow::bail!("Unknown tool"),
                }
            }
            _ => anyhow::bail!("Method not found"),
        }
    }
}

