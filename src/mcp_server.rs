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

pub struct McpServer<S: VectorStore + 'static, E: crate::indexer::Embedder + 'static> {
    store: S,
    indexer: Arc<Mutex<E>>,
    workspace_root: std::path::PathBuf,
    max_backups: usize,
}

impl<S: VectorStore + 'static, E: crate::indexer::Embedder + 'static> McpServer<S, E> {
    pub async fn run(store: S, indexer: Arc<Mutex<E>>, workspace_root: std::path::PathBuf, max_backups: usize) -> Result<()> {
        let server = Self { store, indexer, workspace_root, max_backups };
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

    async fn handle_request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        match method {
            "initialize" => {
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
                                "required": ["mode", "content"]
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
                        let query_str = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                        let limit = args.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
                        
                        let query_vector = {
                            let mut indexer = self.indexer.lock().await;
                            let embeddings = indexer.embed(&[query_str.to_string()]).map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
                            embeddings.into_iter().next().unwrap_or_default()
                        };

                        let results = self.store.search(query_vector, query_str.to_string(), limit).await?;
                        
                        Ok(json!({
                            "content": [{"type": "text", "text": serde_json::to_string(&results)? }]
                        }))
                    }
                    "read" => {
                        let path = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let full_text = self.store.read_document(path).await?;
                        Ok(json!({
                            "content": [{"type": "text", "text": full_text}]
                        }))
                    }
                    "write" => {
                        let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
                        let content = args.get("content").and_then(|v| v.as_str()).unwrap_or("");
                        let mode = args.get("mode").and_then(|v| v.as_str()).unwrap_or("replace");
                        
                        let file_path = self.workspace_root.join(path_str);
                        
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SearchResult;
    use serde_json::json;

    struct MockStore;
    impl VectorStore for MockStore {
        async fn search(&self, _query_vector: Vec<f32>, query_str: String, limit: usize) -> anyhow::Result<Vec<SearchResult>> {
            let mut results = Vec::new();
            if query_str == "test query" {
                results.push(SearchResult {
                    path: "doc1.md".to_string(),
                    heading: "Heading 1".to_string(),
                    text: "Content 1".to_string(),
                    score: Some(0.9),
                });
            }
            Ok(results)
        }
        
        async fn read_document(&self, path: &str) -> anyhow::Result<String> {
            if path == "doc1.md" {
                Ok("Full document text for doc1".to_string())
            } else {
                Err(anyhow::anyhow!("File not found"))
            }
        }
    }

    struct MockEmbedder;
    impl crate::indexer::Embedder for MockEmbedder {
        fn embed(&mut self, _texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            Ok(vec![vec![0.1; 384]])
        }
    }

    #[tokio::test]
    async fn test_mcp_initialize() {
        let store = MockStore;
        let indexer = Arc::new(Mutex::new(MockEmbedder));
        let server = McpServer {
            store,
            indexer,
            workspace_root: std::path::PathBuf::from("/tmp"),
            max_backups: 5,
        };

        let res = server.handle_request("initialize", None).await.unwrap();
        assert_eq!(res["protocolVersion"], "2024-11-05");
        assert_eq!(res["serverInfo"]["name"], "rms-memory");
    }

    #[tokio::test]
    async fn test_mcp_search_memory() {
        let store = MockStore;
        let indexer = Arc::new(Mutex::new(MockEmbedder));
        let server = McpServer {
            store,
            indexer,
            workspace_root: std::path::PathBuf::from("/tmp"),
            max_backups: 5,
        };

        let params = Some(json!({"name": "search_memory", "arguments": {"query": "test query", "limit": 10}}));
        let res = server.handle_request("tools/call", params).await.unwrap();
        
        let content_arr = res["content"].as_array().unwrap();
        let content_text = content_arr[0]["text"].as_str().unwrap();
        assert!(content_text.contains("doc1.md"));
        assert!(content_text.contains("Content 1"));
    }

    #[tokio::test]
    async fn test_mcp_read() {
        let store = MockStore;
        let indexer = Arc::new(Mutex::new(MockEmbedder));
        let server = McpServer {
            store,
            indexer,
            workspace_root: std::path::PathBuf::from("/tmp"),
            max_backups: 5,
        };

        let params = Some(json!({"name": "read", "arguments": {"path": "doc1.md"}}));
        let res = server.handle_request("tools/call", params).await.unwrap();
        
        let content_arr = res["content"].as_array().unwrap();
        let content_text = content_arr[0]["text"].as_str().unwrap();
        assert_eq!(content_text, "Full document text for doc1");
    }
}
