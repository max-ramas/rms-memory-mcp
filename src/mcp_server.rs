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

fn write_json_line(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn file_uri_to_path(uri: &str) -> Result<std::path::PathBuf> {
    if !uri.starts_with("file:") {
        anyhow::bail!("only file:// roots are supported");
    }
    let url = url::Url::parse(uri)?;
    url.to_file_path()
        .map_err(|_| anyhow::anyhow!("invalid file URI"))
}

/// Resolve a `manifest` argument for `rms_wiki_pack` to a real file on disk,
/// requiring the resulting path (after any symlinks) to stay inside the vault.
///
/// This prevents an attacker from pointing the wiki packager at
/// `/etc/passwd`, a private key store, or any other file outside the vault
/// and reading it back through the generated wiki output.
fn resolve_manifest_path(
    workspace_root: &std::path::Path,
    manifest_arg: &str,
) -> Result<std::path::PathBuf> {
    if manifest_arg.trim().is_empty() {
        anyhow::bail!("Wiki manifest path must not be empty");
    }
    let candidate_path = std::path::Path::new(manifest_arg);
    // Absolute manifests are accepted only if they still resolve inside the
    // canonicalised vault root. All other manifests are treated as vault-
    // relative and cannot contain `..` traversal segments.
    let candidate = if candidate_path.is_absolute() {
        candidate_path.to_path_buf()
    } else {
        for component in candidate_path.components() {
            if component == std::path::Component::ParentDir {
                anyhow::bail!(
                    "Wiki manifest path must not contain '..': {}",
                    manifest_arg
                );
            }
        }
        workspace_root.join(candidate_path)
    };

    let canonical_root = std::fs::canonicalize(workspace_root)
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let canonical = std::fs::canonicalize(&candidate).map_err(|error| {
        anyhow::anyhow!(
            "Wiki manifest not found at {}: {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(&canonical_root) {
        anyhow::bail!(
            "Wiki manifest '{}' resolves outside the vault ({}), refusing to load",
            manifest_arg,
            canonical.display()
        );
    }
    Ok(canonical)
}

use crate::indexer::Indexer;
use crate::tools::AppContext;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct McpServer {
    ctx: AppContext,
    shutdown_tx: tokio::sync::watch::Sender<bool>,
    client_supports_roots: bool,
    pending_roots_request_id: Option<Value>,
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
            match crate::indexer::try_sync_vault(&workspace, &store, &mut idx).await {
                Ok(crate::indexer::SyncStatus::Completed) => {}
                Ok(crate::indexer::SyncStatus::Busy) => {
                    tracing::info!("Initial sync skipped: another process is indexing this vault.");
                }
                Err(e) => tracing::error!("Initial sync failed: {:#}", e),
            }
        }

        // File Watcher
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let watched_vault_root = workspace.root.clone();
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
                        let is_markdown = path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
                        if is_markdown
                            && !crate::path_policy::is_vault_wiki_path(&watched_vault_root, path)
                            && !p.contains(".lancedb")
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
                    match crate::indexer::try_sync_vault(&workspace, &store, &mut idx).await {
                        Ok(crate::indexer::SyncStatus::Completed) => {}
                        Ok(crate::indexer::SyncStatus::Busy) => {
                            pending_sync = true;
                            debounce_timer.as_mut().reset(
                                tokio::time::Instant::now()
                                    + tokio::time::Duration::from_secs(3),
                            );
                            tracing::debug!("Background sync deferred: index lock is busy.");
                        }
                        Err(e) => tracing::error!("Background sync failed: {:#}", e),
                    }
                }
            }
        }

        drop(watcher);
    });
}

