//! Ephemeral semantic graph projection.
//!
//! Similarity edges are derived on demand from the already-indexed vault/code
//! embeddings. They are intentionally never written to the graph tables: an
//! unavailable or cancelled request cannot alter the durable graph.

use anyhow::{Context, Result, anyhow};
use futures::stream::StreamExt;
use lancedb::arrow::arrow_array::{Array, StringArray};
use lancedb::query::{ExecutableQuery, QueryBase, Select};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::graph::EdgeResolution;
use crate::indexer::Indexer;
use crate::store::{CodeSearchResult, SearchResult, Store};
use crate::workspace::Workspace;

pub const EXTRACTOR: &str = "embedding-similarity-v1";
pub const RELATION: &str = "semantically_related";
pub const MAX_NODES: usize = 200;
pub const MAX_NEIGHBORS: usize = 8;

#[derive(Debug, Clone)]
pub struct SemanticGraphOptions {
    pub max_nodes: usize,
    pub neighbors_per_node: usize,
    /// Confidence is normalized to [0, 1], where 1 is an exact vector match.
    pub confidence_threshold: f32,
}

impl Default for SemanticGraphOptions {
    fn default() -> Self {
        Self {
            max_nodes: 100,
            neighbors_per_node: 5,
            confidence_threshold: 0.70,
        }
    }
}

