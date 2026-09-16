//! Agent-facing durable knowledge graph (`rms_graph`).

use super::AppContext;
use anyhow::{Result, anyhow};
use rms_memory_index::graph::{
    EdgeOverrideAction, EdgeRelation, GraphEdgeRecord, GraphNodeKey, GraphNodeRecord,
};
use rms_memory_index::semantic_graph::{SemanticGraphOptions, build_semantic_edges};
use rms_memory_index::store::Store;
use rms_memory_index::vault_graph;
use std::collections::{HashMap, HashSet, VecDeque};

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let action = args
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("status");
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow!("Store not initialized"))?;
    let workspace = workspace_from_ctx(ctx)?;
    let explicit_project = args.get("project").and_then(|v| v.as_str());

    match action {
        "status" => action_status(store).await,
        "ensure" => {
            let force = args.get("force").and_then(|v| v.as_bool()).unwrap_or(false);
            action_ensure(store, &workspace, force).await
        }
        "neighbors" => {
            let node = args
                .get("node")
                .and_then(|v| v.as_str())
                .or_else(|| args.get("path").and_then(|v| v.as_str()))
                .ok_or_else(|| anyhow!("neighbors requires `node` (node_key) or `path`"))?;
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(16)
                .clamp(1, 64) as usize;
            action_neighbors(store, node, limit).await
        }
        "path" => {
            let from = args
                .get("from")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("path requires `from`"))?;
            let to = args
                .get("to")
                .and_then(|v| v.as_str())
                .ok_or_else(|| anyhow!("path requires `to`"))?;
            let max_depth = args
                .get("max_depth")
                .and_then(|v| v.as_u64())
                .unwrap_or(8)
                .clamp(1, 32) as usize;
            action_path(store, from, to, max_depth).await
        }
        "snapshot" => {
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(500)
                .clamp(1, 5000) as usize;
            action_snapshot(store, limit).await
        }
        "semantic" => action_semantic(ctx, &workspace, store, args).await,
        "create_edge" => {
            require_explicit_project(explicit_project, "create_edge")?;
            action_create_edge(store, args).await
        }
        "suppress_edge" | "edge_override" => {
            require_explicit_project(explicit_project, action)?;
            action_edge_override(store, args).await
        }
        "export_dot" => {
            let limit = args
                .get("limit")
                .and_then(|v| v.as_u64())
                .unwrap_or(2000)
                .clamp(1, 5000) as usize;
            action_export_dot(store, limit).await
        }
        other => Err(anyhow!(
            "Unknown rms_graph action '{other}'. Valid: status, ensure, neighbors, path, snapshot, semantic, create_edge, suppress_edge, edge_override, export_dot."
        )),
    }
}

fn require_explicit_project(project: Option<&str>, action: &str) -> Result<()> {
    if project.is_none() {
        return Err(anyhow!(
            "action={action} requires an explicit `project` key (refuses sticky-bind mutation)"
        ));
    }
    Ok(())
}

pub fn workspace_from_ctx(ctx: &AppContext) -> Result<rms_memory_core::workspace::Workspace> {
    let root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow!("Workspace root not initialized"))?;
    let code_path = ctx.code_path.as_ref().unwrap_or(root);
    match rms_memory_core::workspace::Workspace::discover(code_path, None) {
        Ok(workspace) => Ok(workspace),
        Err(_) => Ok(rms_memory_core::workspace::Workspace {
            root: root.clone(),
            code_path: code_path.clone(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: rms_memory_core::workspace::CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
        }),
    }
}

/// Outcome of the best-effort vault graph refresh after a successful write.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum GraphRefreshOutcome {
    Ok,
    Skipped { reason: String },
    Failed { error: String },
}

/// Best-effort vault graph refresh after a successful `rms_write`.
/// Failures must not undo a committed vault write; callers surface the outcome.
pub async fn refresh_vault_graph_after_write(ctx: &AppContext) -> GraphRefreshOutcome {
    let Some(store) = ctx.store.as_ref() else {
        return GraphRefreshOutcome::Skipped {
            reason: "store not initialized".into(),
        };
    };
    let workspace = match workspace_from_ctx(ctx) {
        Ok(workspace) => workspace,
        Err(error) => {
            tracing::warn!("post-write vault graph refresh skipped: {error:#}");
            return GraphRefreshOutcome::Skipped {
                reason: format!("{error:#}"),
            };
        }
    };
    match vault_graph::reconcile_vault_links(&workspace, store).await {
        Ok(()) => GraphRefreshOutcome::Ok,
        Err(error) => {
            tracing::warn!("post-write vault graph refresh failed: {error:#}");
            GraphRefreshOutcome::Failed {
                error: format!("{error:#}"),
            }
        }
    }
}

