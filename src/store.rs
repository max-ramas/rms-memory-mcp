use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::Arc;

use lancedb::arrow::arrow_schema::{DataType, Field, Schema as ArrowSchema};
use lancedb::connect;
use lancedb::index::Index;
use lancedb::table::Table;

pub const VECTOR_DIMENSION: usize = 384;

fn escape_filter(s: &str) -> String {
    s.replace('\'', "''")
}

#[derive(Clone)]
pub struct Store {
    pub db: lancedb::Connection,
    pub table_name: String,
    pub storage_path: String,
}

impl Store {
    pub async fn init(storage_path: &str, table_name: &str) -> Result<Self> {
        let meta_path = PathBuf::from(storage_path).join("metadata.json");
        if meta_path.exists() {
            let meta_content = std::fs::read_to_string(&meta_path)?;
            if let Ok(meta_json) = serde_json::from_str::<serde_json::Value>(&meta_content)
                && let Some(dim) = meta_json.get("dimension").and_then(|v| v.as_u64())
                && dim as usize != VECTOR_DIMENSION
            {
                return Err(anyhow::anyhow!(
                    "INDEX_REBUILD_REQUIRED: Database dimension {} does not match current model dimension {}. Please reindex.",
                    dim,
                    VECTOR_DIMENSION
                ));
            }
        }

        let db = connect(storage_path)
            .execute()
            .await
            .context("Failed to connect to LanceDB")?;

        Ok(Self {
            db,
            table_name: table_name.to_string(),
            storage_path: storage_path.to_string(),
        })
    }

