use anyhow::{Context, Result, anyhow};
use futures::stream::StreamExt;
use lancedb::arrow::arrow_array::builder::{Float32Builder, StringBuilder, UInt64Builder};
use lancedb::arrow::arrow_array::{
    Array, RecordBatch, RecordBatchIterator, StringArray, UInt64Array,
};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use std::sync::Arc;

use crate::graph::{
    EdgeOrigin, EdgeOverrideAction, GraphEdgeOverride, GraphEdgeRecord, GraphNodeRecord,
};
use crate::store::Store;

pub struct GraphTables {
    pub nodes: Table,
    pub edges: Table,
    pub overrides: Table,
}

impl Store {
    pub async fn open_or_create_graph_tables(&self) -> Result<GraphTables> {
        Ok(GraphTables {
            nodes: self
                .open_or_create_table(&self.graph_nodes_table_name(), Self::graph_nodes_schema())
                .await?,
            edges: self
                .open_or_create_table(&self.graph_edges_table_name(), Self::graph_edges_schema())
                .await?,
            overrides: self
                .open_or_create_table(
                    &self.graph_edge_overrides_table_name(),
                    Self::graph_edge_overrides_schema(),
                )
                .await?,
        })
    }

    pub async fn next_graph_generation(&self) -> Result<u64> {
        let tables = self.open_or_create_graph_tables().await?;
        let node_max = max_generation(&tables.nodes).await?;
        let edge_max = max_generation(&tables.edges).await?;
        Ok(node_max.max(edge_max).unwrap_or(0) + 1)
    }

    async fn open_or_create_table(
        &self,
        name: &str,
        schema: Arc<lancedb::arrow::arrow_schema::Schema>,
    ) -> Result<Table> {
        match self.db.open_table(name).execute().await {
            Ok(table) => Ok(table),
            Err(open_error) => match self.db.create_empty_table(name, schema).execute().await {
                Ok(table) => Ok(table),
                Err(create_error) => self.db.open_table(name).execute().await.with_context(|| {
                    format!(
                        "could not open graph table {name} after create race; initial open error: {open_error}; create error: {create_error}"
                    )
                }),
            },
        }
    }

    /// Reconcile a complete derived graph generation. User nodes, user edges, and
    /// overrides are deliberately outside this method's ownership.
    pub async fn reconcile_derived_graph(
        &self,
        extractor: &str,
        generation: u64,
        nodes: Vec<GraphNodeRecord>,
        edges: Vec<GraphEdgeRecord>,
    ) -> Result<()> {
        let extractor = extractor.trim();
        if extractor.is_empty() {
            return Err(anyhow!("graph extractor must be non-empty"));
        }
        for edge in &edges {
            if edge.origin != EdgeOrigin::Derived
                || edge.extractor.as_deref() != Some(extractor)
                || edge.generation != Some(generation)
            {
                return Err(anyhow!(
                    "derived reconciliation accepts only edges owned by extractor {extractor} at generation {generation}"
                ));
            }
        }
        for node in &nodes {
            if node.generation != Some(generation) {
                return Err(anyhow!(
                    "derived reconciliation accepts only nodes at generation {generation}"
                ));
            }
        }

        let tables = self.open_or_create_graph_tables().await?;
        self.upsert_graph_nodes(&tables.nodes, nodes).await?;
        self.upsert_graph_edges(&tables.edges, edges).await?;

        // Upsert first and prune second: an interrupted run leaves an older,
        // usable generation instead of deleting the current graph before writing.
        tables
            .edges
            .delete(&format!(
                "origin = 'derived' AND extractor = '{}' AND generation < {}",
                escape(extractor),
                generation
            ))
            .await?;
        // Nodes are canonical identities shared by multiple extractors. The
        // node schema intentionally has no extractor owner, so pruning by this
        // generation could erase a node still referenced by another corpus.
        // A later reference-aware graph GC may remove truly orphaned nodes.
        Ok(())
    }

    pub async fn upsert_user_graph_edge(&self, edge: GraphEdgeRecord) -> Result<()> {
        if edge.origin != EdgeOrigin::User || edge.extractor.is_some() || edge.generation.is_some()
        {
            return Err(anyhow!(
                "user graph edges must not have an extractor or generation"
            ));
        }
        let tables = self.open_or_create_graph_tables().await?;
        self.upsert_graph_edges(&tables.edges, vec![edge]).await
    }

    pub async fn upsert_user_graph_node(&self, node: GraphNodeRecord) -> Result<()> {
        if node.generation.is_some() {
            return Err(anyhow!("user graph nodes must not have a generation"));
        }
        let tables = self.open_or_create_graph_tables().await?;
        self.upsert_graph_nodes(&tables.nodes, vec![node]).await
    }

