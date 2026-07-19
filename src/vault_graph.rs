use anyhow::Result;
use std::collections::HashMap;

const EXTRACTOR: &str = "markdown-links-v1";

struct VaultDocument {
    id: String,
    path: String,
    title: String,
    doc_type: String,
    links: Vec<String>,
}

/// Purge graph identities that were persisted before the Wiki namespace was
/// reserved for GUI-generated material.
pub async fn purge_wiki_graph_records(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
) -> Result<usize> {
    let tables = store.open_or_create_graph_tables().await?;
    let stale = store
        .query_graph_nodes(&tables, 0)
        .await?
        .into_iter()
        .filter(|node| match (node.corpus.as_str(), node.path.as_deref()) {
            ("vault", Some(path)) => crate::path_policy::is_vault_wiki_relative_path(path),
            ("code", Some(path)) => crate::path_policy::is_vault_wiki_path(
                &workspace.root,
                &workspace.code_path.join(path),
            ),
            _ => false,
        })
        .map(|node| node.node_key.as_str().to_string())
        .collect::<Vec<_>>();
    store.delete_graph_nodes_and_edges(&tables, &stale).await?;
    Ok(stale.len())
}

/// Rebuild the Markdown-link projection of the graph. This scan is read-only:
/// missing frontmatter ids use the same deterministic path-derived identity as
/// the Vault index and malformed documents are skipped.
pub async fn reconcile_vault_links(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
) -> Result<()> {
    let generation = store.next_graph_generation().await?;
    let mut documents = Vec::new();
    for path in workspace.find_markdown_files()? {
        let document = match crate::document::Document::parse(&path) {
            Ok(document) => document,
            Err(error) => {
                tracing::warn!(
                    "Skipping invalid Markdown graph source {}: {error:#}",
                    path.display()
                );
                continue;
            }
        };
        let relative = path
            .strip_prefix(&workspace.root)
            .unwrap_or(&path)
            .to_string_lossy()
            .replace('\\', "/");
        let id = document.index_id(std::path::Path::new(&relative));
        documents.push(VaultDocument {
            id,
            title: std::path::Path::new(&relative)
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string(),
            doc_type: document
                .frontmatter
                .as_ref()
                .and_then(|frontmatter| frontmatter.doc_type.clone())
                .unwrap_or_else(|| "note".to_string()),
            links: document.extract_links(),
            path: relative,
        });
    }

    let paths = documents
        .iter()
        .map(|document| (document.path.clone(), document.id.clone()))
        .collect::<HashMap<_, _>>();
    let mut nodes = HashMap::new();
    let mut edges = HashMap::new();
    for document in &documents {
        let source_key = crate::graph::GraphNodeKey::vault(&document.id)?;
        insert_node(
            &mut nodes,
            crate::graph::GraphNodeRecord {
                node_key: source_key.clone(),
                corpus: "vault".to_string(),
                source_id: document.id.clone(),
                kind: document.doc_type.clone(),
                label: document.title.clone(),
                path: Some(document.path.clone()),
                metadata_json: serde_json::json!({ "source": "markdown" }).to_string(),
                generation: Some(generation),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        for link in &document.links {
            let normalized = crate::indexer::normalize_link(
                &workspace.root,
                &workspace.root.join(&document.path),
                link,
            )
            .replace('\\', "/");
            let (target_key, resolution) = match paths.get(&normalized) {
                Some(id) => (
                    crate::graph::GraphNodeKey::vault(id)?,
                    crate::graph::EdgeResolution::Resolved,
                ),
                None => (
                    crate::graph::GraphNodeKey::external(&format!("vault-link:{normalized}"))?,
                    crate::graph::EdgeResolution::Unresolved,
                ),
            };
            if !nodes.contains_key(target_key.as_str()) {
                insert_node(
                    &mut nodes,
                    crate::graph::GraphNodeRecord {
                        node_key: target_key.clone(),
                        corpus: if matches!(resolution, crate::graph::EdgeResolution::Resolved) {
                            "vault".to_string()
                        } else {
                            "external".to_string()
                        },
                        source_id: target_key.as_str().split_once(':').unwrap().1.to_string(),
                        kind: "linked_document".to_string(),
                        label: normalized.clone(),
                        path: None,
                        metadata_json: serde_json::json!({ "source": "markdown" }).to_string(),
                        generation: Some(generation),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
            }
            let relation = crate::graph::EdgeRelation::new("links_to")?;
            let edge_key =
                crate::graph::derived_edge_key(EXTRACTOR, &source_key, &target_key, &relation)?;
            edges.insert(
                edge_key.clone(),
                crate::graph::GraphEdgeRecord {
                    edge_key,
                    source_key: source_key.clone(),
                    target_key,
                    relation,
                    origin: crate::graph::EdgeOrigin::Derived,
                    extractor: Some(EXTRACTOR.to_string()),
                    resolution,
                    confidence: None,
                    generation: Some(generation),
                    metadata_json: serde_json::json!({ "source": "markdown" }).to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            );
        }
    }
    store
        .reconcile_derived_graph(
            EXTRACTOR,
            generation,
            nodes.into_values().collect(),
            edges.into_values().collect(),
        )
        .await
}

fn insert_node(
    nodes: &mut HashMap<String, crate::graph::GraphNodeRecord>,
    node: crate::graph::GraphNodeRecord,
) {
    nodes.insert(node.node_key.as_str().to_string(), node);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn markdown_link_resolves_to_stable_vault_node() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(
            directory.path().join("a.md"),
            "---\nid: doc-a\n---\n[Read B](b.md) and [Missing](missing.md)\n",
        )
        .unwrap();
        std::fs::write(directory.path().join("b.md"), "---\nid: doc-b\n---\n# B\n").unwrap();
        let workspace = crate::workspace::Workspace {
            root: directory.path().to_path_buf(),
            code_path: directory.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: crate::workspace::CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
        };
        let store =
            crate::store::Store::init(&directory.path().join("db").to_string_lossy(), "memory")
                .await
                .unwrap();
        reconcile_vault_links(&workspace, &store).await.unwrap();
        let relation = crate::graph::EdgeRelation::new("links_to").unwrap();
        let source = crate::graph::GraphNodeKey::vault("doc-a").unwrap();
        let target = crate::graph::GraphNodeKey::vault("doc-b").unwrap();
        let edge_key =
            crate::graph::derived_edge_key(EXTRACTOR, &source, &target, &relation).unwrap();
        let tables = store.open_or_create_graph_tables().await.unwrap();
        assert_eq!(
            tables
                .edges
                .count_rows(Some(format!("edge_key = '{edge_key}'")))
                .await
                .unwrap(),
            1
        );
        let missing = crate::graph::GraphNodeKey::external("vault-link:missing.md").unwrap();
        let missing_key =
            crate::graph::derived_edge_key(EXTRACTOR, &source, &missing, &relation).unwrap();
        assert_eq!(
            tables
                .edges
                .count_rows(Some(format!(
                    "edge_key = '{missing_key}' AND resolution = 'unresolved'"
                )))
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn legacy_wiki_graph_nodes_and_incident_edges_are_purged() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = crate::workspace::Workspace {
            root: directory.path().to_path_buf(),
            code_path: directory.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: crate::workspace::CodeIndexMode::Off,
            code_languages: vec!["auto".to_string()],
        };
        let store =
            crate::store::Store::init(&directory.path().join("db").to_string_lossy(), "memory")
                .await
                .unwrap();
        let generation = 1;
        let canonical_key = crate::graph::GraphNodeKey::vault("canonical").unwrap();
        let wiki_key = crate::graph::GraphNodeKey::vault("generated-wiki").unwrap();
        let node = |key: crate::graph::GraphNodeKey, path: &str| crate::graph::GraphNodeRecord {
            node_key: key.clone(),
            corpus: "vault".to_string(),
            source_id: key.as_str().split_once(':').unwrap().1.to_string(),
            kind: "note".to_string(),
            label: path.to_string(),
            path: Some(path.to_string()),
            metadata_json: "{}".to_string(),
            generation: Some(generation),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        let relation = crate::graph::EdgeRelation::new("links_to").unwrap();
        let edge_key =
            crate::graph::derived_edge_key("legacy-test", &canonical_key, &wiki_key, &relation)
                .unwrap();
        store
            .reconcile_derived_graph(
                "legacy-test",
                generation,
                vec![
                    node(canonical_key.clone(), "docs/canonical.md"),
                    node(wiki_key.clone(), "wiki/.generation/page.md"),
                ],
                vec![crate::graph::GraphEdgeRecord {
                    edge_key,
                    source_key: canonical_key.clone(),
                    target_key: wiki_key.clone(),
                    relation,
                    origin: crate::graph::EdgeOrigin::Derived,
                    extractor: Some("legacy-test".to_string()),
                    resolution: crate::graph::EdgeResolution::Resolved,
                    confidence: None,
                    generation: Some(generation),
                    metadata_json: "{}".to_string(),
                    created_at: chrono::Utc::now().to_rfc3339(),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                }],
            )
            .await
            .unwrap();

        assert_eq!(
            purge_wiki_graph_records(&workspace, &store).await.unwrap(),
            1
        );
        let tables = store.open_or_create_graph_tables().await.unwrap();
        let nodes = store.query_graph_nodes(&tables, 0).await.unwrap();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_key, canonical_key);
        assert!(
            store
                .query_graph_edges(&tables, 0)
                .await
                .unwrap()
                .is_empty()
        );
    }
}
