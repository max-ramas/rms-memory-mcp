use anyhow::{Context, Result};
use ignore::WalkBuilder;
use std::collections::{HashMap, HashSet};
use std::path::Path;

const MAX_CODE_FILE_BYTES: u64 = 512 * 1024;
const EMBEDDING_BATCH_SIZE: usize = 32;
/// A deterministic projection that makes the code graph navigable at every
/// scale: project -> folder -> file -> parsed symbol.  This stays independent
/// from language extractors, which own only syntactic relation hints.
const STRUCTURE_EXTRACTOR: &str = "code-structure-v1";
const HARD_EXCLUDED_DIRS: &[&str] = &[
    ".git",
    ".rms-memory",
    ".next",
    ".nuxt",
    "node_modules",
    "target",
    "vendor",
    "coverage",
];

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct CodeIndexStats {
    pub files_scanned: usize,
    pub files_indexed: usize,
    pub items_indexed: usize,
    pub segments_indexed: usize,
    pub segments_embedded: usize,
    pub segments_reused: usize,
    pub segments_deleted: usize,
    pub files_skipped: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeIndexStatus {
    Completed,
    Busy,
}

const CODE_INDEX_MARKER: &str = ".code-index.updated";

/// Remove code segments written by older versions for files inside the
/// reserved Vault Wiki namespace.
pub async fn purge_wiki_code_records(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
) -> Result<usize> {
    let stale = store
        .indexed_code_file_paths()
        .await?
        .into_iter()
        .filter(|path| {
            crate::path_policy::is_vault_wiki_path(&workspace.root, &workspace.code_path.join(path))
        })
        .collect::<Vec<_>>();
    store.delete_code_file_paths(&stale).await?;
    Ok(stale.len())
}

pub async fn index_code_full(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut crate::indexer::Indexer,
) -> Result<CodeIndexStats> {
    let _lock = crate::index_lock::acquire(&store.storage_path).await?;
    let stats = index_code_full_inner(workspace, store, indexer).await?;
    mark_code_index_completed(&store.storage_path)?;
    Ok(stats)
}

pub async fn try_index_code(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut crate::indexer::Indexer,
) -> Result<CodeIndexStatus> {
    let Some(_lock) = crate::index_lock::try_acquire(&store.storage_path)? else {
        return Ok(CodeIndexStatus::Busy);
    };
    index_code_full_inner(workspace, store, indexer).await?;
    mark_code_index_completed(&store.storage_path)?;
    Ok(CodeIndexStatus::Completed)
}

pub fn code_index_is_fresh_since(storage_path: &str, observed_at: std::time::SystemTime) -> bool {
    completed_marker_time(storage_path).is_ok_and(|completed_at| completed_at >= observed_at)
}

pub fn code_index_is_initialized(storage_path: &str) -> bool {
    std::path::Path::new(storage_path)
        .join(CODE_INDEX_MARKER)
        .is_file()
}

fn mark_code_index_completed(storage_path: &str) -> Result<()> {
    let marker = std::path::Path::new(storage_path).join(CODE_INDEX_MARKER);
    std::fs::write(marker, chrono::Utc::now().to_rfc3339())?;
    Ok(())
}

/// Prefer the timestamp written by the process that completed the generation.
/// Filesystem mtimes may be rounded below an observed event timestamp on CI or
/// networked filesystems, which would otherwise cause a duplicate reindex.
/// Existing marker files without a parseable payload retain their historical
/// mtime-based behavior.
fn completed_marker_time(storage_path: &str) -> Result<std::time::SystemTime> {
    let marker = std::path::Path::new(storage_path).join(CODE_INDEX_MARKER);
    if let Ok(contents) = std::fs::read_to_string(&marker)
        && let Ok(completed_at) = chrono::DateTime::parse_from_rfc3339(contents.trim())
    {
        return Ok(completed_at.with_timezone(&chrono::Utc).into());
    }
    Ok(std::fs::metadata(marker)?.modified()?)
}

async fn index_code_full_inner(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    indexer: &mut crate::indexer::Indexer,
) -> Result<CodeIndexStats> {
    let root = &workspace.code_path;
    let mut stats = CodeIndexStats::default();
    let mut pending = Vec::new();
    let (table, table_created) = store.open_or_create_code_table().await?;
    let previous = store.stored_code_segments(&table).await?;
    let graph_generation = store.next_graph_generation().await?;
    let mut graph_nodes = HashMap::new();
    let mut graph_edges = HashMap::new();
    let mut structure_nodes = HashMap::new();
    let mut structure_edges = HashMap::new();
    for language in crate::code_parser::LanguageId::ALL {
        graph_edges.insert(language.extractor_version().to_string(), HashMap::new());
    }

    let project_source_id = format!(
        "structure:project:{}",
        blake3::hash(root.to_string_lossy().as_bytes()).to_hex()
    );
    let project_key = crate::graph::GraphNodeKey::code(&project_source_id)?;
    let project_label = root
        .file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy()
        .to_string();
    insert_graph_node(
        &mut structure_nodes,
        crate::graph::GraphNodeRecord {
            node_key: project_key.clone(),
            corpus: "code".to_string(),
            source_id: project_source_id,
            kind: "project".to_string(),
            label: project_label,
            path: Some(String::new()),
            metadata_json: serde_json::json!({ "role": "project" }).to_string(),
            generation: Some(graph_generation),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    );

    let mut walker = WalkBuilder::new(root);
    walker
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .parents(true);
    for entry in walker.build() {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                tracing::warn!("Skipping unreadable code path: {error}");
                stats.files_skipped += 1;
                continue;
            }
        };
        let path = entry.path();
        if crate::path_policy::is_vault_wiki_path(&workspace.root, path)
            || is_hard_excluded(path)
            || !is_indexable_code_path(path, &workspace.code_languages)
        {
            continue;
        }
        stats.files_scanned += 1;
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) if metadata.len() <= MAX_CODE_FILE_BYTES => metadata,
            Ok(_) => {
                stats.files_skipped += 1;
                continue;
            }
            Err(error) => {
                tracing::warn!("Cannot stat {}: {error}", path.display());
                stats.files_skipped += 1;
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let source = match std::fs::read_to_string(path) {
            Ok(source) => source,
            Err(error) => {
                tracing::warn!("Cannot read {} as UTF-8: {error}", path.display());
                stats.files_skipped += 1;
                continue;
            }
        };
        let file_path = path
            .strip_prefix(root)
            .context("Code walker yielded a path outside the workspace")?
            .to_string_lossy()
            .replace('\\', "/");
        let parsed = match crate::code_parser::parse_code_file(&file_path, &source) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!("Cannot parse {}: {error}", path.display());
                stats.files_skipped += 1;
                continue;
            }
        };
        let language = parsed.language;
        let items = parsed.items;
        stats.files_indexed += 1;
        stats.items_indexed += items.len();
        let file_source_id = format!("structure:file:{file_path}");
        let file_key = crate::graph::GraphNodeKey::code(&file_source_id)?;
        insert_graph_node(
            &mut structure_nodes,
            crate::graph::GraphNodeRecord {
                node_key: file_key.clone(),
                corpus: "code".to_string(),
                source_id: file_source_id,
                kind: "file".to_string(),
                label: file_path.clone(),
                path: Some(file_path.clone()),
                metadata_json: serde_json::json!({
                    "language": language.as_str(),
                    "role": "file",
                })
                .to_string(),
                generation: Some(graph_generation),
                updated_at: chrono::Utc::now().to_rfc3339(),
            },
        );
        let mut parent_key = project_key.clone();
        let folder_parts = file_path.split('/').collect::<Vec<_>>();
        for depth in 1..folder_parts.len() {
            let folder_path = folder_parts[..depth].join("/");
            let folder_source_id = format!("structure:folder:{folder_path}");
            let folder_key = crate::graph::GraphNodeKey::code(&folder_source_id)?;
            insert_graph_node(
                &mut structure_nodes,
                crate::graph::GraphNodeRecord {
                    node_key: folder_key.clone(),
                    corpus: "code".to_string(),
                    source_id: folder_source_id,
                    kind: "folder".to_string(),
                    label: folder_path.clone(),
                    path: Some(folder_path),
                    metadata_json: serde_json::json!({ "role": "folder" }).to_string(),
                    generation: Some(graph_generation),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            );
            insert_structure_edge(
                &mut structure_edges,
                &parent_key,
                &folder_key,
                graph_generation,
            )?;
            parent_key = folder_key;
        }
        insert_structure_edge(
            &mut structure_edges,
            &parent_key,
            &file_key,
            graph_generation,
        )?;
        let item_keys = items
            .iter()
            .map(|item| item.item_key.as_str())
            .collect::<HashSet<_>>();
        for item in &items {
            let node_key = crate::graph::GraphNodeKey::code(&item.item_key)?;
            insert_graph_node(
                &mut graph_nodes,
                crate::graph::GraphNodeRecord {
                    node_key: node_key.clone(),
                    corpus: "code".to_string(),
                    source_id: item.item_key.clone(),
                    kind: item.kind.as_str().to_string(),
                    label: item.qualified_symbol.clone(),
                    path: Some(item.file_path.clone()),
                    metadata_json: serde_json::json!({
                        "language": language.as_str(),
                        "module_path": item.module_path,
                    })
                    .to_string(),
                    generation: Some(graph_generation),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            );
            insert_structure_edge(&mut structure_edges, &file_key, &node_key, graph_generation)?;
            pending.extend(
                crate::code_parser::split_with_preamble(item)
                    .into_iter()
                    .map(|segment| PendingCodeChunk {
                        id: blake3::hash(
                            format!("{}\0{}", item.item_key, segment.segment_index).as_bytes(),
                        )
                        .to_string(),
                        item_key: item.item_key.clone(),
                        file_path: item.file_path.clone(),
                        module_path: item.module_path.clone(),
                        symbol_name: item.symbol_name.clone(),
                        qualified_symbol: item.qualified_symbol.clone(),
                        kind: item.kind.as_str().to_string(),
                        start_line: item.start_line,
                        end_line: item.end_line,
                        item_hash: item.item_hash.clone(),
                        segment_index: segment.segment_index,
                        content_hash: segment.content_hash,
                        content: segment.content,
                        language: language.as_str().to_string(),
                    }),
            );
        }
        for hint in parsed.relation_hints {
            let source_key = crate::graph::GraphNodeKey::code(&hint.source_item_key)?;
            if !item_keys.contains(hint.source_item_key.as_str()) {
                insert_graph_node(
                    &mut graph_nodes,
                    crate::graph::GraphNodeRecord {
                        node_key: source_key.clone(),
                        corpus: "code".to_string(),
                        source_id: hint.source_item_key.clone(),
                        kind: format!("{}_module", language.as_str()),
                        label: file_path.clone(),
                        path: Some(file_path.clone()),
                        metadata_json: serde_json::json!({
                            "language": language.as_str(),
                            "synthetic": true,
                        })
                        .to_string(),
                        generation: Some(graph_generation),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
            }
            let target_key = crate::graph::GraphNodeKey::external(&hint.target_identifier)?;
            insert_graph_node(
                &mut graph_nodes,
                crate::graph::GraphNodeRecord {
                    node_key: target_key.clone(),
                    corpus: "external".to_string(),
                    source_id: hint.target_identifier.clone(),
                    kind: format!("{}_symbol_hint", language.as_str()),
                    label: hint.target_identifier.clone(),
                    path: None,
                    metadata_json: serde_json::json!({ "language": language.as_str() }).to_string(),
                    generation: Some(graph_generation),
                    updated_at: chrono::Utc::now().to_rfc3339(),
                },
            );
            let relation = crate::graph::EdgeRelation::new(&hint.relation)?;
            let extractor = language.extractor_version().to_string();
            let edge_key =
                crate::graph::derived_edge_key(&extractor, &source_key, &target_key, &relation)?;
            graph_edges
                .get_mut(&extractor)
                .expect("every supported language has a graph extractor bucket")
                .insert(
                    edge_key.clone(),
                    crate::graph::GraphEdgeRecord {
                        edge_key,
                        source_key,
                        target_key,
                        relation,
                        origin: crate::graph::EdgeOrigin::Derived,
                        extractor: Some(extractor),
                        resolution: crate::graph::EdgeResolution::Unresolved,
                        confidence: None,
                        generation: Some(graph_generation),
                        metadata_json: serde_json::json!({
                            "language": language.as_str(),
                            "syntactic_hint": true,
                        })
                        .to_string(),
                        created_at: chrono::Utc::now().to_rfc3339(),
                        updated_at: chrono::Utc::now().to_rfc3339(),
                    },
                );
        }
    }

    // Tree-sitter extraction can surface the same semantic item through more than
    // one syntactic path. Lance merge-insert rejects duplicate source keys, so
    // make the stable chunk-ID contract explicit before batching writes.
    pending.sort_by(|left, right| left.id.cmp(&right.id));
    let mut unique_pending: Vec<PendingCodeChunk> = Vec::with_capacity(pending.len());
    let mut first_content_by_id = HashMap::new();
    for mut chunk in pending {
        if let Some(existing_hash) = first_content_by_id.get(&chunk.id) {
            if existing_hash == &chunk.content_hash {
                continue;
            }

            // Preserve normal stable IDs, but keep both records when a parser
            // identity collision represents different content. The suffix is
            // deterministic for that content and prevents one malformed item
            // from aborting the entire project index.
            let colliding_id = chunk.id.clone();
            chunk.id = blake3::hash(format!("{colliding_id}\0{}", chunk.content_hash).as_bytes())
                .to_string();
        } else {
            first_content_by_id.insert(chunk.id.clone(), chunk.content_hash.clone());
        }
        unique_pending.push(chunk);
    }
    let pending = unique_pending;

    let timestamp = chrono::Utc::now().to_rfc3339();
    for batch in pending.chunks(EMBEDDING_BATCH_SIZE) {
        let changed = batch
            .iter()
            .enumerate()
            .filter(|(_, chunk)| {
                previous
                    .get(&chunk.id)
                    .is_none_or(|stored| stored.content_hash != chunk.content_hash)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let embedded = if changed.is_empty() {
            Vec::new()
        } else {
            indexer.embed(
                &changed
                    .iter()
                    .map(|index| batch[*index].content.clone())
                    .collect::<Vec<_>>(),
            )?
        };
        let mut embedded = embedded.into_iter();
        let records = batch
            .iter()
            .cloned()
            .map(|chunk| {
                let vector = match previous.get(&chunk.id) {
                    Some(stored) if stored.content_hash == chunk.content_hash => {
                        stats.segments_reused += 1;
                        stored.vector.clone()
                    }
                    _ => {
                        stats.segments_embedded += 1;
                        embedded
                            .next()
                            .expect("embedding result count must match changed code segments")
                    }
                };
                crate::store::CodeChunkRecord {
                    id: chunk.id,
                    item_key: chunk.item_key,
                    file_path: chunk.file_path,
                    module_path: chunk.module_path,
                    symbol_name: chunk.symbol_name,
                    qualified_symbol: chunk.qualified_symbol,
                    kind: chunk.kind,
                    language: chunk.language,
                    start_line: chunk.start_line,
                    end_line: chunk.end_line,
                    segment_index: chunk.segment_index,
                    item_hash: chunk.item_hash,
                    content_hash: chunk.content_hash,
                    content: chunk.content,
                    timestamp: Some(timestamp.clone()),
                    vector,
                }
            })
            .collect::<Vec<_>>();
        stats.segments_indexed += records.len();
        store.upsert_code_batch(&table, records).await?;
    }
    let current_ids = pending
        .iter()
        .map(|chunk| chunk.id.as_str())
        .collect::<HashSet<_>>();
    let stale = previous
        .keys()
        .filter(|id| !current_ids.contains(id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    stats.segments_deleted = stale.len();
    store.delete_code_segments(&table, &stale).await?;
    if table_created {
        store.create_code_fts_index(&table).await?;
    }
    let graph_nodes = graph_nodes.into_values().collect::<Vec<_>>();
    for (extractor, edges) in graph_edges {
        store
            .reconcile_derived_graph(
                &extractor,
                graph_generation,
                graph_nodes.clone(),
                edges.into_values().collect(),
            )
            .await?;
    }
    store
        .reconcile_derived_graph(
            STRUCTURE_EXTRACTOR,
            graph_generation,
            structure_nodes.into_values().collect(),
            structure_edges.into_values().collect(),
        )
        .await?;
    crate::vault_graph::purge_wiki_graph_records(workspace, store).await?;
    Ok(stats)
}

fn insert_structure_edge(
    edges: &mut HashMap<String, crate::graph::GraphEdgeRecord>,
    source_key: &crate::graph::GraphNodeKey,
    target_key: &crate::graph::GraphNodeKey,
    generation: u64,
) -> Result<()> {
    let relation = crate::graph::EdgeRelation::new("contains")?;
    let edge_key =
        crate::graph::derived_edge_key(STRUCTURE_EXTRACTOR, source_key, target_key, &relation)?;
    edges.insert(
        edge_key.clone(),
        crate::graph::GraphEdgeRecord {
            edge_key,
            source_key: source_key.clone(),
            target_key: target_key.clone(),
            relation,
            origin: crate::graph::EdgeOrigin::Derived,
            extractor: Some(STRUCTURE_EXTRACTOR.to_string()),
            resolution: crate::graph::EdgeResolution::Resolved,
            confidence: Some(1.0),
            generation: Some(generation),
            metadata_json: serde_json::json!({ "source": "code_structure" }).to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        },
    );
    Ok(())
}

fn insert_graph_node(
    nodes: &mut HashMap<String, crate::graph::GraphNodeRecord>,
    node: crate::graph::GraphNodeRecord,
) {
    nodes.insert(node.node_key.as_str().to_string(), node);
}

#[derive(Clone)]
struct PendingCodeChunk {
    id: String,
    item_key: String,
    file_path: String,
    module_path: String,
    symbol_name: String,
    qualified_symbol: String,
    kind: String,
    start_line: u32,
    end_line: u32,
    segment_index: u32,
    item_hash: String,
    content_hash: String,
    content: String,
    language: String,
}

pub fn is_supported_code_path(path: &Path) -> bool {
    crate::code_parser::language_for_path(path).is_some()
}

pub fn is_indexable_code_path(path: &Path, configured_languages: &[String]) -> bool {
    crate::code_parser::language_for_path(path).is_some_and(|language| {
        crate::code_parser::language_is_enabled(language, configured_languages)
    })
}

fn is_hard_excluded(path: &Path) -> bool {
    path.components().any(|component| {
        HARD_EXCLUDED_DIRS
            .iter()
            .any(|excluded| component.as_os_str() == *excluded)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn supported_source_files_outside_hard_excluded_dirs_are_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let rust_file = directory.path().join("lib.rs");
        let go_file = directory.path().join("main.go");
        let markdown_file = directory.path().join("README.md");
        std::fs::write(&rust_file, "pub fn candidate() {}\n").unwrap();
        std::fs::write(&go_file, "package main\n").unwrap();
        std::fs::write(&markdown_file, "# Not code\n").unwrap();
        assert!(is_supported_code_path(&rust_file));
        assert!(is_supported_code_path(&go_file));
        assert!(!is_supported_code_path(&markdown_file));
        assert!(!is_supported_code_path(Path::new("src/lib.rs.bak")));
        assert!(is_indexable_code_path(
            Path::new("src/main.go"),
            &["go".to_string()]
        ));
        assert!(!is_indexable_code_path(
            Path::new("src/main.rs"),
            &["go".to_string()]
        ));
        assert!(is_hard_excluded(Path::new("target/debug/build.rs")));
        assert!(is_hard_excluded(Path::new("vendor/crate/lib.rs")));
        assert!(is_hard_excluded(Path::new("frontend/.next/server/page.js")));
        assert!(!is_hard_excluded(Path::new("internal/build/steps.go")));
        assert!(is_hard_excluded(Path::new(".git/hooks/pre-commit.rs")));
        assert!(!is_hard_excluded(Path::new("src/lib.rs")));
    }

    #[test]
    fn code_schema_includes_stable_segment_identity_fields() {
        let schema = crate::store::Store::code_schema();
        for field in [
            "id",
            "item_key",
            "segment_index",
            "item_hash",
            "content_hash",
            "content",
            "vector",
        ] {
            assert!(schema.column_with_name(field).is_some(), "missing {field}");
        }
    }

    #[tokio::test]
    async fn code_table_persists_a_segment_record() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::store::Store::init(&directory.path().to_string_lossy(), "memory")
            .await
            .unwrap();
        let table = store.recreate_code_table().await.unwrap();
        store
            .insert_code_batch(
                &table,
                vec![crate::store::CodeChunkRecord {
                    id: "segment-id".to_string(),
                    item_key: "item-key".to_string(),
                    file_path: "src/lib.rs".to_string(),
                    module_path: String::new(),
                    symbol_name: "example".to_string(),
                    qualified_symbol: "example".to_string(),
                    kind: "function".to_string(),
                    language: "rust".to_string(),
                    start_line: 1,
                    end_line: 2,
                    segment_index: 0,
                    item_hash: "item-hash".to_string(),
                    content_hash: "content-hash".to_string(),
                    content: "pub fn example() {}".to_string(),
                    timestamp: None,
                    vector: vec![0.0; crate::store::VECTOR_DIMENSION],
                }],
            )
            .await
            .unwrap();
        assert_eq!(table.count_rows(None).await.unwrap(), 1);
        let results = store
            .search_code(vec![0.0; crate::store::VECTOR_DIMENSION], 10)
            .await
            .unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].qualified_symbol, "example");
        assert_eq!(results[0].language, "rust");
        assert_eq!(results[0].segment_index, 0);
    }

    #[tokio::test]
    async fn legacy_wiki_code_segments_are_purged_by_path() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = crate::workspace::Workspace {
            root: directory.path().to_path_buf(),
            code_path: directory.path().to_path_buf(),
            include: vec!["**/*.md".to_string()],
            exclude: vec![],
            code_index_mode: crate::workspace::CodeIndexMode::Manual,
            code_languages: vec!["rust".to_string()],
        };
        let store =
            crate::store::Store::init(&directory.path().join("db").to_string_lossy(), "memory")
                .await
                .unwrap();
        let table = store.recreate_code_table().await.unwrap();
        let record = |id: &str, file_path: &str| crate::store::CodeChunkRecord {
            id: id.to_string(),
            item_key: format!("item-{id}"),
            file_path: file_path.to_string(),
            module_path: String::new(),
            symbol_name: id.to_string(),
            qualified_symbol: id.to_string(),
            kind: "function".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 2,
            segment_index: 0,
            item_hash: format!("item-hash-{id}"),
            content_hash: format!("content-hash-{id}"),
            content: format!("fn {id}() {{}}"),
            timestamp: None,
            vector: vec![0.0; crate::store::VECTOR_DIMENSION],
        };
        store
            .insert_code_batch(
                &table,
                vec![
                    record("canonical", "src/lib.rs"),
                    record("generated", "wiki/.generation/generated.rs"),
                ],
            )
            .await
            .unwrap();

        assert_eq!(
            purge_wiki_code_records(&workspace, &store).await.unwrap(),
            1
        );
        assert_eq!(
            store.indexed_code_file_paths().await.unwrap(),
            vec!["src/lib.rs"]
        );
    }

    #[tokio::test]
    async fn code_segments_can_be_reused_and_orphans_removed() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::store::Store::init(&directory.path().to_string_lossy(), "memory")
            .await
            .unwrap();
        let (table, _) = store.open_or_create_code_table().await.unwrap();
        let record = crate::store::CodeChunkRecord {
            id: "stable-segment".to_string(),
            item_key: "item-key".to_string(),
            file_path: "src/lib.rs".to_string(),
            module_path: String::new(),
            symbol_name: "example".to_string(),
            qualified_symbol: "example".to_string(),
            kind: "function".to_string(),
            language: "rust".to_string(),
            start_line: 1,
            end_line: 2,
            segment_index: 0,
            item_hash: "item-hash".to_string(),
            content_hash: "unchanged-content".to_string(),
            content: "pub fn example() {}".to_string(),
            timestamp: None,
            vector: vec![0.25; crate::store::VECTOR_DIMENSION],
        };
        store
            .insert_code_batch(&table, vec![record.clone()])
            .await
            .unwrap();
        let stored = store.stored_code_segments(&table).await.unwrap();
        assert_eq!(stored["stable-segment"].content_hash, "unchanged-content");
        assert_eq!(stored["stable-segment"].vector, record.vector);

        store.upsert_code_batch(&table, vec![record]).await.unwrap();
        assert_eq!(table.count_rows(None).await.unwrap(), 1);
        store
            .delete_code_segments(&table, &["stable-segment".to_string()])
            .await
            .unwrap();
        assert_eq!(table.count_rows(None).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn code_search_returns_empty_when_the_optional_table_is_absent() {
        let directory = tempfile::tempdir().unwrap();
        let store = crate::store::Store::init(&directory.path().to_string_lossy(), "memory")
            .await
            .unwrap();
        assert!(
            store
                .search_code(vec![0.0; crate::store::VECTOR_DIMENSION], 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn completed_marker_suppresses_duplicate_generation_after_an_observed_save() {
        let directory = tempfile::tempdir().unwrap();
        let observed = std::time::SystemTime::now();
        mark_code_index_completed(&directory.path().to_string_lossy()).unwrap();
        assert!(code_index_is_initialized(
            &directory.path().to_string_lossy()
        ));
        assert!(code_index_is_fresh_since(
            &directory.path().to_string_lossy(),
            observed
        ));
    }

    #[test]
    fn legacy_unparseable_marker_falls_back_to_filesystem_mtime() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::write(directory.path().join(CODE_INDEX_MARKER), "legacy marker").unwrap();
        assert!(code_index_is_fresh_since(
            &directory.path().to_string_lossy(),
            std::time::UNIX_EPOCH
        ));
    }

    #[test]
    fn structural_projection_edges_are_stable_and_resolved() {
        let source = crate::graph::GraphNodeKey::code("structure:project:example").unwrap();
        let target = crate::graph::GraphNodeKey::code("structure:file:src/lib.rs").unwrap();
        let mut edges = HashMap::new();
        insert_structure_edge(&mut edges, &source, &target, 7).unwrap();
        let edge = edges.values().next().unwrap();
        assert_eq!(edges.len(), 1);
        assert_eq!(edge.relation.as_str(), "contains");
        assert_eq!(edge.extractor.as_deref(), Some(STRUCTURE_EXTRACTOR));
        assert_eq!(edge.resolution, crate::graph::EdgeResolution::Resolved);
        assert_eq!(edge.source_key, source);
        assert_eq!(edge.target_key, target);
    }
}
