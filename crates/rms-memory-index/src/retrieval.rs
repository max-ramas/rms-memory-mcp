use crate::indexer::Indexer;
use crate::store::{CodeSearchResult, SearchResult, Store};
use anyhow::Result;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct RetrievalService {
    store: Arc<Store>,
    indexer: Arc<Mutex<Indexer>>,
}

impl RetrievalService {
    pub fn new(store: Arc<Store>, indexer: Arc<Mutex<Indexer>>) -> Self {
        Self { store, indexer }
    }

    pub async fn search_vault(&self, query: &str, limit: usize) -> Result<Vec<SearchResult>> {
        let query_vector = Indexer::embed_query_shared(&self.indexer, query)
            .await
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        self.store
            .search(query_vector, query.to_string(), limit, None)
            .await
    }

    pub async fn search_code(&self, query: &str, limit: usize) -> Result<Vec<CodeSearchResult>> {
        let query_vector = Indexer::embed_query_shared(&self.indexer, query)
            .await
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        self.store.search_code(query_vector, limit).await
    }
}
