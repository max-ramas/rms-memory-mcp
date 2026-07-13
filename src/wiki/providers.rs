use crate::wiki::manifest::WikiSource;
use serde::{Deserialize, Serialize};

pub trait SourceProvider: Send + Sync {
    fn resolve<'a>(
        &'a self,
        source: &'a WikiSource,
        ctx: &'a ProviderContext,
    ) -> impl std::future::Future<Output = Vec<ResolvedItem>> + Send;
}

#[derive(Debug, Clone)]
pub struct ProviderContext {
    pub workspace_root: std::path::PathBuf,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedItem {
    pub content: String,
    pub provenance: ItemProvenance,
    pub score: Option<f32>,
    pub char_count: usize,
}

impl ResolvedItem {
    pub fn new(content: String, provenance: ItemProvenance, score: Option<f32>) -> Self {
        let char_count = content.chars().count();
        Self {
            content,
            provenance,
            score,
            char_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemProvenance {
    pub source_type: String,
    pub path: Option<String>,
    pub line_range: Option<(usize, usize)>,
    pub symbol_id: Option<String>,
    pub retrieval_score: Option<f32>,
    pub content_hash: String,
}

impl ItemProvenance {
    pub fn new(source_type: &str, content: &str) -> Self {
        Self {
            source_type: source_type.to_string(),
            path: None,
            line_range: None,
            symbol_id: None,
            retrieval_score: None,
            content_hash: blake3::hash(content.as_bytes()).to_hex().to_string(),
        }
    }
}

pub fn stable_id(item: &ResolvedItem) -> String {
    match &item.provenance.path {
        Some(path) => format!("{}:{}", item.provenance.source_type, path),
        None => item.provenance.content_hash.clone(),
    }
}