impl SemanticGraphOptions {
    pub fn validate(&self) -> Result<()> {
        if self.max_nodes == 0 || self.max_nodes > MAX_NODES {
            return Err(anyhow!(
                "semantic graph max_nodes must be in 1..={MAX_NODES}"
            ));
        }
        if self.neighbors_per_node == 0 || self.neighbors_per_node > MAX_NEIGHBORS {
            return Err(anyhow!(
                "semantic graph neighbors_per_node must be in 1..={MAX_NEIGHBORS}"
            ));
        }
        if !self.confidence_threshold.is_finite()
            || !(0.0..=1.0).contains(&self.confidence_threshold)
        {
            return Err(anyhow!(
                "semantic graph confidence_threshold must be a finite value in 0..=1"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SemanticGraphEdge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub confidence: f32,
    pub extractor: String,
    pub resolution: EdgeResolution,
}

#[derive(Debug, Clone, Serialize)]
pub struct SemanticGraphResult {
    pub nodes_considered: usize,
    pub edges: Vec<SemanticGraphEdge>,
}

/// Build a bounded, in-memory semantic edge projection.
///
/// The cancellation flag is checked before every embedding/search operation.
/// It cannot interrupt a single embedding inference or database request, but
/// prevents subsequent work and always returns without persistence.
pub async fn build_semantic_edges(
    workspace: &Workspace,
    store: &Store,
    indexer: &mut Indexer,
    options: SemanticGraphOptions,
    cancelled: Option<&AtomicBool>,
) -> Result<SemanticGraphResult> {
    options.validate()?;
    check_cancelled(cancelled)?;

    // This read-only open deliberately avoids `open_or_create_graph_tables`,
    // which would create durable tables for a purely ephemeral request.
    let nodes = load_existing_nodes(store).await?;
    let selected = select_nodes(nodes, options.max_nodes);
    if selected.is_empty() {
        return Ok(SemanticGraphResult {
            nodes_considered: 0,
            edges: Vec::new(),
        });
    }

    let vault_by_path = selected
        .iter()
        .filter(|node| node.corpus == "vault")
        .filter_map(|node| {
            node.path
                .as_ref()
                .map(|path| (normalize_path(workspace, path), node.key.clone()))
        })
        .collect::<BTreeMap<_, _>>();
    let code_by_symbol = selected
        .iter()
        .filter(|node| node.corpus == "code")
        .filter_map(|node| {
            node.path.as_ref().map(|path| {
                (
                    (normalize_path(workspace, path), node.label.clone()),
                    node.key.clone(),
                )
            })
        })
        .collect::<BTreeMap<_, _>>();

    let mut deduped = BTreeMap::<(String, String), f32>::new();
    let queries: Vec<String> = selected.iter().map(semantic_query).collect();
    // Batch embed once instead of one ONNX call per selected node.
    let vectors = if queries.is_empty() {
        Vec::new()
    } else {
        indexer.embed(&queries)?
    };
    if vectors.len() != selected.len() {
        anyhow::bail!(
            "semantic embed returned {} vectors for {} queries",
            vectors.len(),
            selected.len()
        );
    }

    for (source, (query, vector)) in selected.iter().zip(queries.into_iter().zip(vectors)) {
        check_cancelled(cancelled)?;

        // Both corpora are queried so semantic links can cross vault/code
        // boundaries. Each query and each per-source candidate set is bounded.
        let vault_results = match store
            .search(
                vector.clone(),
                query,
                options.neighbors_per_node + 1,
                None,
            )
            .await
        {
            Ok(results) => results,
            Err(error) => {
                tracing::debug!("semantic graph vault lookup skipped: {error:#}");
                Vec::new()
            }
        };
        check_cancelled(cancelled)?;
        let code_results = match store
            .search_code(vector, options.neighbors_per_node + 1)
            .await
        {
            Ok(results) => results,
            Err(error) => {
                tracing::debug!("semantic graph code lookup skipped: {error:#}");
                Vec::new()
            }
        };
        check_cancelled(cancelled)?;

        let candidates = collect_candidates(
            CandidateContext {
                source,
                workspace,
                vault_by_path: &vault_by_path,
                code_by_symbol: &code_by_symbol,
            },
            &vault_results,
            &code_results,
            options.confidence_threshold,
            options.neighbors_per_node,
        );
        for (target, confidence) in candidates {
            let (left, right) = canonical_pair(&source.key, &target);
            deduped
                .entry((left, right))
                .and_modify(|existing| *existing = existing.max(confidence))
                .or_insert(confidence);
        }
    }

    let edges = deduped
        .into_iter()
        .map(|((source, target), confidence)| SemanticGraphEdge {
            source,
            target,
            relation: RELATION.to_string(),
            confidence,
            extractor: EXTRACTOR.to_string(),
            resolution: EdgeResolution::Resolved,
        })
        .collect();
    Ok(SemanticGraphResult {
        nodes_considered: selected.len(),
        edges,
    })
}

#[derive(Debug, Clone)]
struct ExistingNode {
    key: String,
    corpus: String,
    label: String,
    path: Option<String>,
}

async fn load_existing_nodes(store: &Store) -> Result<Vec<ExistingNode>> {
    let table = match store
        .db
        .open_table(store.graph_nodes_table_name())
        .execute()
        .await
    {
        Ok(table) => table,
        // No durable graph is a normal, empty projection. Do not create it.
        Err(_) => return Ok(Vec::new()),
    };
    let mut stream = table
        .query()
        .select(Select::Columns(vec![
            "node_key".into(),
            "corpus".into(),
            "label".into(),
            "path".into(),
        ]))
        .execute()
        .await?;
    let mut nodes = Vec::new();
    while let Some(batch) = stream.next().await.transpose()? {
        let text = |name: &str| -> Result<&StringArray> {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                .with_context(|| format!("semantic graph node column {name} is not Utf8"))
        };
        let keys = text("node_key")?;
        let corpora = text("corpus")?;
        let labels = text("label")?;
        let paths = text("path")?;
        for index in 0..batch.num_rows() {
            let corpus = corpora.value(index);
            if corpus != "vault" && corpus != "code" {
                continue;
            }
            nodes.push(ExistingNode {
                key: keys.value(index).to_string(),
                corpus: corpus.to_string(),
                label: labels.value(index).to_string(),
                path: (!paths.is_null(index)).then(|| paths.value(index).to_string()),
            });
        }
    }
    Ok(nodes)
}

fn select_nodes(mut nodes: Vec<ExistingNode>, limit: usize) -> Vec<ExistingNode> {
    nodes.sort_by(|left, right| left.key.cmp(&right.key));
    nodes.dedup_by(|left, right| left.key == right.key);
    nodes.truncate(limit);
    nodes
}

struct CandidateContext<'a> {
    source: &'a ExistingNode,
    workspace: &'a Workspace,
    vault_by_path: &'a BTreeMap<String, String>,
    code_by_symbol: &'a BTreeMap<(String, String), String>,
}

fn collect_candidates(
    context: CandidateContext<'_>,
    vault_results: &[SearchResult],
    code_results: &[CodeSearchResult],
    threshold: f32,
    limit: usize,
) -> Vec<(String, f32)> {
    let mut candidates = HashMap::<String, f32>::new();
    for result in vault_results {
        let Some(confidence) = confidence_from_distance(result.score) else {
            continue;
        };
        if confidence < threshold {
            continue;
        }
        if let Some(target) = context
            .vault_by_path
            .get(&normalize_path(context.workspace, &result.path))
            && target != &context.source.key
        {
            candidates
                .entry(target.clone())
                .and_modify(|current| *current = current.max(confidence))
                .or_insert(confidence);
        }
    }
    for result in code_results {
        let Some(confidence) = confidence_from_distance(result.score) else {
            continue;
        };
        if confidence < threshold {
            continue;
        }
        let key = (
            normalize_path(context.workspace, &result.file_path),
            result.qualified_symbol.clone(),
        );
        if let Some(target) = context.code_by_symbol.get(&key)
            && target != &context.source.key
        {
            candidates
                .entry(target.clone())
                .and_modify(|current| *current = current.max(confidence))
                .or_insert(confidence);
        }
    }
    let mut candidates = candidates.into_iter().collect::<Vec<_>>();
    candidates.sort_by(|(left_key, left_score), (right_key, right_score)| {
        right_score
            .total_cmp(left_score)
            .then_with(|| left_key.cmp(right_key))
    });
    candidates.truncate(limit);
    candidates
}

/// LanceDB vector search reports an L2-style distance; convert it to a stable
/// confidence without pretending an unbounded distance is a probability.
pub fn confidence_from_distance(distance: Option<f32>) -> Option<f32> {
    let distance = distance?;
    if !distance.is_finite() {
        return None;
    }
    Some((1.0 / (1.0 + distance.max(0.0))).clamp(0.0, 1.0))
}

fn semantic_query(node: &ExistingNode) -> String {
    match &node.path {
        Some(path) => format!("{}\n{}", node.label, path),
        None => node.label.clone(),
    }
}

fn normalize_path(workspace: &Workspace, path: &str) -> String {
    let candidate = std::path::Path::new(path);
    let path = candidate
        .strip_prefix(&workspace.root)
        .unwrap_or(candidate)
        .to_string_lossy();
    path.replace('\\', "/")
}

fn canonical_pair(left: &str, right: &str) -> (String, String) {
    if left <= right {
        (left.to_string(), right.to_string())
    } else {
        (right.to_string(), left.to_string())
    }
}

fn check_cancelled(cancelled: Option<&AtomicBool>) -> Result<()> {
    if cancelled.is_some_and(|flag| flag.load(Ordering::Acquire)) {
        return Err(anyhow!("SEMANTIC_GRAPH_CANCELLED"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_lance_distance_to_bounded_confidence() {
        assert_eq!(confidence_from_distance(Some(0.0)), Some(1.0));
        assert_eq!(confidence_from_distance(Some(1.0)), Some(0.5));
        assert_eq!(confidence_from_distance(Some(-4.0)), Some(1.0));
        assert_eq!(confidence_from_distance(Some(f32::NAN)), None);
    }

    #[test]
    fn node_selection_is_deterministic_and_bounded() {
        let nodes = vec![
            ExistingNode {
                key: "code:z".into(),
                corpus: "code".into(),
                label: "z".into(),
                path: None,
            },
            ExistingNode {
                key: "vault:a".into(),
                corpus: "vault".into(),
                label: "a".into(),
                path: None,
            },
            ExistingNode {
                key: "vault:a".into(),
                corpus: "vault".into(),
                label: "duplicate".into(),
                path: None,
            },
        ];
        let selected = select_nodes(nodes, 2);
        assert_eq!(
            selected
                .iter()
                .map(|node| node.key.as_str())
                .collect::<Vec<_>>(),
            vec!["code:z", "vault:a"]
        );
    }

    #[test]
    fn options_reject_unbounded_requests() {
        assert!(
            SemanticGraphOptions {
                max_nodes: 201,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SemanticGraphOptions {
                neighbors_per_node: 9,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            SemanticGraphOptions {
                confidence_threshold: f32::INFINITY,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn canonical_pair_deduplicates_reverse_edges() {
        assert_eq!(
            canonical_pair("vault:a", "code:b"),
            ("code:b".into(), "vault:a".into())
        );
        assert_eq!(
            canonical_pair("code:b", "vault:a"),
            ("code:b".into(), "vault:a".into())
        );
    }
}