    pub async fn set_graph_edge_override(
        &self,
        edge_key: &str,
        action: EdgeOverrideAction,
        expected_revision: u64,
        author: Option<String>,
    ) -> Result<GraphEdgeOverride> {
        let edge_key = edge_key.trim();
        if edge_key.is_empty() {
            return Err(anyhow!("graph edge override requires a non-empty edge key"));
        }
        let tables = self.open_or_create_graph_tables().await?;
        let now = chrono::Utc::now().to_rfc3339();
        let override_row = GraphEdgeOverride {
            edge_key: edge_key.to_string(),
            action,
            revision: expected_revision + 1,
            author,
            created_at: now.clone(),
            updated_at: now,
        };
        let batch = graph_override_batch(vec![override_row.clone()])?;
        let mut merge = tables.overrides.merge_insert(&["edge_key"]);
        merge.when_matched_update_all(Some(format!("target.revision = {expected_revision}")));
        merge.when_not_matched_insert_all();
        let result = merge
            .execute(Box::new(RecordBatchIterator::new(
                vec![Ok(batch)],
                Store::graph_edge_overrides_schema(),
            )))
            .await?;
        if result.num_inserted_rows + result.num_updated_rows != 1 {
            return Err(anyhow!(
                "GRAPH_OVERRIDE_CONFLICT: edge {edge_key} changed before revision {expected_revision}"
            ));
        }
        Ok(override_row)
    }

    pub async fn graph_edge_override(&self, edge_key: &str) -> Result<Option<GraphEdgeOverride>> {
        let tables = self.open_or_create_graph_tables().await?;
        let mut stream = tables
            .overrides
            .query()
            .only_if(format!("edge_key = '{}'", escape(edge_key)))
            .execute()
            .await?;
        let Some(batch) = stream.next().await.transpose()? else {
            return Ok(None);
        };
        if batch.num_rows() == 0 {
            return Ok(None);
        }
        let text = |name: &str| -> Result<&StringArray> {
            batch
                .column_by_name(name)
                .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                .with_context(|| format!("graph override column {name} is not a string"))
        };
        let revisions = batch
            .column_by_name("revision")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
            .context("graph override revision is not UInt64")?;
        let action = match text("action")?.value(0) {
            "suppress" => EdgeOverrideAction::Suppress,
            "restore" => EdgeOverrideAction::Restore,
            other => return Err(anyhow!("unknown graph override action {other}")),
        };
        let authors = text("author")?;
        Ok(Some(GraphEdgeOverride {
            edge_key: text("edge_key")?.value(0).to_string(),
            action,
            revision: revisions.value(0),
            author: (!authors.is_null(0)).then(|| authors.value(0).to_string()),
            created_at: text("created_at")?.value(0).to_string(),
            updated_at: text("updated_at")?.value(0).to_string(),
        }))
    }