async fn action_status(store: &Store) -> Result<serde_json::Value> {
    let tables = store.open_or_create_graph_tables().await?;
    let generation = store.next_graph_generation().await?;
    let nodes = store.query_graph_nodes(&tables, generation).await?;
    let edges = store.query_graph_edges(&tables, generation).await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "status",
        "generation_next": generation,
        "node_count": nodes.len(),
        "edge_count": edges.len(),
        "empty": nodes.is_empty(),
    }))
}

async fn action_ensure(
    store: &Store,
    workspace: &rms_memory_core::workspace::Workspace,
    force: bool,
) -> Result<serde_json::Value> {
    let tables = store.open_or_create_graph_tables().await?;
    let generation = store.next_graph_generation().await?;
    let before = store.query_graph_nodes(&tables, generation).await?.len();
    let refreshed = force || before == 0;
    if refreshed {
        vault_graph::reconcile_vault_links(workspace, store).await?;
    }
    let tables = store.open_or_create_graph_tables().await?;
    let generation = store.next_graph_generation().await?;
    let nodes = store.query_graph_nodes(&tables, generation).await?;
    let edges = store.query_graph_edges(&tables, generation).await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "ensure",
        "refreshed": refreshed,
        "force": force,
        "node_count": nodes.len(),
        "edge_count": edges.len(),
    }))
}

async fn load_graph(
    store: &Store,
) -> Result<(
    Vec<GraphNodeRecord>,
    Vec<GraphEdgeRecord>,
    HashMap<String, GraphNodeRecord>,
)> {
    let tables = store.open_or_create_graph_tables().await?;
    let generation = store.next_graph_generation().await?;
    let nodes = store.query_graph_nodes(&tables, generation).await?;
    let edges = store.query_graph_edges(&tables, generation).await?;
    let by_key = nodes
        .iter()
        .cloned()
        .map(|node| (node.node_key.as_str().to_string(), node))
        .collect::<HashMap<_, _>>();
    Ok((nodes, edges, by_key))
}

fn resolve_node_key(nodes: &[GraphNodeRecord], needle: &str) -> Result<String> {
    resolve_node_key_for_hit(nodes, needle, None, None)
}

/// Resolve a search/graph hit to a durable node key.
/// Prefer an exact symbol label when provided; for bare paths prefer
/// vault docs or code `file` nodes over arbitrary symbol rows that share the path.
fn resolve_node_key_for_hit(
    nodes: &[GraphNodeRecord],
    needle: &str,
    qualified_symbol: Option<&str>,
    prefer_corpus: Option<&str>,
) -> Result<String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Err(anyhow!("node identifier must be non-empty"));
    }
    if let Some(node) = nodes.iter().find(|n| n.node_key.as_str() == needle) {
        return Ok(node.node_key.as_str().to_string());
    }
    if let Some(symbol) = qualified_symbol.map(str::trim).filter(|s| !s.is_empty())
        && let Some(node) = nodes
            .iter()
            .find(|n| n.label == symbol || n.source_id == symbol)
    {
        return Ok(node.node_key.as_str().to_string());
    }
    let normalized = needle.replace('\\', "/");
    let path_matches = |n: &&GraphNodeRecord| {
        n.path
            .as_deref()
            .is_some_and(|path| path.replace('\\', "/") == normalized)
    };
    if let Some(corpus) = prefer_corpus {
        if let Some(node) = nodes
            .iter()
            .find(|n| path_matches(n) && n.corpus == corpus && n.kind == "file")
        {
            return Ok(node.node_key.as_str().to_string());
        }
        if let Some(node) = nodes.iter().find(|n| path_matches(n) && n.corpus == corpus) {
            return Ok(node.node_key.as_str().to_string());
        }
    }
    if let Some(node) = nodes
        .iter()
        .find(|n| path_matches(n) && (n.corpus == "vault" || n.kind == "file"))
    {
        return Ok(node.node_key.as_str().to_string());
    }
    if let Some(node) = nodes.iter().find(path_matches) {
        return Ok(node.node_key.as_str().to_string());
    }
    if let Some(node) = nodes
        .iter()
        .find(|n| n.source_id == needle || n.label == needle)
    {
        return Ok(node.node_key.as_str().to_string());
    }
    Err(anyhow!("graph node not found for '{needle}'"))
}