fn spawn_code_watcher(
    workspace: crate::workspace::Workspace,
    store: crate::store::Store,
    indexer: Arc<Mutex<Indexer>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    tokio::spawn(async move {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<std::time::SystemTime>(100);
        let watched_languages = workspace.code_languages.clone();
        let watched_vault_root = workspace.root.clone();
        let mut watcher = match notify::RecommendedWatcher::new(
            move |result: notify::Result<notify::Event>| {
                if let Ok(event) = result
                    && matches!(
                        event.kind,
                        notify::EventKind::Modify(_)
                            | notify::EventKind::Create(_)
                            | notify::EventKind::Remove(_)
                    )
                    && event.paths.iter().any(|path| {
                        is_watched_code_path(&watched_vault_root, path, &watched_languages)
                    })
                {
                    let _ = tx.try_send(std::time::SystemTime::now());
                }
            },
            notify::Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::error!("Failed to create code watcher: {error}");
                return;
            }
        };
        use notify::Watcher;
        if let Err(error) = watcher.watch(&workspace.code_path, notify::RecursiveMode::Recursive) {
            tracing::error!("Failed to watch code path: {error}");
            return;
        }

        let mut dirty_since = if crate::code_indexer::code_index_is_initialized(&store.storage_path)
        {
            None
        } else {
            Some(std::time::SystemTime::now())
        };
        let debounce = tokio::time::sleep(tokio::time::Duration::from_secs(3));
        tokio::pin!(debounce);
        if dirty_since.is_some() {
            debounce.as_mut().reset(tokio::time::Instant::now());
        }

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => break,
                observed = rx.recv() => {
                    let Some(observed) = observed else { break; };
                    dirty_since = Some(dirty_since.map_or(observed, |current| current.min(observed)));
                    debounce.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_secs(3));
                }
                _ = &mut debounce, if dirty_since.is_some() => {
                    let Some(observed) = dirty_since else {
                        tracing::warn!("code watcher debounce fired without dirty_since");
                        continue;
                    };
                    if crate::code_indexer::code_index_is_fresh_since(&store.storage_path, observed) {
                        dirty_since = None;
                        continue;
                    }
                    let mut indexer = indexer.lock().await;
                    match crate::code_indexer::try_index_code(&workspace, &store, &mut indexer).await {
                        Ok(crate::code_indexer::CodeIndexStatus::Completed) => dirty_since = None,
                        Ok(crate::code_indexer::CodeIndexStatus::Busy) => {
                            debounce.as_mut().reset(tokio::time::Instant::now() + tokio::time::Duration::from_secs(3));
                        }
                        Err(error) => {
                            tracing::error!("Background code index failed: {error:#}");
                            dirty_since = None;
                        }
                    }
                }
            }
        }
        drop(watcher);
    });
}

fn is_watched_code_path(
    vault_root: &std::path::Path,
    path: &std::path::Path,
    configured_languages: &[String],
) -> bool {
    crate::code_indexer::is_indexable_code_path(path, configured_languages)
        && !crate::path_policy::is_vault_wiki_path(vault_root, path)
        && !path.components().any(|component| {
            matches!(
                component.as_os_str().to_str(),
                Some(".git" | ".rms-memory" | "node_modules" | "target" | "vendor")
            )
        })
}

impl McpServer {
    async fn wiki_service(&self) -> Result<crate::wiki::WikiService> {
        let store = Arc::new(
            self.ctx
                .store
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?,
        );
        let retrieval = crate::retrieval::RetrievalService::new(
            store,
            self.ctx
                .indexer
                .clone()
                .ok_or_else(|| anyhow::anyhow!("Indexer not initialized"))?,
        );
        let root = self.ctx.workspace_root.clone().unwrap_or_default();
        let scope = self.ctx.scope.clone().unwrap_or_default();
        Ok(crate::wiki::WikiService::new(retrieval, root, scope))
    }