    pub fn schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("document_id", DataType::Utf8, false),
            Field::new("path", DataType::Utf8, false),
            Field::new("type", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
            // links_raw and links_resolved represented as JSON strings for simplicity in LanceDB flat schema
            // or we can use List(Utf8)
            Field::new("links_raw", DataType::Utf8, false),
            Field::new("links_resolved", DataType::Utf8, false),
            Field::new("chunk_index", DataType::UInt32, false),
            Field::new("heading", DataType::Utf8, false),
            Field::new("text", DataType::Utf8, false),
            Field::new("confidence", DataType::Float32, true),
            // Use VECTOR_DIMENSION
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    VECTOR_DIMENSION as i32,
                ),
                false,
            ),
        ]))
    }

    pub async fn create_table(&self) -> Result<Table> {
        let meta_path = PathBuf::from(&self.storage_path).join("metadata.json");
        let meta_json = serde_json::json!({
            "dimension": VECTOR_DIMENSION,
            "created_at": chrono::Utc::now().to_rfc3339()
        });
        std::fs::write(&meta_path, serde_json::to_string_pretty(&meta_json)?)?;

        let schema = Self::schema();
        let table = self
            .db
            .create_empty_table(&self.table_name, schema)
            .execute()
            .await?;
        Ok(table)
    }

    pub async fn open_table(&self) -> Result<Table> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        self.migrate_schema(&table).await?;
        Ok(table)
    }

    async fn migrate_schema(&self, table: &Table) -> Result<()> {
        let schema = table.schema().await?;
        if schema.column_with_name("confidence").is_none() {
            use lancedb::arrow::arrow_schema::{DataType, Field, Schema as ArrowSchema};
            use lancedb::table::NewColumnTransform;
            use std::sync::Arc;

            tracing::info!(
                "Migrating LanceDB schema: adding 'confidence' column (Float32, nullable)"
            );

            let confidence_schema = Arc::new(ArrowSchema::new(vec![Field::new(
                "confidence",
                DataType::Float32,
                true,
            )]));

            match table
                .add_columns(NewColumnTransform::AllNulls(confidence_schema), None)
                .await
            {
                Ok(_) => {
                    tracing::info!("Schema migration successful.");
                    // Recreate FTS index which may be invalidated by add_columns
                    if let Err(e) = self.create_fts_index(table).await {
                        tracing::warn!(
                            "Failed to recreate FTS index after schema migration: {}",
                            e
                        );
                    }
                }
                Err(e) => {
                    let err_str = e.to_string();
                    if err_str.contains("already exists") || err_str.contains("duplicate") {
                        tracing::info!(
                            "Confidence column already exists (race condition), skipping migration."
                        );
                    } else {
                        tracing::warn!(
                            "Schema migration skipped (confidence column will not be available): {}",
                            e
                        );
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn create_fts_index(&self, table: &Table) -> Result<()> {
        // Build Tantivy FTS index on the `text` column
        table
            .create_index(&["text"], Index::FTS(Default::default()))
            .execute()
            .await?;
        Ok(())
    }

    pub fn code_table_name(&self) -> String {
        format!("{}_code_chunks", self.table_name)
    }

    pub fn graph_nodes_table_name(&self) -> String {
        format!("{}_graph_nodes", self.table_name)
    }

    pub fn graph_edges_table_name(&self) -> String {
        format!("{}_graph_edges", self.table_name)
    }

    pub fn graph_edge_overrides_table_name(&self) -> String {
        format!("{}_graph_edge_overrides", self.table_name)
    }

    pub fn graph_nodes_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("node_key", DataType::Utf8, false),
            Field::new("corpus", DataType::Utf8, false),
            Field::new("source_id", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, false),
            Field::new("path", DataType::Utf8, true),
            Field::new("metadata_json", DataType::Utf8, false),
            Field::new("generation", DataType::UInt64, true),
            Field::new("updated_at", DataType::Utf8, false),
        ]))
    }

    pub fn graph_edges_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("edge_key", DataType::Utf8, false),
            Field::new("source_key", DataType::Utf8, false),
            Field::new("target_key", DataType::Utf8, false),
            Field::new("relation", DataType::Utf8, false),
            Field::new("origin", DataType::Utf8, false),
            Field::new("extractor", DataType::Utf8, true),
            Field::new("resolution", DataType::Utf8, false),
            Field::new("confidence", DataType::Float32, true),
            Field::new("generation", DataType::UInt64, true),
            Field::new("metadata_json", DataType::Utf8, false),
            Field::new("created_at", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
        ]))
    }

    pub fn graph_edge_overrides_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("edge_key", DataType::Utf8, false),
            Field::new("action", DataType::Utf8, false),
            Field::new("revision", DataType::UInt64, false),
            Field::new("author", DataType::Utf8, true),
            Field::new("created_at", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
        ]))
    }

    pub fn code_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("item_key", DataType::Utf8, false),
            Field::new("file_path", DataType::Utf8, false),
            Field::new("module_path", DataType::Utf8, false),
            Field::new("symbol_name", DataType::Utf8, false),
            Field::new("qualified_symbol", DataType::Utf8, false),
            Field::new("kind", DataType::Utf8, false),
            Field::new("language", DataType::Utf8, false),
            Field::new("start_line", DataType::UInt32, false),
            Field::new("end_line", DataType::UInt32, false),
            Field::new("segment_index", DataType::UInt32, false),
            Field::new("item_hash", DataType::Utf8, false),
            Field::new("content_hash", DataType::Utf8, false),
            Field::new("content", DataType::Utf8, false),
            Field::new("timestamp", DataType::Utf8, true),
            Field::new(
                "vector",
                DataType::FixedSizeList(
                    Arc::new(Field::new("item", DataType::Float32, true)),
                    VECTOR_DIMENSION as i32,
                ),
                false,
            ),
        ]))
    }

    pub async fn recreate_code_table(&self) -> Result<Table> {
        let name = self.code_table_name();
        if let Err(e) = self.db.drop_table(&name, &[]).await {
            tracing::warn!("Failed to drop table {name}: {e}");
        }
        Ok(self
            .db
            .create_empty_table(&name, Self::code_schema())
            .execute()
            .await?)
    }

    pub async fn open_or_create_code_table(&self) -> Result<(Table, bool)> {
        let name = self.code_table_name();
        match self.db.open_table(&name).execute().await {
            Ok(table) => Ok((table, false)),
            Err(open_error) => match self
                .db
                .create_empty_table(&name, Self::code_schema())
                .execute()
                .await
            {
                Ok(table) => Ok((table, true)),
                Err(create_error) => Ok((
                    self.db.open_table(&name).execute().await.with_context(|| {
                        format!(
                            "could not open code table after create race; initial open error: {open_error}; create error: {create_error}"
                        )
                    })?,
                    false,
                )),
            },
        }
    }

    pub async fn insert_code_batch(
        &self,
        table: &Table,
        records: Vec<CodeChunkRecord>,
    ) -> Result<()> {
        table
            .add(vec![code_record_batch(records)?])
            .execute()
            .await?;
        Ok(())
    }

    pub async fn upsert_code_batch(
        &self,
        table: &Table,
        records: Vec<CodeChunkRecord>,
    ) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        use lancedb::arrow::arrow_array::RecordBatchIterator;
        let batch = code_record_batch(records)?;
        let mut merge = table.merge_insert(&["id"]);
        merge.when_matched_update_all(None);
        merge.when_not_matched_insert_all();
        merge
            .execute(Box::new(RecordBatchIterator::new(
                vec![Ok(batch)],
                Self::code_schema(),
            )))
            .await?;
        Ok(())
    }

    pub async fn stored_code_segments(
        &self,
        table: &Table,
    ) -> Result<std::collections::HashMap<String, StoredCodeSegment>> {
        use futures::stream::StreamExt;
        use lancedb::arrow::arrow_array::{Array, FixedSizeListArray, Float32Array, StringArray};
        use lancedb::query::{ExecutableQuery, QueryBase};

        let mut stream = table
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "id".to_string(),
                "content_hash".to_string(),
                "vector".to_string(),
            ]))
            .execute()
            .await?;
        let mut segments = std::collections::HashMap::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let ids = batch
                .column_by_name("id")
                .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                .context("code id column is not a StringArray")?;
            let hashes = batch
                .column_by_name("content_hash")
                .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                .context("code content_hash column is not a StringArray")?;
            let vectors = batch
                .column_by_name("vector")
                .and_then(|column| column.as_any().downcast_ref::<FixedSizeListArray>())
                .context("code vector column is not a FixedSizeListArray")?;
            let values = vectors
                .values()
                .as_any()
                .downcast_ref::<Float32Array>()
                .context("code vector values are not Float32Array")?;
            for index in 0..batch.num_rows() {
                if vectors.is_null(index) {
                    continue;
                }
                let start = index * VECTOR_DIMENSION;
                segments.insert(
                    ids.value(index).to_string(),
                    StoredCodeSegment {
                        content_hash: hashes.value(index).to_string(),
                        vector: values.values()[start..start + VECTOR_DIMENSION].to_vec(),
                    },
                );
            }
        }
        Ok(segments)
    }

    pub async fn delete_code_segments(&self, table: &Table, ids: &[String]) -> Result<()> {
        for id in ids {
            table
                .delete(&format!("id = '{}'", escape_filter(id)))
                .await?;
        }
        Ok(())
    }

    pub async fn create_code_fts_index(&self, table: &Table) -> Result<()> {
        table
            .create_index(&["content"], Index::FTS(Default::default()))
            .execute()
            .await?;
        Ok(())
    }

    pub async fn insert_batch(&self, table: &Table, records: Vec<ChunkRecord>) -> Result<()> {
        use lancedb::arrow::arrow_array::RecordBatch;
        use lancedb::arrow::arrow_array::builder::{
            FixedSizeListBuilder, Float32Builder, StringBuilder, UInt32Builder,
        };

        let mut document_id_b = StringBuilder::new();
        let mut path_b = StringBuilder::new();
        let mut type_b = StringBuilder::new();
        let mut title_b = StringBuilder::new();
        let mut content_hash_b = StringBuilder::new();
        let mut updated_at_b = StringBuilder::new();
        let mut links_raw_b = StringBuilder::new();
        let mut links_resolved_b = StringBuilder::new();
        let mut chunk_index_b = UInt32Builder::new();
        let mut heading_b = StringBuilder::new();
        let mut text_b = StringBuilder::new();
        let mut confidence_b = Float32Builder::new();

        let item_builder = Float32Builder::new();
        let mut vector_b = FixedSizeListBuilder::new(item_builder, VECTOR_DIMENSION as i32);

        for r in records {
            document_id_b.append_value(r.document_id);
            path_b.append_value(r.path);
            type_b.append_value(r.doc_type);
            title_b.append_value(r.title);
            content_hash_b.append_value(r.content_hash);
            updated_at_b.append_value(r.updated_at);
            links_raw_b.append_value(r.links_raw);
            links_resolved_b.append_value(r.links_resolved);
            chunk_index_b.append_value(r.chunk_index);
            heading_b.append_value(r.heading);
            text_b.append_value(r.text);
            match r.confidence {
                Some(c) => confidence_b.append_value(c),
                None => confidence_b.append_null(),
            }

            vector_b.values().append_slice(&r.vector);
            vector_b.append(true);
        }

        let batch = RecordBatch::try_new(
            Self::schema(),
            vec![
                Arc::new(document_id_b.finish()),
                Arc::new(path_b.finish()),
                Arc::new(type_b.finish()),
                Arc::new(title_b.finish()),
                Arc::new(content_hash_b.finish()),
                Arc::new(updated_at_b.finish()),
                Arc::new(links_raw_b.finish()),
                Arc::new(links_resolved_b.finish()),
                Arc::new(chunk_index_b.finish()),
                Arc::new(heading_b.finish()),
                Arc::new(text_b.finish()),
                Arc::new(confidence_b.finish()),
                Arc::new(vector_b.finish()),
            ],
        )?;

        let batches = vec![batch];
        table.add(batches).execute().await?;
        Ok(())
    }

    pub async fn delete_document(&self, table: &Table, document_id: &str) -> Result<()> {
        table
            .delete(&format!("document_id = '{}'", escape_filter(document_id)))
            .await?;
        Ok(())
    }

    pub async fn get_all_document_timestamps(
        &self,
        table: &Table,
    ) -> Result<std::collections::HashMap<String, String>> {
        use futures::stream::StreamExt;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let mut stream = table
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "document_id".to_string(),
                "updated_at".to_string(),
            ]))
            .execute()
            .await?;
        let mut map = std::collections::HashMap::new();
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res?;
            let doc_id_col = batch
                .column_by_name("document_id")
                .context("Missing 'document_id' column in timestamps query")?;
            let updated_at_col = batch
                .column_by_name("updated_at")
                .context("Missing 'updated_at' column in timestamps query")?;

            let doc_id_array = doc_id_col
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'document_id' column is not a StringArray")?;
            let updated_at_array = updated_at_col
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'updated_at' column is not a StringArray")?;

            for i in 0..batch.num_rows() {
                map.insert(
                    doc_id_array.value(i).to_string(),
                    updated_at_array.value(i).to_string(),
                );
            }
        }
        Ok(map)
    }

    /// Returns a map of relative file path → (document_id, last stored mtime).
    pub async fn get_file_timestamps(
        &self,
        table: &Table,
    ) -> Result<std::collections::HashMap<String, (String, String)>> {
        use futures::stream::StreamExt;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let mut stream = table
            .query()
            .select(lancedb::query::Select::Columns(vec![
                "path".to_string(),
                "document_id".to_string(),
                "updated_at".to_string(),
            ]))
            .execute()
            .await?;
        let mut map = std::collections::HashMap::new();
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res?;
            let path_col = batch
                .column_by_name("path")
                .context("Missing 'path' column")?;
            let doc_id_col = batch
                .column_by_name("document_id")
                .context("Missing 'document_id' column")?;
            let updated_at_col = batch
                .column_by_name("updated_at")
                .context("Missing 'updated_at' column")?;
            let path_array = path_col
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'path' column is not a StringArray")?;
            let doc_id_array = doc_id_col
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'document_id' column is not a StringArray")?;
            let updated_at_array = updated_at_col
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'updated_at' column is not a StringArray")?;
            for i in 0..batch.num_rows() {
                map.insert(
                    path_array.value(i).to_string(),
                    (
                        doc_id_array.value(i).to_string(),
                        updated_at_array.value(i).to_string(),
                    ),
                );
            }
        }
        Ok(map)
    }
}