fn parse_user_node_key(raw: &str) -> Result<GraphNodeKey> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 512 {
        return Err(anyhow!(
            "graph node key must be non-empty and at most 512 characters"
        ));
    }
    let Some((corpus, source_id)) = raw.split_once(':') else {
        return Err(anyhow!(
            "graph node key must look like vault:<id>, code:<id>, or external:<id>"
        ));
    };
    match corpus {
        "vault" => GraphNodeKey::vault(source_id),
        "code" => GraphNodeKey::code(source_id),
        "external" => GraphNodeKey::external(source_id),
        other => Err(anyhow!(
            "unknown graph node corpus '{other}' (expected vault|code|external)"
        )),
    }
}

fn node_json(node: &GraphNodeRecord) -> serde_json::Value {
    serde_json::json!({
        "node_key": node.node_key.as_str(),
        "corpus": node.corpus,
        "source_id": node.source_id,
        "kind": node.kind,
        "label": node.label,
        "path": node.path,
        "generation": node.generation,
        "updated_at": node.updated_at,
    })
}

fn edge_json(edge: &GraphEdgeRecord) -> serde_json::Value {
    serde_json::json!({
        "edge_key": edge.edge_key,
        "source_key": edge.source_key.as_str(),
        "target_key": edge.target_key.as_str(),
        "relation": edge.relation.as_str(),
        "origin": edge.origin,
        "extractor": edge.extractor,
        "resolution": edge.resolution,
        "confidence": edge.confidence,
        "generation": edge.generation,
    })
}

async fn action_neighbors(store: &Store, needle: &str, limit: usize) -> Result<serde_json::Value> {
    let (nodes, edges, by_key) = load_graph(store).await?;
    let node_key = resolve_node_key(&nodes, needle)?;
    let mut neighbors = Vec::new();
    for edge in &edges {
        if neighbors.len() >= limit {
            break;
        }
        let (direction, neighbor_key) = if edge.source_key.as_str() == node_key {
            ("out", edge.target_key.as_str())
        } else if edge.target_key.as_str() == node_key {
            ("in", edge.source_key.as_str())
        } else {
            continue;
        };
        let neighbor = by_key
            .get(neighbor_key)
            .map(node_json)
            .unwrap_or_else(|| serde_json::json!({ "node_key": neighbor_key }));
        neighbors.push(serde_json::json!({
            "direction": direction,
            "edge": edge_json(edge),
            "neighbor": neighbor,
        }));
    }
    super::response::json_structured_response(&serde_json::json!({
        "action": "neighbors",
        "node_key": node_key,
        "neighbors": neighbors,
    }))
}