    async fn upsert_graph_nodes(&self, table: &Table, nodes: Vec<GraphNodeRecord>) -> Result<()> {
        if nodes.is_empty() {
            return Ok(());
        }
        let batch = graph_node_batch(nodes)?;
        let mut merge = table.merge_insert(&["node_key"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(RecordBatchIterator::new(
                vec![Ok(batch)],
                Self::graph_nodes_schema(),
            )))
            .await?;
        Ok(())
    }

    async fn upsert_graph_edges(&self, table: &Table, edges: Vec<GraphEdgeRecord>) -> Result<()> {
        if edges.is_empty() {
            return Ok(());
        }
        let batch = graph_edge_batch(edges)?;
        let mut merge = table.merge_insert(&["edge_key"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(RecordBatchIterator::new(
                vec![Ok(batch)],
                Self::graph_edges_schema(),
            )))
            .await?;
        Ok(())
    }
}

fn graph_node_batch(records: Vec<GraphNodeRecord>) -> Result<RecordBatch> {
    let mut node_key = StringBuilder::new();
    let mut corpus = StringBuilder::new();
    let mut source_id = StringBuilder::new();
    let mut kind = StringBuilder::new();
    let mut label = StringBuilder::new();
    let mut path = StringBuilder::new();
    let mut metadata = StringBuilder::new();
    let mut generation = UInt64Builder::new();
    let mut updated_at = StringBuilder::new();
    for record in records {
        validate_json(&record.metadata_json)?;
        node_key.append_value(record.node_key.as_str());
        corpus.append_value(record.corpus);
        source_id.append_value(record.source_id);
        kind.append_value(record.kind);
        label.append_value(record.label);
        append_optional_string(&mut path, record.path);
        metadata.append_value(record.metadata_json);
        append_optional_u64(&mut generation, record.generation);
        updated_at.append_value(record.updated_at);
    }
    Ok(RecordBatch::try_new(
        Store::graph_nodes_schema(),
        vec![
            Arc::new(node_key.finish()),
            Arc::new(corpus.finish()),
            Arc::new(source_id.finish()),
            Arc::new(kind.finish()),
            Arc::new(label.finish()),
            Arc::new(path.finish()),
            Arc::new(metadata.finish()),
            Arc::new(generation.finish()),
            Arc::new(updated_at.finish()),
        ],
    )?)
}

fn graph_edge_batch(records: Vec<GraphEdgeRecord>) -> Result<RecordBatch> {
    let mut edge_key = StringBuilder::new();
    let mut source = StringBuilder::new();
    let mut target = StringBuilder::new();
    let mut relation = StringBuilder::new();
    let mut origin = StringBuilder::new();
    let mut extractor = StringBuilder::new();
    let mut resolution = StringBuilder::new();
    let mut confidence = Float32Builder::new();
    let mut generation = UInt64Builder::new();
    let mut metadata = StringBuilder::new();
    let mut created_at = StringBuilder::new();
    let mut updated_at = StringBuilder::new();
    for record in records {
        validate_json(&record.metadata_json)?;
        edge_key.append_value(record.edge_key);
        source.append_value(record.source_key.as_str());
        target.append_value(record.target_key.as_str());
        relation.append_value(record.relation.as_str());
        origin.append_value(match record.origin {
            EdgeOrigin::Derived => "derived",
            EdgeOrigin::User => "user",
        });
        append_optional_string(&mut extractor, record.extractor);
        resolution.append_value(match record.resolution {
            crate::graph::EdgeResolution::Resolved => "resolved",
            crate::graph::EdgeResolution::Unresolved => "unresolved",
            crate::graph::EdgeResolution::Ambiguous => "ambiguous",
        });
        match record.confidence {
            Some(value) => confidence.append_value(value),
            None => confidence.append_null(),
        };
        append_optional_u64(&mut generation, record.generation);
        metadata.append_value(record.metadata_json);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }
    Ok(RecordBatch::try_new(
        Store::graph_edges_schema(),
        vec![
            Arc::new(edge_key.finish()),
            Arc::new(source.finish()),
            Arc::new(target.finish()),
            Arc::new(relation.finish()),
            Arc::new(origin.finish()),
            Arc::new(extractor.finish()),
            Arc::new(resolution.finish()),
            Arc::new(confidence.finish()),
            Arc::new(generation.finish()),
            Arc::new(metadata.finish()),
            Arc::new(created_at.finish()),
            Arc::new(updated_at.finish()),
        ],
    )?)
}

fn graph_override_batch(records: Vec<GraphEdgeOverride>) -> Result<RecordBatch> {
    let mut edge_key = StringBuilder::new();
    let mut action = StringBuilder::new();
    let mut revision = UInt64Builder::new();
    let mut author = StringBuilder::new();
    let mut created_at = StringBuilder::new();
    let mut updated_at = StringBuilder::new();
    for record in records {
        edge_key.append_value(record.edge_key);
        action.append_value(match record.action {
            EdgeOverrideAction::Suppress => "suppress",
            EdgeOverrideAction::Restore => "restore",
        });
        revision.append_value(record.revision);
        append_optional_string(&mut author, record.author);
        created_at.append_value(record.created_at);
        updated_at.append_value(record.updated_at);
    }
    Ok(RecordBatch::try_new(
        Store::graph_edge_overrides_schema(),
        vec![
            Arc::new(edge_key.finish()),
            Arc::new(action.finish()),
            Arc::new(revision.finish()),
            Arc::new(author.finish()),
            Arc::new(created_at.finish()),
            Arc::new(updated_at.finish()),
        ],
    )?)
}

fn append_optional_string(builder: &mut StringBuilder, value: Option<String>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}
fn append_optional_u64(builder: &mut UInt64Builder, value: Option<u64>) {
    match value {
        Some(value) => builder.append_value(value),
        None => builder.append_null(),
    }
}
fn validate_json(value: &str) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(value)
        .context("graph metadata must be valid JSON")?;
    Ok(())
}
fn escape(value: &str) -> String {
    value.replace('\'', "''")
}

async fn max_generation(table: &Table) -> Result<Option<u64>> {
    use lancedb::arrow::arrow_array::Array;
    let mut stream = table
        .query()
        .select(lancedb::query::Select::Columns(vec![
            "generation".to_string(),
        ]))
        .execute()
        .await?;
    let mut maximum = None;
    while let Some(batch) = stream.next().await {
        let batch = batch?;
        let values = batch
            .column_by_name("generation")
            .and_then(|column| column.as_any().downcast_ref::<UInt64Array>())
            .context("graph generation is not UInt64")?;
        for index in 0..batch.num_rows() {
            if !values.is_null(index) {
                maximum = Some(maximum.unwrap_or(0).max(values.value(index)));
            }
        }
    }
    Ok(maximum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{
        EdgeRelation, EdgeResolution, GraphNodeKey, derived_edge_key, new_user_edge_key,
    };

    fn node(key: GraphNodeKey, generation: u64) -> GraphNodeRecord {
        GraphNodeRecord {
            source_id: key.as_str().split_once(':').unwrap().1.to_string(),
            corpus: key.as_str().split_once(':').unwrap().0.to_string(),
            kind: "code_item".to_string(),
            label: key.as_str().to_string(),
            node_key: key,
            path: None,
            metadata_json: "{}".to_string(),
            generation: Some(generation),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    fn derived_edge(
        source: GraphNodeKey,
        target: GraphNodeKey,
        generation: u64,
    ) -> GraphEdgeRecord {
        let relation = EdgeRelation::new("uses").unwrap();
        let extractor = "rust-tree-sitter-v1".to_string();
        GraphEdgeRecord {
            edge_key: derived_edge_key(&extractor, &source, &target, &relation).unwrap(),
            source_key: source,
            target_key: target,
            relation,
            origin: EdgeOrigin::Derived,
            extractor: Some(extractor),
            resolution: EdgeResolution::Unresolved,
            confidence: None,
            generation: Some(generation),
            metadata_json: "{}".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[tokio::test]
    async fn reconciliation_preserves_user_edges_and_override_cas() {
        let directory = tempfile::tempdir().unwrap();
        let store = Store::init(&directory.path().to_string_lossy(), "memory")
            .await
            .unwrap();
        let source = GraphNodeKey::code("crate::source").unwrap();
        let target = GraphNodeKey::external("crate::dependency").unwrap();
        let first = derived_edge(source.clone(), target.clone(), 1);
        store
            .reconcile_derived_graph(
                "rust-tree-sitter-v1",
                1,
                vec![node(source.clone(), 1), node(target.clone(), 1)],
                vec![first.clone()],
            )
            .await
            .unwrap();

        store
            .upsert_user_graph_node(GraphNodeRecord {
                node_key: GraphNodeKey::external("manual:note").unwrap(),
                corpus: "external".to_string(),
                source_id: "manual:note".to_string(),
                kind: "note".to_string(),
                label: "Manual note".to_string(),
                path: None,
                metadata_json: "{}".to_string(),
                generation: None,
                updated_at: chrono::Utc::now().to_rfc3339(),
            })
            .await
            .unwrap();

        let user_edge = GraphEdgeRecord {
            edge_key: new_user_edge_key(),
            source_key: source.clone(),
            target_key: target.clone(),
            relation: EdgeRelation::new("related_to").unwrap(),
            origin: EdgeOrigin::User,
            extractor: None,
            resolution: EdgeResolution::Resolved,
            confidence: None,
            generation: None,
            metadata_json: "{}".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        };
        store.upsert_user_graph_edge(user_edge).await.unwrap();
        store
            .set_graph_edge_override(
                &first.edge_key,
                EdgeOverrideAction::Suppress,
                0,
                Some("gui-user".to_string()),
            )
            .await
            .unwrap();

        let second = derived_edge(source.clone(), target.clone(), 2);
        store
            .reconcile_derived_graph(
                "rust-tree-sitter-v1",
                2,
                vec![node(source, 2), node(target, 2)],
                vec![second],
            )
            .await
            .unwrap();
        let tables = store.open_or_create_graph_tables().await.unwrap();
        assert_eq!(tables.edges.count_rows(None).await.unwrap(), 2);
        assert_eq!(tables.nodes.count_rows(None).await.unwrap(), 3);
        let override_row = store
            .graph_edge_override(&first.edge_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(override_row.action, EdgeOverrideAction::Suppress);
        assert_eq!(override_row.revision, 1);
        assert!(
            store
                .set_graph_edge_override(&first.edge_key, EdgeOverrideAction::Restore, 0, None)
                .await
                .unwrap_err()
                .to_string()
                .contains("GRAPH_OVERRIDE_CONFLICT")
        );
    }
}