pub struct ChunkRecord {
    pub document_id: String,
    pub path: String,
    pub doc_type: String,
    pub title: String,
    pub content_hash: String,
    pub updated_at: String,
    pub links_raw: String,
    pub links_resolved: String,
    pub chunk_index: u32,
    pub heading: String,
    pub text: String,
    pub vector: Vec<f32>,
    pub confidence: Option<f32>,
}

#[derive(Clone)]
pub struct CodeChunkRecord {
    pub id: String,
    pub item_key: String,
    pub file_path: String,
    pub module_path: String,
    pub symbol_name: String,
    pub qualified_symbol: String,
    pub kind: String,
    pub language: String,
    pub start_line: u32,
    pub end_line: u32,
    pub segment_index: u32,
    pub item_hash: String,
    pub content_hash: String,
    pub content: String,
    pub timestamp: Option<String>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone)]
pub struct StoredCodeSegment {
    pub content_hash: String,
    pub vector: Vec<f32>,
}

fn code_record_batch(
    records: Vec<CodeChunkRecord>,
) -> Result<lancedb::arrow::arrow_array::RecordBatch> {
    use lancedb::arrow::arrow_array::RecordBatch;
    use lancedb::arrow::arrow_array::builder::{
        FixedSizeListBuilder, Float32Builder, StringBuilder, UInt32Builder,
    };

    let mut id = StringBuilder::new();
    let mut item_key = StringBuilder::new();
    let mut file_path = StringBuilder::new();
    let mut module_path = StringBuilder::new();
    let mut symbol_name = StringBuilder::new();
    let mut qualified_symbol = StringBuilder::new();
    let mut kind = StringBuilder::new();
    let mut language = StringBuilder::new();
    let mut start_line = UInt32Builder::new();
    let mut end_line = UInt32Builder::new();
    let mut segment_index = UInt32Builder::new();
    let mut item_hash = StringBuilder::new();
    let mut content_hash = StringBuilder::new();
    let mut content = StringBuilder::new();
    let mut timestamp = StringBuilder::new();
    let mut vector = FixedSizeListBuilder::new(Float32Builder::new(), VECTOR_DIMENSION as i32);
    for record in records {
        if record.vector.len() != VECTOR_DIMENSION {
            return Err(anyhow::anyhow!(
                "code vector dimension {} does not match expected {}",
                record.vector.len(),
                VECTOR_DIMENSION
            ));
        }
        id.append_value(record.id);
        item_key.append_value(record.item_key);
        file_path.append_value(record.file_path);
        module_path.append_value(record.module_path);
        symbol_name.append_value(record.symbol_name);
        qualified_symbol.append_value(record.qualified_symbol);
        kind.append_value(record.kind);
        language.append_value(record.language);
        start_line.append_value(record.start_line);
        end_line.append_value(record.end_line);
        segment_index.append_value(record.segment_index);
        item_hash.append_value(record.item_hash);
        content_hash.append_value(record.content_hash);
        content.append_value(record.content);
        match record.timestamp {
            Some(value) => timestamp.append_value(value),
            None => timestamp.append_null(),
        }
        vector.values().append_slice(&record.vector);
        vector.append(true);
    }
    Ok(RecordBatch::try_new(
        Store::code_schema(),
        vec![
            Arc::new(id.finish()),
            Arc::new(item_key.finish()),
            Arc::new(file_path.finish()),
            Arc::new(module_path.finish()),
            Arc::new(symbol_name.finish()),
            Arc::new(qualified_symbol.finish()),
            Arc::new(kind.finish()),
            Arc::new(language.finish()),
            Arc::new(start_line.finish()),
            Arc::new(end_line.finish()),
            Arc::new(segment_index.finish()),
            Arc::new(item_hash.finish()),
            Arc::new(content_hash.finish()),
            Arc::new(content.finish()),
            Arc::new(timestamp.finish()),
            Arc::new(vector.finish()),
        ],
    )?)
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SearchResult {
    pub path: String,
    pub heading: String,
    pub text: String,
    pub score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct CodeSearchResult {
    pub file_path: String,
    pub qualified_symbol: String,
    pub kind: String,
    pub language: String,
    pub start_line: u32,
    pub end_line: u32,
    pub segment_index: u32,
    pub content: String,
    pub score: Option<f32>,
}

fn extract_results(
    batch: &lancedb::arrow::arrow_array::RecordBatch,
    results: &mut Vec<SearchResult>,
) -> Result<()> {
    use lancedb::arrow::arrow_array::{Float32Array, StringArray};

    let path_array = batch
        .column_by_name("path")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
        .context("'path' column is not a StringArray")?;
    let heading_array = batch
        .column_by_name("heading")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
        .context("'heading' column is not a StringArray")?;
    let text_array = batch
        .column_by_name("text")
        .and_then(|c| c.as_any().downcast_ref::<StringArray>().cloned())
        .context("'text' column is not a StringArray")?;
    let score_col = batch.column_by_name("_distance");
    let score_array = score_col.and_then(|c| c.as_any().downcast_ref::<Float32Array>().cloned());
    let confidence_col = batch.column_by_name("confidence");
    let confidence_array =
        confidence_col.and_then(|c| c.as_any().downcast_ref::<Float32Array>().cloned());

    for i in 0..batch.num_rows() {
        results.push(SearchResult {
            path: path_array.value(i).to_string(),
            heading: heading_array.value(i).to_string(),
            text: text_array.value(i).to_string(),
            score: score_array.as_ref().map(|sa| sa.value(i)),
            confidence: confidence_array.as_ref().map(|ca| ca.value(i)),
        });
    }
    Ok(())
}

impl Store {
    pub async fn search_code(
        &self,
        query_vector: Vec<f32>,
        limit: usize,
    ) -> Result<Vec<CodeSearchResult>> {
        use futures::stream::StreamExt;
        use lancedb::arrow::arrow_array::{Float32Array, StringArray, UInt32Array};
        use lancedb::query::{ExecutableQuery, QueryBase};

        let table = match self.db.open_table(self.code_table_name()).execute().await {
            Ok(table) => table,
            // Code memory is opt-in; an absent table is a valid empty corpus.
            Err(_) => return Ok(Vec::new()),
        };
        let mut stream = table
            .vector_search(query_vector)?
            .limit(limit)
            .execute()
            .await?;
        let mut results = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let text = |name: &str| -> Result<&StringArray> {
                batch
                    .column_by_name(name)
                    .and_then(|column| column.as_any().downcast_ref::<StringArray>())
                    .with_context(|| format!("code search column {name} is not a StringArray"))
            };
            let number = |name: &str| -> Result<&UInt32Array> {
                batch
                    .column_by_name(name)
                    .and_then(|column| column.as_any().downcast_ref::<UInt32Array>())
                    .with_context(|| format!("code search column {name} is not a UInt32Array"))
            };
            let scores = batch
                .column_by_name("_distance")
                .and_then(|column| column.as_any().downcast_ref::<Float32Array>());
            for index in 0..batch.num_rows() {
                results.push(CodeSearchResult {
                    file_path: text("file_path")?.value(index).to_string(),
                    qualified_symbol: text("qualified_symbol")?.value(index).to_string(),
                    kind: text("kind")?.value(index).to_string(),
                    language: text("language")?.value(index).to_string(),
                    start_line: number("start_line")?.value(index),
                    end_line: number("end_line")?.value(index),
                    segment_index: number("segment_index")?.value(index),
                    content: text("content")?.value(index).to_string(),
                    score: scores.map(|values| values.value(index)),
                });
            }
        }
        Ok(results)
    }

    pub async fn search(
        &self,
        query_vector: Vec<f32>,
        query_str: String,
        limit: usize,
        min_confidence: Option<f32>,
    ) -> Result<Vec<SearchResult>> {
        use futures::stream::StreamExt;
        use lance_index::scalar::FullTextSearchQuery;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let table = self.db.open_table(&self.table_name).execute().await?;

        let mut results = Vec::new();

        let query_vector_first = query_vector.clone();

        // Build base query builder
        let query_builder = table.vector_search(query_vector_first)?;
        let mut query: lancedb::query::VectorQuery = if !query_str.is_empty() {
            query_builder
                .full_text_search(FullTextSearchQuery::new(query_str))
                .limit(limit)
        } else {
            query_builder.limit(limit)
        };

        // Apply confidence filter (NULL-aware)
        if let Some(min_conf) = min_confidence {
            let filter = format!("confidence IS NULL OR confidence >= {}", min_conf);
            query = query.only_if(filter);
        }

        let stream = query.execute().await;
        match stream {
            Ok(mut s) => {
                while let Some(batch) = s.next().await {
                    let batch = batch?;
                    extract_results(&batch, &mut results)?;
                }
            }
            Err(e) => {
                tracing::warn!("Hybrid search failed ({}), falling back to vector-only", e);
                let query_builder = table.vector_search(query_vector)?.limit(limit);
                if let Some(min_conf) = min_confidence {
                    let filter = format!("confidence IS NULL OR confidence >= {}", min_conf);
                    let mut stream = query_builder.only_if(filter).execute().await?;
                    while let Some(batch) = stream.next().await {
                        let batch = batch?;
                        extract_results(&batch, &mut results)?;
                    }
                    return Ok(results);
                }
                let mut stream = query_builder.execute().await?;
                while let Some(batch) = stream.next().await {
                    let batch = batch?;
                    extract_results(&batch, &mut results)?;
                }
            }
        }
        Ok(results)
    }

    pub async fn read_document(&self, path: &str) -> Result<String> {
        use futures::stream::StreamExt;
        use lancedb::query::{ExecutableQuery, QueryBase};

        let table = self.db.open_table(&self.table_name).execute().await?;
        let stream = table
            .query()
            .only_if(format!("path = '{}'", escape_filter(path)))
            .execute()
            .await?;
        let mut stream = stream;

        let mut chunks = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let text_array = batch
                .column_by_name("text")
                .context("Missing 'text' column in read_document query")?
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .context("'text' column is not a StringArray")?;
            let chunk_index_array = batch
                .column_by_name("chunk_index")
                .context("Missing 'chunk_index' column in read_document query")?
                .as_any()
                .downcast_ref::<lancedb::arrow::arrow_array::UInt32Array>()
                .context("'chunk_index' column is not a UInt32Array")?;

            for i in 0..batch.num_rows() {
                chunks.push((chunk_index_array.value(i), text_array.value(i).to_string()));
            }
        }
        chunks.sort_by_key(|k| k.0);
        let full_text = chunks
            .into_iter()
            .map(|(_, t)| t)
            .collect::<Vec<_>>()
            .join("\n\n");
        Ok(full_text)
    }
}