async fn action_path(
    store: &Store,
    from: &str,
    to: &str,
    max_depth: usize,
) -> Result<serde_json::Value> {
    let (nodes, edges, by_key) = load_graph(store).await?;
    let start = resolve_node_key(&nodes, from)?;
    let goal = resolve_node_key(&nodes, to)?;
    if start == goal {
        return super::response::json_structured_response(&serde_json::json!({
            "action": "path",
            "found": true,
            "nodes": [by_key.get(&start).map(node_json)],
            "edges": [],
        }));
    }

    let mut adjacency: HashMap<String, Vec<(String, String)>> = HashMap::new();
    for edge in &edges {
        adjacency
            .entry(edge.source_key.as_str().to_string())
            .or_default()
            .push((edge.target_key.as_str().to_string(), edge.edge_key.clone()));
        adjacency
            .entry(edge.target_key.as_str().to_string())
            .or_default()
            .push((edge.source_key.as_str().to_string(), edge.edge_key.clone()));
    }

    let mut queue = VecDeque::new();
    let mut visited = HashSet::new();
    let mut parent: HashMap<String, (String, String)> = HashMap::new();
    queue.push_back((start.clone(), 0usize));
    visited.insert(start.clone());
    let mut found = false;
    while let Some((current, depth)) = queue.pop_front() {
        if current == goal {
            found = true;
            break;
        }
        if depth >= max_depth {
            continue;
        }
        for (next, edge_key) in adjacency.get(&current).into_iter().flatten() {
            if visited.insert(next.clone()) {
                parent.insert(next.clone(), (current.clone(), edge_key.clone()));
                queue.push_back((next.clone(), depth + 1));
            }
        }
    }

    if !found {
        return super::response::json_structured_response(&serde_json::json!({
            "action": "path",
            "found": false,
            "from": start,
            "to": goal,
            "max_depth": max_depth,
        }));
    }

    let mut node_keys = vec![goal.clone()];
    let mut edge_keys = Vec::new();
    let mut cursor = goal.clone();
    while cursor != start {
        let Some((prev, edge_key)) = parent.get(&cursor) else {
            break;
        };
        edge_keys.push(edge_key.clone());
        node_keys.push(prev.clone());
        cursor = prev.clone();
    }
    node_keys.reverse();
    edge_keys.reverse();
    let edge_by_key = edges
        .iter()
        .map(|e| (e.edge_key.clone(), e))
        .collect::<HashMap<_, _>>();
    super::response::json_structured_response(&serde_json::json!({
        "action": "path",
        "found": true,
        "nodes": node_keys.iter().map(|k| by_key.get(k).map(node_json)).collect::<Vec<_>>(),
        "edges": edge_keys.iter().filter_map(|k| edge_by_key.get(k).map(|e| edge_json(e))).collect::<Vec<_>>(),
    }))
}

async fn action_snapshot(store: &Store, limit: usize) -> Result<serde_json::Value> {
    let (nodes, edges, _) = load_graph(store).await?;
    let generation = store.next_graph_generation().await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "snapshot",
        "generation_next": generation,
        "truncated": nodes.len() > limit || edges.len() > limit,
        "nodes": nodes.iter().take(limit).map(node_json).collect::<Vec<_>>(),
        "edges": edges.iter().take(limit).map(edge_json).collect::<Vec<_>>(),
    }))
}

async fn action_semantic(
    ctx: &AppContext,
    workspace: &rms_memory_core::workspace::Workspace,
    store: &Store,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| anyhow!("Indexer not initialized"))?;
    let defaults = SemanticGraphOptions::default();
    let options = SemanticGraphOptions {
        max_nodes: args
            .get("max_nodes")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(defaults.max_nodes),
        neighbors_per_node: args
            .get("neighbors_per_node")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize)
            .unwrap_or(defaults.neighbors_per_node),
        confidence_threshold: args
            .get("confidence_threshold")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32)
            .unwrap_or(defaults.confidence_threshold),
    };
    let mut indexer = indexer.lock().await;
    let result = build_semantic_edges(workspace, store, &mut indexer, options, None).await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "semantic",
        "nodes_considered": result.nodes_considered,
        "edges": result.edges,
    }))
}

async fn action_create_edge(
    store: &Store,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let source = args
        .get("source")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("create_edge requires `source`"))?;
    let target = args
        .get("target")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("create_edge requires `target`"))?;
    let relation = args
        .get("relation")
        .and_then(|v| v.as_str())
        .unwrap_or("links_to");
    let now = chrono::Utc::now().to_rfc3339();
    let edge = GraphEdgeRecord::new_user(
        parse_user_node_key(source)?,
        parse_user_node_key(target)?,
        EdgeRelation::new(relation)?,
        "{}".into(),
        now,
    );
    let edge_key = edge.edge_key.clone();
    store.upsert_user_graph_edge(edge).await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "create_edge",
        "edge_key": edge_key,
        "source": source,
        "target": target,
        "relation": relation,
    }))
}

async fn action_edge_override(
    store: &Store,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let edge_key = args
        .get("edge_key")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("edge override requires `edge_key`"))?;
    if edge_key.trim().is_empty() || edge_key.len() > 512 {
        return Err(anyhow!("Invalid graph edge key"));
    }
    let action = args
        .get("override_action")
        .or_else(|| args.get("edge_action"))
        .and_then(|v| v.as_str())
        .unwrap_or("suppress");
    let action = match action {
        "suppress" => EdgeOverrideAction::Suppress,
        "restore" => EdgeOverrideAction::Restore,
        other => {
            return Err(anyhow!(
                "override_action must be suppress or restore, got '{other}'"
            ));
        }
    };
    let expected_revision = args
        .get("expected_revision")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let author = args
        .get("author")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let override_record = store
        .set_graph_edge_override(edge_key, action, expected_revision, author)
        .await?;
    super::response::json_structured_response(&serde_json::json!({
        "action": "edge_override",
        "override": override_record,
    }))
}