    pub async fn run(
        store: Option<crate::store::Store>,
        indexer: Option<Arc<Mutex<Indexer>>>,
        workspace_root: Option<std::path::PathBuf>,
        max_backups: usize,
        scope: Option<String>,
    ) -> Result<()> {
        let (shutdown_tx, _shutdown_rx) = tokio::sync::watch::channel(false);
        let shutdown_tx_for_server = shutdown_tx.clone();
        let shared_indexer = if let Some(idx) = indexer {
            idx
        } else {
            Arc::new(Mutex::new(crate::indexer::Indexer::new().map_err(|e| {
                anyhow::anyhow!("Failed to initialize embedding model: {e}")
            })?))
        };
        let mut server = Self {
            ctx: AppContext {
                store,
                indexer: Some(shared_indexer.clone()),
                workspace_root,
                max_backups,
                scope,
                caller_id: "unknown".to_string(),
                project_key: None,
            },
            shutdown_tx: shutdown_tx_for_server,
            client_supports_roots: false,
            pending_roots_request_id: None,
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

            let message: Result<Value, _> = serde_json::from_str(&line);
            match message {
                Ok(message) if message.get("method").is_some() => {
                    let request: RpcRequest = serde_json::from_value(message)?;
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
                        if let Some(outbound) = server
                            .handle_notification(&request.method, request.params)
                            .await?
                        {
                            write_json_line(&mut stdout, &outbound)?;
                        }
                    }
                }
                Ok(message) if message.get("id").is_some() => {
                    server.handle_client_response(&message).await?;
                }
                Ok(_) => {
                    tracing::warn!("Ignoring JSON-RPC message without method or id");
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

    async fn bind_workspace(&mut self, workspace: crate::workspace::Workspace) -> Result<()> {
        let project_key = workspace.project_key();
        if let Some(current_root) = &self.ctx.workspace_root {
            if current_root == &workspace.root {
                return Ok(());
            }
            anyhow::bail!(
                "MCP connection is already bound to project '{}'; refusing to switch to '{}'",
                self.ctx.project_key.as_deref().unwrap_or("unknown"),
                project_key.as_deref().unwrap_or("unknown")
            );
        }

        let store = workspace.get_store().await?;
        let indexer = self
            .ctx
            .indexer
            .clone()
            .ok_or_else(|| anyhow::anyhow!("Indexer not initialized"))?;
        spawn_sync_watcher(
            workspace.clone(),
            store.clone(),
            indexer.clone(),
            self.shutdown_tx.subscribe(),
        );
        if workspace.code_index_mode == crate::workspace::CodeIndexMode::Watch {
            spawn_code_watcher(
                workspace.clone(),
                store.clone(),
                indexer,
                self.shutdown_tx.subscribe(),
            );
        }
        self.ctx.workspace_root = Some(workspace.root.clone());
        self.ctx.project_key = project_key;
        self.ctx.store = Some(store);
        tracing::info!(
            "MCP workspace bound: client={} project_key={} vault_root={}",
            self.ctx.caller_id,
            self.ctx.project_key.as_deref().unwrap_or("none"),
            workspace.root.display()
        );
        Ok(())
    }

    async fn bind_project_argument(&mut self, args: &serde_json::Map<String, Value>) -> Result<()> {
        let requested = args.get("project").and_then(Value::as_str);
        if let Some(current_root) = &self.ctx.workspace_root {
            if let Some(requested) = requested
                && self.ctx.project_key.as_deref() != Some(requested)
            {
                anyhow::bail!(
                    "MCP connection is already bound to project '{}', not '{}'",
                    self.ctx.project_key.as_deref().unwrap_or("unknown"),
                    requested
                );
            }
            tracing::debug!(
                "Using initialized MCP workspace at {}",
                current_root.display()
            );
            return Ok(());
        }

        let requested = requested.ok_or_else(|| {
            anyhow::anyhow!(
                "Workspace root not initialized. Pass the registered project key in the `project` argument (for example `rms-threads-assistant`), or use `rms_projects` to list keys."
            )
        })?;
        let registry = crate::workspace::Registry::load()?;
        if let Some(config) = registry.locate_by_project(requested) {
            let workspace =
                crate::workspace::Workspace::discover(std::path::Path::new(&config.code_path), None)?;
            return self.bind_workspace(workspace).await;
        }
        if let Some(message) = registry.migration_redirect_message(requested) {
            anyhow::bail!(message);
        }
        anyhow::bail!("Unknown RMS Memory project key: '{requested}'");
    }

    async fn handle_notification(
        &mut self,
        method: &str,
        _params: Option<Value>,
    ) -> Result<Option<Value>> {
        match method {
            "notifications/initialized" | "notifications/roots/list_changed"
                if self.ctx.workspace_root.is_none()
                    && self.client_supports_roots
                    && self.pending_roots_request_id.is_none() =>
            {
                let id = Value::String("rms-memory-roots-1".to_string());
                self.pending_roots_request_id = Some(id.clone());
                Ok(Some(json!({
                    "jsonrpc": "2.0",
                    "id": id,
                    "method": "roots/list",
                    "params": {}
                })))
            }
            _ => Ok(None),
        }
    }

    async fn handle_client_response(&mut self, message: &Value) -> Result<()> {
        let Some(pending_id) = self.pending_roots_request_id.as_ref() else {
            tracing::debug!("Ignoring unsolicited JSON-RPC response");
            return Ok(());
        };
        if message.get("id") != Some(pending_id) {
            tracing::debug!("Ignoring JSON-RPC response with an unknown id");
            return Ok(());
        }
        self.pending_roots_request_id = None;
        if let Some(error) = message.get("error") {
            tracing::warn!("MCP roots/list failed: {error}");
            return Ok(());
        }

        let roots = message
            .pointer("/result/roots")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let mut candidates = Vec::<crate::workspace::Workspace>::new();
        for root in roots {
            let Some(uri) = root.get("uri").and_then(Value::as_str) else {
                continue;
            };
            let path = match file_uri_to_path(uri) {
                Ok(path) => path,
                Err(error) => {
                    tracing::warn!("Ignoring unsupported MCP root URI '{uri}': {error}");
                    continue;
                }
            };
            if let Ok(workspace) = crate::workspace::Workspace::discover(&path, None)
                && !candidates.iter().any(|item| item.root == workspace.root)
            {
                candidates.push(workspace);
            }
        }

        match candidates.len() {
            1 => self.bind_workspace(candidates.remove(0)).await?,
            0 => tracing::warn!(
                "MCP roots/list contained no registered RMS Memory project; tool calls must pass `project`"
            ),
            count => tracing::warn!(
                "MCP roots/list resolved to {count} registered projects; tool calls must pass `project`"
            ),
        }
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
                    if let Some(root_uri) = params_obj.get("rootUri").and_then(|v| v.as_str())
                        && let Ok(root_path) = file_uri_to_path(root_uri)
                        && root_path != std::path::Path::new("/")
                    {
                        path = Some(root_path);
                    }
                    self.client_supports_roots = params_obj
                        .get("capabilities")
                        .and_then(Value::as_object)
                        .is_some_and(|capabilities| capabilities.contains_key("roots"));
                }

                if path.is_none() && self.ctx.scope.is_some() {
                    path = Some(std::env::current_dir().unwrap_or_default());
                }
                if path.is_none()
                    && !self.client_supports_roots
                    && let Ok(cwd) = std::env::current_dir()
                    && cwd != std::path::Path::new("/")
                {
                    path = Some(cwd);
                }

                if let Some(path) = path {
                    match crate::workspace::Workspace::discover_with_scope(
                        self.ctx.scope.as_deref(),
                        &path,
                        None,
                    ) {
                        Ok(workspace) => {
                            if let Err(error) = self.bind_workspace(workspace).await {
                                tracing::error!("Failed to initialize MCP workspace: {error:#}");
                            }
                        }
                        Err(error) => tracing::warn!(
                            "Cannot initialize MCP workspace from {}: {error:#}",
                            path.display()
                        ),
                    }
                } else {
                    tracing::warn!(
                        "MCP client did not provide a workspace root; waiting for roots/list or an explicit tool `project` argument"
                    );
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
                        "description": "Search RMS Memory. `corpus=vault` (default) searches human Markdown memory; `code` searches derived semantic code; `all` ranks each corpus independently and combines them with Reciprocal Rank Fusion, never raw vector distances.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The semantic query string to search for." },
                                "project": { "type": "string", "description": "Registered project key, used when the MCP client did not provide a workspace root." },
                                "corpus": { "type": "string", "enum": ["vault", "code", "all"], "description": "Corpus to search. Defaults to vault." },
                                "limit": { "type": "integer", "description": "Maximum number of chunks to return. Default is 10." },
                                "include_content": { "type": "boolean", "description": "Whether to include full chunk text in results." },
                                "min_confidence": { "type": "number", "description": "Optional minimum confidence threshold (0.0–1.0). Records with NULL confidence are always included. CAUTION: do NOT use high values (e.g. 0.9+) unless you need strict filtering. If zero results, retry without this parameter." }
                            },
                            "required": ["query"]
                        }
                    },
                    {
                        "name": "rms_code_search",
                        "description": "Search only the derived semantic code index. Results include language, file, symbol, kind, line range, and segment index. The code index is optional, so an unindexed project returns an empty result list.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "query": { "type": "string", "description": "The semantic code query." },
                                "project": { "type": "string", "description": "Registered project key, used when the MCP client did not provide a workspace root." },
                                "limit": { "type": "integer", "description": "Maximum results; default 10, maximum 100." },
                                "include_content": { "type": "boolean", "description": "Whether to include indexed code content; default true." }
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
                                "project": { "type": "string", "description": "Registered project key, used when the MCP client did not provide a workspace root." },
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
                                "project": { "type": "string", "description": "Registered project key, used when the MCP client did not provide a workspace root." },
                                "path": { "type": "string", "description": "Relative path to save the document (e.g., 'decisions/001-db.md')." },
                                "content": { "type": "string", "description": "The markdown content to write." },
                                "mode": { "type": "string", "enum": ["create", "append", "replace"], "description": "Write mode." },
                                "confidence": { "type": "number", "description": "Optional confidence score (0.0–1.0) indicating reliability of this record." },
                                "source": { "type": "string", "description": "Optional free-text citation or source reference for this record." }
                            },
                            "required": ["path", "mode", "content"]
                        }
                    },
                    {
                        "name": "rms_wiki_pack",
                        "description": "Generate a wiki context pack from the vault and code index. Returns material for an agent to create human-readable wiki documentation from verified sources.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {
                                "manifest": { "type": "string", "description": "Optional YAML manifest path for custom sections." },
                                "project": { "type": "string", "description": "Registered project key, used when the MCP client did not provide a workspace root." },
                                "refresh_code": { "type": "boolean", "description": "Force code reindex before generating." }
                            }
                        }
                    },
                    {
                        "name": "rms_projects",
                        "description": "List registered RMS Memory project keys. This tool works even when the MCP client did not provide a workspace root.",
                        "inputSchema": {
                            "type": "object",
                            "properties": {}
                        }
                    }
                ]
            })),
            "tools/call" => {
                let params = params.unwrap_or(json!({}));
                let name = params["name"].as_str().unwrap_or("");
                let args = params["arguments"].as_object().cloned().unwrap_or_default();

                if name == "rms_projects" {
                    let registry = crate::workspace::Registry::load()?;
                    let mut projects = registry.projects.keys().cloned().collect::<Vec<_>>();
                    projects.sort();
                    return Ok(json!({
                        "content": [{
                            "type": "text",
                            "text": serde_json::to_string_pretty(&projects)?
                        }]
                    }));
                }

                self.bind_project_argument(&args).await?;

                match name {
                    "rms_search" => crate::tools::search::execute(&self.ctx, &args).await,
                    "rms_code_search" => crate::tools::search::execute_code(&self.ctx, &args).await,
                    "rms_read" => crate::tools::read::execute(&self.ctx, &args).await,
                    "rms_write" => crate::tools::write::execute(&self.ctx, &args).await,
                    "rms_wiki_pack" => {
                        let refresh = args
                            .get("refresh_code")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let manifest = if let Some(path) =
                            args.get("manifest").and_then(|v| v.as_str())
                        {
                            let workspace_root =
                                self.ctx.workspace_root.as_ref().ok_or_else(|| {
                                    anyhow::anyhow!(
                                        "Workspace root not initialized; cannot resolve manifest path"
                                    )
                                })?;
                            let resolved = resolve_manifest_path(workspace_root, path)?;
                            crate::wiki::WikiManifest::from_file(&resolved)?
                        } else {
                            crate::wiki::WikiManifest::default_manifest()
                        };
                        let req = crate::wiki::WikiGenerateRequest {
                            manifest,
                            refresh_code: refresh,
                        };
                        let service = self.wiki_service().await?;
                        let result = service.generate(req).await?;
                        Ok(serde_json::json!({
                            "content": [{
                                "type": "text",
                                "text": format!("Wiki pack generated. Pack ID: {}. Context pack: {}. Sections: {}.",
                                    result.pack_id, result.context_pack_path.display(), result.sections_generated)
                            }]
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
    use tempfile::tempdir;

    #[test]
    fn resolve_manifest_path_accepts_vault_relative() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("m.yaml"), "schema_version: 1\nsections: []\n").unwrap();
        let resolved = super::resolve_manifest_path(dir.path(), "m.yaml").unwrap();
        assert!(resolved.starts_with(std::fs::canonicalize(dir.path()).unwrap()));
    }

    #[test]
    fn resolve_manifest_path_accepts_absolute_inside_vault() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("m.yaml"), "x").unwrap();
        let absolute = dir.path().join("m.yaml");
        let resolved =
            super::resolve_manifest_path(dir.path(), absolute.to_str().unwrap()).unwrap();
        assert!(resolved.starts_with(std::fs::canonicalize(dir.path()).unwrap()));
    }

    #[test]
    fn resolve_manifest_path_rejects_absolute_outside_vault() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        std::fs::write(outside.path().join("evil.yaml"), "x").unwrap();
        let outside_path = outside.path().join("evil.yaml");
        let error = super::resolve_manifest_path(vault.path(), outside_path.to_str().unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.contains("outside the vault"), "got: {error}");
    }

    #[test]
    fn resolve_manifest_path_rejects_parent_traversal() {
        let vault = tempdir().unwrap();
        let error = super::resolve_manifest_path(vault.path(), "../evil.yaml")
            .unwrap_err()
            .to_string();
        assert!(error.contains(".."), "got: {error}");
    }

    #[cfg(unix)]
    #[test]
    fn resolve_manifest_path_rejects_symlink_escape() {
        let vault = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("evil.yaml");
        std::fs::write(&target, "x").unwrap();
        std::os::unix::fs::symlink(&target, vault.path().join("escape.yaml")).unwrap();
        let error = super::resolve_manifest_path(vault.path(), "escape.yaml")
            .unwrap_err()
            .to_string();
        assert!(error.contains("outside the vault"), "got: {error}");
    }

    #[test]
    fn resolve_manifest_path_rejects_missing_file() {
        let vault = tempdir().unwrap();
        let error = super::resolve_manifest_path(vault.path(), "missing.yaml")
            .unwrap_err()
            .to_string();
        assert!(error.contains("not found"), "got: {error}");
    }

    #[test]
    fn file_root_uri_decodes_spaces() {
        assert_eq!(
            super::file_uri_to_path("file:///tmp/rms%20memory").unwrap(),
            std::path::PathBuf::from("/tmp/rms memory")
        );
    }

    #[test]
    fn non_file_root_uri_is_rejected() {
        assert!(super::file_uri_to_path("https://example.com/repo").is_err());
    }

    #[test]
    fn code_watcher_filters_non_source_and_generated_paths() {
        let auto = ["auto".to_string()];
        let vault = std::path::Path::new("/vault");
        assert!(super::is_watched_code_path(
            vault,
            std::path::Path::new("src/lib.rs"),
            &auto
        ));
        assert!(super::is_watched_code_path(
            vault,
            std::path::Path::new("src/main.go"),
            &auto
        ));
        assert!(!super::is_watched_code_path(
            vault,
            std::path::Path::new("README.md"),
            &auto
        ));
        assert!(!super::is_watched_code_path(
            vault,
            std::path::Path::new("target/debug/lib.rs"),
            &auto
        ));
        assert!(!super::is_watched_code_path(
            vault,
            std::path::Path::new(".git/hooks/check.rs"),
            &auto
        ));
        assert!(!super::is_watched_code_path(
            vault,
            std::path::Path::new("src/lib.rs"),
            &["go".to_string()]
        ));
        assert!(!super::is_watched_code_path(
            vault,
            std::path::Path::new("/vault/wiki/generated.rs"),
            &auto
        ));
        assert!(super::is_watched_code_path(
            vault,
            std::path::Path::new("/project/wiki/handwritten.rs"),
            &auto
        ));
    }
}
