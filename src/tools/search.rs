use super::AppContext;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::HashMap;

const RRF_K: f32 = 60.0;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Corpus {
    Vault,
    Code,
    All,
}

impl Corpus {
    fn parse(value: Option<&str>) -> Result<Self> {
        match value.unwrap_or("vault") {
            "vault" => Ok(Self::Vault),
            "code" => Ok(Self::Code),
            "all" => Ok(Self::All),
            other => Err(anyhow!(
                "corpus must be one of: vault, code, all (got {other})"
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct UnifiedSearchResult {
    pub source: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heading: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rrf_score: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub qualified_symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segment_index: Option<u32>,
}

impl UnifiedSearchResult {
    fn vault(result: crate::store::SearchResult) -> Self {
        Self {
            source: "vault".to_string(),
            path: result.path,
            heading: Some(result.heading),
            content: Some(result.text),
            confidence: result.confidence,
            score: result.score,
            rrf_score: None,
            qualified_symbol: None,
            kind: None,
            language: None,
            start_line: None,
            end_line: None,
            segment_index: None,
        }
    }

    fn code(result: crate::store::CodeSearchResult) -> Self {
        Self {
            source: "code".to_string(),
            path: result.file_path,
            heading: None,
            content: Some(result.content),
            confidence: None,
            score: result.score,
            rrf_score: None,
            qualified_symbol: Some(result.qualified_symbol),
            kind: Some(result.kind),
            language: Some(result.language),
            start_line: Some(result.start_line),
            end_line: Some(result.end_line),
            segment_index: Some(result.segment_index),
        }
    }

    fn identity(&self) -> String {
        let content_hash = self
            .content
            .as_deref()
            .map(|content| blake3::hash(content.as_bytes()).to_string())
            .unwrap_or_default();
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}",
            self.source,
            self.path,
            self.heading.as_deref().unwrap_or_default(),
            self.qualified_symbol.as_deref().unwrap_or_default(),
            self.segment_index.unwrap_or_default(),
            content_hash,
        )
    }
}

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    execute_with_forced_corpus(ctx, args, None).await
}

pub async fn execute_code(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    execute_with_forced_corpus(ctx, args, Some(Corpus::Code)).await
}

async fn execute_with_forced_corpus(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
    forced_corpus: Option<Corpus>,
) -> Result<serde_json::Value> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow!("Store not initialized"))?;
    let query = args
        .get("query")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let limit = (args
        .get("limit")
        .and_then(|value| value.as_u64())
        .unwrap_or(10) as usize)
        .min(100);
    let include_content = args
        .get("include_content")
        .and_then(|value| value.as_bool())
        .unwrap_or(true);
    let min_confidence = args
        .get("min_confidence")
        .and_then(|value| value.as_f64())
        .map(|value| value as f32);
    let corpus = forced_corpus.unwrap_or(Corpus::parse(
        args.get("corpus").and_then(|value| value.as_str()),
    )?);

    let indexer = ctx
        .indexer
        .as_ref()
        .ok_or_else(|| anyhow!("Indexer not initialized"))?;
    let query_vector = {
        let mut indexer = indexer.lock().await;
        indexer
            .embed(&[query.to_string()])
            .map_err(|error| anyhow!("Embed failed: {error}"))?
            .into_iter()
            .next()
            .unwrap_or_default()
    };

    let mut results = match corpus {
        Corpus::Vault => store
            .search(query_vector, query.to_string(), limit, min_confidence)
            .await?
            .into_iter()
            .map(UnifiedSearchResult::vault)
            .collect(),
        Corpus::Code => store
            .search_code(query_vector, limit)
            .await?
            .into_iter()
            .map(UnifiedSearchResult::code)
            .collect(),
        Corpus::All => {
            let vault = store
                .search(
                    query_vector.clone(),
                    query.to_string(),
                    limit,
                    min_confidence,
                )
                .await?
                .into_iter()
                .map(UnifiedSearchResult::vault)
                .collect::<Vec<_>>();
            let code = store
                .search_code(query_vector, limit)
                .await?
                .into_iter()
                .map(UnifiedSearchResult::code)
                .collect::<Vec<_>>();
            reciprocal_rank_fusion(vault, code, limit)
        }
    };
    if !include_content {
        for result in &mut results {
            result.content = None;
        }
    }
    Ok(super::response::json_text_response(&serde_json::to_string(
        &results,
    )?))
}

/// Merge ranked result lists without comparing their raw retrieval distances.
/// Stable identity and lexicographic order make equal RRF scores deterministic.
pub fn reciprocal_rank_fusion(
    vault: Vec<UnifiedSearchResult>,
    code: Vec<UnifiedSearchResult>,
    limit: usize,
) -> Vec<UnifiedSearchResult> {
    let mut merged = HashMap::<String, UnifiedSearchResult>::new();
    for (rank, result) in vault.into_iter().enumerate() {
        accumulate_rrf(&mut merged, result, rank);
    }
    for (rank, result) in code.into_iter().enumerate() {
        accumulate_rrf(&mut merged, result, rank);
    }
    let mut results = merged.into_values().collect::<Vec<_>>();
    results.sort_by(|left, right| {
        right
            .rrf_score
            .partial_cmp(&left.rrf_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.identity().cmp(&right.identity()))
    });
    results.truncate(limit);
    results
}

fn accumulate_rrf(
    merged: &mut HashMap<String, UnifiedSearchResult>,
    result: UnifiedSearchResult,
    source_rank: usize,
) {
    let score = 1.0 / (RRF_K + source_rank as f32 + 1.0);
    let identity = result.identity();
    match merged.get_mut(&identity) {
        Some(existing) => existing.rrf_score = Some(existing.rrf_score.unwrap_or(0.0) + score),
        None => {
            let mut result = result;
            result.score = None;
            result.rrf_score = Some(score);
            merged.insert(identity, result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(source: &str, path: &str) -> UnifiedSearchResult {
        UnifiedSearchResult {
            source: source.to_string(),
            path: path.to_string(),
            heading: None,
            content: None,
            confidence: None,
            score: Some(999.0),
            rrf_score: None,
            qualified_symbol: None,
            kind: None,
            language: None,
            start_line: None,
            end_line: None,
            segment_index: None,
        }
    }

    #[test]
    fn rrf_uses_source_local_rank_and_never_raw_distance() {
        let fused = reciprocal_rank_fusion(
            vec![result("vault", "a"), result("vault", "b")],
            vec![result("code", "z"), result("code", "y")],
            4,
        );
        assert_eq!(fused[0].path, "z");
        assert_eq!(fused[1].path, "a");
        assert!(fused.iter().all(|entry| entry.score.is_none()));
        assert!(fused.iter().all(|entry| entry.rrf_score.is_some()));
    }
}