fn escape_dot(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "")
}

async fn action_export_dot(store: &Store, limit: usize) -> Result<serde_json::Value> {
    let (nodes, edges, _) = load_graph(store).await?;
    let mut dot = String::from("digraph rms_memory {\n  rankdir=LR;\n");
    for node in nodes.iter().take(limit) {
        let label = escape_dot(&node.label);
        let id = escape_dot(node.node_key.as_str());
        dot.push_str(&format!("  \"{id}\" [label=\"{label}\"];\n"));
    }
    for edge in edges.iter().take(limit) {
        dot.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\"];\n",
            escape_dot(edge.source_key.as_str()),
            escape_dot(edge.target_key.as_str()),
            escape_dot(edge.relation.as_str())
        ));
    }
    dot.push_str("}\n");
    super::response::json_structured_response(&serde_json::json!({
        "action": "export_dot",
        "dot": dot,
        "truncated": nodes.len() > limit || edges.len() > limit,
    }))
}

/// Collect graph neighbor summaries for a search hit.
pub async fn neighbors_for_hit(
    store: &Store,
    path: &str,
    qualified_symbol: Option<&str>,
    source: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let (nodes, edges, by_key) = load_graph(store).await?;
    neighbors_from_loaded(
        &nodes,
        &edges,
        &by_key,
        path,
        qualified_symbol,
        source,
        limit,
    )
}

/// Collect graph neighbor summaries for a search hit path (vault or code file).
pub async fn neighbors_for_path(
    store: &Store,
    path: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    neighbors_for_hit(store, path, None, "vault", limit).await
}

fn neighbors_from_loaded(
    nodes: &[GraphNodeRecord],
    edges: &[GraphEdgeRecord],
    by_key: &HashMap<String, GraphNodeRecord>,
    path: &str,
    qualified_symbol: Option<&str>,
    source: &str,
    limit: usize,
) -> Result<Vec<serde_json::Value>> {
    let prefer_corpus = match source {
        "code" => Some("code"),
        "vault" => Some("vault"),
        _ => None,
    };
    let Ok(node_key) = resolve_node_key_for_hit(nodes, path, qualified_symbol, prefer_corpus)
    else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for edge in edges {
        if out.len() >= limit {
            break;
        }
        let (direction, neighbor_key) = if edge.source_key.as_str() == node_key {
            ("out", edge.target_key.as_str())
        } else if edge.target_key.as_str() == node_key {
            ("in", edge.source_key.as_str())
        } else {
            continue;
        };
        out.push(serde_json::json!({
            "direction": direction,
            "relation": edge.relation.as_str(),
            "edge_key": edge.edge_key,
            "neighbor": by_key.get(neighbor_key).map(node_json).unwrap_or_else(|| {
                serde_json::json!({ "node_key": neighbor_key })
            }),
        }));
    }
    Ok(out)
}

