import sys

with open('/Users/ramas/dev/rms-memory-mcp/src/mcp_server.rs', 'r') as f:
    content = f.read()

watcher_func = """
fn spawn_sync_watcher(workspace: crate::workspace::Workspace, store: crate::store::Store) {
    tokio::spawn(async move {
        // Initial sync
        if let Ok(sync_indexer) = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
            let _ = crate::indexer::sync_vault(&workspace, &store, sync_indexer).await;
        }
        
        // File Watcher
        let (tx, mut rx) = tokio::sync::mpsc::channel(100);
        let mut watcher = match notify::RecommendedWatcher::new(
            move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res {
                    if matches!(event.kind, notify::EventKind::Modify(_) | notify::EventKind::Create(_) | notify::EventKind::Remove(_)) {
                        let _ = tx.blocking_send(());
                    }
                }
            },
            notify::Config::default()
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

        let mut debounce_timer = tokio::time::sleep(tokio::time::Duration::from_secs(3));
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
                    if let Ok(sync_indexer) = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
                        let _ = crate::indexer::sync_vault(&workspace, &store, sync_indexer).await;
                    }
                }
            }
        }
        
        drop(watcher);
    });
}
"""

# Insert watcher_func before impl McpServer
content = content.replace("impl McpServer {\n", watcher_func + "\nimpl McpServer {\n")

old_spawn = """                            let sync_workspace = workspace.clone();
                            let sync_store = store.clone();
                            tokio::spawn(async move {
                                if let Ok(sync_indexer) = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
                                    let _ = crate::indexer::sync_vault(&sync_workspace, &sync_store, sync_indexer).await;
                                }
                            });"""

new_spawn = """                            spawn_sync_watcher(workspace.clone(), store.clone());"""

content = content.replace(old_spawn, new_spawn)

# The second one uses slightly different indentation
old_spawn2 = """                                    let sync_workspace = workspace.clone();
                                    let sync_store = store.clone();
                                    tokio::spawn(async move {
                                        if let Ok(sync_indexer) = tokio::task::spawn_blocking(|| crate::indexer::Indexer::new()).await.unwrap_or(Err(anyhow::anyhow!("spawn_blocking failed"))) {
                                            let _ = crate::indexer::sync_vault(&sync_workspace, &sync_store, sync_indexer).await;
                                        }
                                    });"""

new_spawn2 = """                                    spawn_sync_watcher(workspace.clone(), store.clone());"""

content = content.replace(old_spawn2, new_spawn2)

with open('/Users/ramas/dev/rms-memory-mcp/src/mcp_server.rs', 'w') as f:
    f.write(content)

print("Patch applied successfully.")
