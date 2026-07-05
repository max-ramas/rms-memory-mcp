use std::sync::Arc;
use std::path::PathBuf;
use anyhow::{Result, Context};

use lancedb::connect;
use lancedb::table::Table;
use lancedb::index::{Index, vector::IvfPqIndexBuilder};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema as ArrowSchema};

pub const VECTOR_DIMENSION: usize = 384;

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
            if let Ok(meta_json) = serde_json::from_str::<serde_json::Value>(&meta_content) {
                if let Some(dim) = meta_json.get("dimension").and_then(|v| v.as_u64()) {
                    if dim as usize != VECTOR_DIMENSION {
                        return Err(anyhow::anyhow!(
                            "INDEX_REBUILD_REQUIRED: Database dimension {} does not match current model dimension {}. Please reindex.",
                            dim, VECTOR_DIMENSION
                        ));
                    }
                }
            }
        }

        let db = connect(storage_path).execute().await.context("Failed to connect to LanceDB")?;
        
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
            // Use VECTOR_DIMENSION
            Field::new("vector", DataType::FixedSizeList(Arc::new(Field::new("item", DataType::Float32, true)), VECTOR_DIMENSION as i32), false),
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
        let table = self.db.create_empty_table(&self.table_name, schema).execute().await?;
        Ok(table)
    }

    pub async fn open_table(&self) -> Result<Table> {
        let table = self.db.open_table(&self.table_name).execute().await?;
        Ok(table)
    }

    pub async fn create_fts_index(&self, table: &Table) -> Result<()> {
        // Build Tantivy FTS index on the `text` column
        table.create_index(&["text"], Index::FTS(Default::default())).execute().await?;
        Ok(())
    }

    pub async fn insert_batch(&self, table: &Table, records: Vec<ChunkRecord>) -> Result<()> {
        use lancedb::arrow::arrow_array::builder::{StringBuilder, UInt32Builder, FixedSizeListBuilder, Float32Builder};
        use lancedb::arrow::arrow_array::RecordBatch;
        
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
                Arc::new(vector_b.finish()),
            ],
        )?;

        let batches = vec![batch];
        table.add(batches).execute().await?;
        Ok(())
    }

    pub async fn delete_document(&self, table: &Table, document_id: &str) -> Result<()> {
        table.delete(&format!("document_id = '{}'", document_id)).await?;
        Ok(())
    }

    pub async fn get_all_document_timestamps(&self, table: &Table) -> Result<std::collections::HashMap<String, String>> {
        use lancedb::query::{ExecutableQuery, QueryBase};
        use futures::stream::StreamExt;
        
        let mut stream = table.query().select(lancedb::query::Select::Columns(vec!["document_id".to_string(), "updated_at".to_string()])).execute().await?;
        let mut map = std::collections::HashMap::new();
        while let Some(batch_res) = stream.next().await {
            let batch = batch_res?;
            let doc_id_col = batch.column_by_name("document_id").unwrap();
            let updated_at_col = batch.column_by_name("updated_at").unwrap();
            
            let doc_id_array = doc_id_col.as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>().unwrap();
            let updated_at_array = updated_at_col.as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>().unwrap();
            
            for i in 0..batch.num_rows() {
                map.insert(doc_id_array.value(i).to_string(), updated_at_array.value(i).to_string());
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
}

#[derive(serde::Serialize, serde::Deserialize, Debug, Clone)]
pub struct SearchResult {
    pub path: String,
    pub heading: String,
    pub text: String,
    pub score: Option<f32>,
}

pub trait VectorStore: Send + Sync {
    fn search(&self, query_vector: Vec<f32>, query_str: String, limit: usize) -> impl std::future::Future<Output = Result<Vec<SearchResult>>> + Send;
    fn read_document(&self, path: &str) -> impl std::future::Future<Output = Result<String>> + Send;
}

impl VectorStore for Store {
    async fn search(&self, query_vector: Vec<f32>, _query_str: String, limit: usize) -> Result<Vec<SearchResult>> {
        use lancedb::query::{ExecutableQuery, QueryBase};
        use futures::stream::StreamExt;

        let table = self.db.open_table(&self.table_name).execute().await?;

        let query_builder = table.vector_search(query_vector).unwrap().limit(limit);
        let mut stream = query_builder.execute().await?;
        
        let mut results = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let path_col = batch.column_by_name("path").expect("Missing 'path' column in search results");
            let path_array = path_col.as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .unwrap_or_else(|| panic!("'path' column is not a StringArray. Actual type: {:?}", path_col.data_type()));
            
            let heading_col = batch.column_by_name("heading").expect("Missing 'heading' column in search results");
            let heading_array = heading_col.as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .unwrap_or_else(|| panic!("'heading' column is not a StringArray. Actual type: {:?}", heading_col.data_type()));
            
            let text_col = batch.column_by_name("text").expect("Missing 'text' column in search results");
            let text_array = text_col.as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>()
                .unwrap_or_else(|| panic!("'text' column is not a StringArray. Actual type: {:?}", text_col.data_type()));
            let score_col = batch.column_by_name("_distance");
            let score_array = score_col.map(|c| c.as_any().downcast_ref::<lancedb::arrow::arrow_array::Float32Array>().unwrap());
            
            for i in 0..batch.num_rows() {
                results.push(SearchResult {
                    path: path_array.value(i).to_string(),
                    heading: heading_array.value(i).to_string(),
                    text: text_array.value(i).to_string(),
                    score: score_array.map(|sa| sa.value(i)),
                });
            }
        }
        Ok(results)
    }

    async fn read_document(&self, path: &str) -> Result<String> {
        use lancedb::query::{ExecutableQuery, QueryBase};
        use futures::stream::StreamExt;

        let table = self.db.open_table(&self.table_name).execute().await?;
        let stream = table.query().only_if(format!("path = '{}'", path)).execute().await?;
        let mut stream = stream;
        
        let mut chunks = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            let text_array = batch.column_by_name("text").unwrap().as_any().downcast_ref::<lancedb::arrow::arrow_array::StringArray>().unwrap();
            let chunk_index_array = batch.column_by_name("chunk_index").unwrap().as_any().downcast_ref::<lancedb::arrow::arrow_array::UInt32Array>().unwrap();
            
            for i in 0..batch.num_rows() {
                chunks.push((chunk_index_array.value(i), text_array.value(i).to_string()));
            }
        }
        chunks.sort_by_key(|k| k.0);
        let full_text = chunks.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n\n");
        Ok(full_text)
    }
}