/// Load the durable graph once and attach neighbors to many search hits.
pub async fn attach_neighbors_to_hits(
    store: &Store,
    results: &mut [crate::tools::search::UnifiedSearchResult],
    limit_per_hit: usize,
) -> Result<()> {
    let (nodes, edges, by_key) = load_graph(store).await?;
    for result in results {
        let neighbors = neighbors_from_loaded(
            &nodes,
            &edges,
            &by_key,
            &result.path,
            result.qualified_symbol.as_deref(),
            &result.source,
            limit_per_hit,
        )?;
        if !neighbors.is_empty() {
            result.graph_neighbors = Some(neighbors);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rms_memory_index::graph::{EdgeOrigin, EdgeResolution, derived_edge_key};

    #[tokio::test]
    async fn neighbors_and_path_over_reconciled_vault_graph() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("a.md"),
            "---\nid: doc-a\n---\n[Read B](b.md)\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("b.md"), "---\nid: doc-b\n---\n# B\n").unwrap();
        let workspace = rms_memory_core::workspace::Workspace {
            root: directory.path().to_path_buf(),
            code_path: directory.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: rms_memory_core::workspace::CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
        };
        let store = Store::init(&directory.path().join("db").to_string_lossy(), "memory")
            .await
            .unwrap();
        vault_graph::reconcile_vault_links(&workspace, &store)
            .await
            .unwrap();

        let neighbors = neighbors_for_path(&store, "a.md", 8).await.unwrap();
        assert_eq!(neighbors.len(), 1);
        assert_eq!(neighbors[0]["direction"], "out");
        assert_eq!(neighbors[0]["neighbor"]["path"], "b.md");

        let path = action_path(&store, "a.md", "b.md", 4).await.unwrap();
        let payload = path.get("structuredContent").cloned().unwrap_or_else(|| {
            let text = path["content"][0]["text"].as_str().unwrap();
            serde_json::from_str(text).unwrap()
        });
        assert_eq!(payload["found"], true);
        assert_eq!(payload["nodes"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn create_edge_requires_user_origin_key() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::init(&directory.path().join("db").to_string_lossy(), "memory")
            .await
            .unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        let edge = GraphEdgeRecord::new_user(
            GraphNodeKey::vault("a").unwrap(),
            GraphNodeKey::vault("b").unwrap(),
            EdgeRelation::new("links_to").unwrap(),
            "{}".into(),
            now,
        );
        assert!(edge.edge_key.starts_with("user:"));
        store.upsert_user_graph_edge(edge).await.unwrap();
    }

    #[test]
    fn resolve_prefers_file_node_over_symbol_for_shared_path() {
        let file = GraphNodeRecord {
            node_key: GraphNodeKey::code("structure:file:src/lib.rs").unwrap(),
            corpus: "code".into(),
            source_id: "structure:file:src/lib.rs".into(),
            kind: "file".into(),
            label: "src/lib.rs".into(),
            path: Some("src/lib.rs".into()),
            metadata_json: "{}".into(),
            generation: Some(1),
            updated_at: "now".into(),
        };
        let symbol = GraphNodeRecord {
            node_key: GraphNodeKey::code("item-1").unwrap(),
            corpus: "code".into(),
            source_id: "item-1".into(),
            kind: "function".into(),
            label: "example::foo".into(),
            path: Some("src/lib.rs".into()),
            metadata_json: "{}".into(),
            generation: Some(1),
            updated_at: "now".into(),
        };
        // Symbol first in the list — prefer file when resolving a bare path.
        let nodes = vec![symbol.clone(), file.clone()];
        assert_eq!(
            resolve_node_key_for_hit(&nodes, "src/lib.rs", None, Some("code")).unwrap(),
            file.node_key.as_str()
        );
        assert_eq!(
            resolve_node_key_for_hit(&nodes, "src/lib.rs", Some("example::foo"), Some("code"))
                .unwrap(),
            symbol.node_key.as_str()
        );
    }

    #[test]
    fn parse_user_node_key_rejects_malformed() {
        assert!(parse_user_node_key("").is_err());
        assert!(parse_user_node_key("no-colon").is_err());
        assert!(parse_user_node_key("weird:id").is_err());
        assert!(parse_user_node_key("vault:").is_err());
        assert_eq!(
            parse_user_node_key("vault:doc-a").unwrap().as_str(),
            "vault:doc-a"
        );
    }

    #[test]
    fn resolve_node_key_matches_path_and_id() {
        let source = GraphNodeKey::vault("doc-a").unwrap();
        let nodes = vec![GraphNodeRecord {
            node_key: source.clone(),
            corpus: "vault".into(),
            source_id: "doc-a".into(),
            kind: "note".into(),
            label: "a".into(),
            path: Some("docs/a.md".into()),
            metadata_json: "{}".into(),
            generation: Some(1),
            updated_at: "now".into(),
        }];
        assert_eq!(
            resolve_node_key(&nodes, "docs/a.md").unwrap(),
            source.as_str()
        );
        assert_eq!(resolve_node_key(&nodes, "doc-a").unwrap(), source.as_str());
        let _ = derived_edge_key;
        let _ = EdgeOrigin::Derived;
        let _ = EdgeResolution::Resolved;
    }
}
