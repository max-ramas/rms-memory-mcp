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
        let mut idx = self.indexer.lock().await;
        let embeddings = idx
            .embed(&[query.to_string()])
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        let query_vector = embeddings.into_iter().next().unwrap_or_default();
        drop(idx);
        self.store
            .search(query_vector, query.to_string(), limit, None)
            .await
    }

    pub async fn search_code(&self, query: &str, limit: usize) -> Result<Vec<CodeSearchResult>> {
        let mut idx = self.indexer.lock().await;
        let embeddings = idx
            .embed(&[query.to_string()])
            .map_err(|e| anyhow::anyhow!("Embed failed: {}", e))?;
        let query_vector = embeddings.into_iter().next().unwrap_or_default();
        drop(idx);
        self.store.search_code(query_vector, limit).await
    }
}
