use super::AppContext;
use anyhow::{Result, anyhow};
use serde::Serialize;
use std::collections::HashMap;

const RRF_K: f32 = 60.0;
/// Default total character budget for injected content (include_content=true).
const DEFAULT_MAX_CHARS: usize = 2000;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<bool>,
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
            pinned: result.pinned,
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
            pinned: None,
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

    fn inject_id(&self) -> String {
        if let Some(symbol) = self.qualified_symbol.as_deref() {
            format!("{}:{}", self.path, symbol)
        } else if let Some(heading) = self.heading.as_deref().filter(|h| !h.is_empty()) {
            format!("{}#{}", self.path, heading)
        } else {
            self.path.clone()
        }
    }

    /// Normalize Lance `_distance` or RRF into a 0..1-ish relevance (higher = better).
    fn relevance(&self) -> Option<f32> {
        if let Some(rrf) = self.rrf_score {
            // Single-list top rank ≈ 1/(K+1); scale so that ≈1.0.
            return Some((rrf * (RRF_K + 1.0)).min(1.0));
        }
        self.score.map(|distance| 1.0 / (1.0 + distance.max(0.0)))
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchDecision {
    Inject,
    Abstain,
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchEnvelope {
    pub decision: SearchDecision,
    pub reason: String,
    pub injected_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_relevance: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retrieval_mode: Option<String>,
    pub results: Vec<UnifiedSearchResult>,
}

impl SearchEnvelope {
    fn inject(
        results: Vec<UnifiedSearchResult>,
        max_relevance: Option<f32>,
        retrieval_mode: Option<String>,
    ) -> Self {
        let injected_ids = results.iter().map(|r| r.inject_id()).collect();
        Self {
            decision: SearchDecision::Inject,
            reason: "hits_above_threshold".into(),
            injected_ids,
            max_relevance,
            retrieval_mode,
            results,
        }
    }

    fn abstain(
        reason: impl Into<String>,
        max_relevance: Option<f32>,
        retrieval_mode: Option<String>,
    ) -> Self {
        Self {
            decision: SearchDecision::Abstain,
            reason: reason.into(),
            injected_ids: Vec::new(),
            max_relevance,
            retrieval_mode,
            results: Vec::new(),
        }
    }
}

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    execute_with_forced_corpus(ctx, args, None).await
}

/// Library/GUI entry: same bounded-recall envelope as the MCP tool, without JSON-RPC wrapping.
pub async fn search_envelope(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<SearchEnvelope> {
    match execute_inner(ctx, args, None).await {
        Ok(envelope) => {
            log_decision(&envelope);
            Ok(envelope)
        }
        Err(error) => {
            tracing::warn!(error = %error, "rms_search failed closed (abstain)");
            let envelope = SearchEnvelope::abstain(format!("search_error: {error}"), None, None);
            log_decision(&envelope);
            Ok(envelope)
        }
    }
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
    match execute_inner(ctx, args, forced_corpus).await {
        Ok(envelope) => {
            log_decision(&envelope);
            Ok(super::response::json_text_response(&serde_json::to_string(
                &envelope,
            )?))
        }
        Err(error) => {
            // Fail-closed: never invent weak context for the agent.
            tracing::warn!(error = %error, "rms_search failed closed (abstain)");
            let envelope = SearchEnvelope::abstain(format!("search_error: {error}"), None, None);
            log_decision(&envelope);
            Ok(super::response::json_text_response(&serde_json::to_string(
                &envelope,
            )?))
        }
    }
}

fn log_decision(envelope: &SearchEnvelope) {
    match envelope.decision {
        SearchDecision::Inject => tracing::info!(
            ids = ?envelope.injected_ids,
            max_relevance = ?envelope.max_relevance,
            "rms_search injected"
        ),
        SearchDecision::Abstain => tracing::info!(
            reason = %envelope.reason,
            max_relevance = ?envelope.max_relevance,
            "rms_search abstained"
        ),
    }
}

async fn execute_inner(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
    forced_corpus: Option<Corpus>,
) -> Result<SearchEnvelope> {
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
    let max_chars = args
        .get("max_chars")
        .and_then(|value| value.as_u64())
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_MAX_CHARS);
    let min_score = args
        .get("min_score")
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

    let mut retrieval_mode: Option<String> = None;
    let results = match corpus {
        Corpus::Vault => {
            let (hits, mode) = store
                .search_with_mode(query_vector, query.to_string(), limit, min_confidence)
                .await?;
            retrieval_mode = Some(mode.as_str().to_string());
            hits.into_iter().map(UnifiedSearchResult::vault).collect()
        }
        Corpus::Code => store
            .search_code(query_vector, limit)
            .await?
            .into_iter()
            .map(UnifiedSearchResult::code)
            .collect(),
        Corpus::All => {
            let (vault_hits, vault_mode) = store
                .search_with_mode(
                    query_vector.clone(),
                    query.to_string(),
                    limit,
                    min_confidence,
                )
                .await?;
            retrieval_mode = Some(vault_mode.as_str().to_string());
            let vault = vault_hits
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

    Ok(apply_bounded_recall(
        results,
        include_content,
        max_chars,
        min_score,
        retrieval_mode,
    ))
}

/// Apply score abstain + character budget. Pure for unit tests.
pub fn apply_bounded_recall(
    mut results: Vec<UnifiedSearchResult>,
    include_content: bool,
    max_chars: usize,
    min_score: Option<f32>,
    retrieval_mode: Option<String>,
) -> SearchEnvelope {
    let max_relevance = results
        .iter()
        .filter_map(|r| r.relevance())
        .fold(None, |acc: Option<f32>, value| {
            Some(acc.map_or(value, |current| current.max(value)))
        });

    if results.is_empty() {
        return SearchEnvelope::abstain("no_hits", max_relevance, retrieval_mode);
    }

    if let Some(threshold) = min_score {
        let best = max_relevance.unwrap_or(0.0);
        if best < threshold {
            return SearchEnvelope::abstain(
                format!("best_relevance_below_min_score:{best:.4}<{threshold:.4}"),
                max_relevance,
                retrieval_mode,
            );
        }
    }

    if !include_content {
        for result in &mut results {
            result.content = None;
        }
        return SearchEnvelope::inject(results, max_relevance, retrieval_mode);
    }

    results = apply_char_budget(results, max_chars);
    if results.is_empty() {
        return SearchEnvelope::abstain("char_budget_exhausted", max_relevance, retrieval_mode);
    }
    SearchEnvelope::inject(results, max_relevance, retrieval_mode)
}

fn apply_char_budget(
    results: Vec<UnifiedSearchResult>,
    max_chars: usize,
) -> Vec<UnifiedSearchResult> {
    // Pinned vault notes survive the budget: pack them first, then fill with the rest.
    let (pinned, rest): (Vec<_>, Vec<_>) = results
        .into_iter()
        .partition(|result| result.pinned == Some(true));
    let ordered = pinned.into_iter().chain(rest).collect::<Vec<_>>();

    let mut total = 0usize;
    let mut kept = Vec::new();
    for mut result in ordered {
        let is_pinned = result.pinned == Some(true);
        let Some(content) = result.content.as_mut() else {
            kept.push(result);
            continue;
        };
        let len = content.chars().count();
        if total >= max_chars && !is_pinned {
            break;
        }
        if total + len > max_chars {
            let remaining = max_chars.saturating_sub(total);
            if remaining < 80 && !is_pinned {
                break;
            }
            if remaining >= 80 {
                truncate_content(
                    content,
                    remaining.max(if is_pinned { 80 } else { remaining }),
                );
            } else if is_pinned {
                // Always keep a short pinned stub even when the budget is exhausted.
                truncate_content(content, 80.min(len));
            } else {
                break;
            }
        }
        total += content.chars().count();
        kept.push(result);
    }
    kept
}

fn truncate_content(content: &mut String, max_chars: usize) {
    if content.chars().count() <= max_chars {
        return;
    }
    let keep = max_chars.saturating_sub(18); // room for truncation marker
    let truncated: String = content.chars().take(keep).collect();
    *content = format!("{truncated}\n... [truncated]\n");
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
            content: Some(format!("body for {path}")),
            confidence: None,
            score: Some(999.0),
            rrf_score: None,
            qualified_symbol: None,
            kind: None,
            language: None,
            start_line: None,
            end_line: None,
            segment_index: None,
            pinned: None,
        }
    }

    fn with_distance(mut item: UnifiedSearchResult, distance: f32) -> UnifiedSearchResult {
        item.score = Some(distance);
        item
    }

    fn with_content(mut item: UnifiedSearchResult, content: &str) -> UnifiedSearchResult {
        item.content = Some(content.to_string());
        item
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

    #[test]
    fn abstains_when_best_relevance_below_min_score() {
        let envelope = apply_bounded_recall(
            vec![with_distance(result("vault", "weak"), 20.0)],
            true,
            2000,
            Some(0.5),
            None,
        );
        assert_eq!(envelope.decision, SearchDecision::Abstain);
        assert!(envelope.results.is_empty());
        assert!(envelope.reason.contains("best_relevance_below_min_score"));
    }

    #[test]
    fn injects_when_relevance_meets_min_score() {
        let envelope = apply_bounded_recall(
            vec![with_distance(result("vault", "strong"), 0.1)],
            true,
            2000,
            Some(0.5),
            Some("fts_prefer".into()),
        );
        assert_eq!(envelope.decision, SearchDecision::Inject);
        assert_eq!(envelope.results.len(), 1);
        assert_eq!(envelope.injected_ids, vec!["strong".to_string()]);
        assert_eq!(envelope.retrieval_mode.as_deref(), Some("fts_prefer"));
    }

    /// Dual-gate contract with `vault_recall_filter`: NULL confidence is
    /// fail-open at Lance (`min_confidence`), but `min_score` still abstains
    /// on weak runtime relevance. Do not "fix" this into injecting legacy notes.
    #[test]
    fn null_confidence_hit_still_abstains_under_min_score() {
        let mut legacy = with_distance(result("vault", "legacy-null-confidence"), 20.0);
        legacy.confidence = None;
        let envelope =
            apply_bounded_recall(vec![legacy], true, 2000, Some(0.5), Some("hybrid".into()));
        assert_eq!(envelope.decision, SearchDecision::Abstain);
        assert!(envelope.results.is_empty());
        assert!(envelope.reason.contains("best_relevance_below_min_score"));
        assert_eq!(envelope.retrieval_mode.as_deref(), Some("hybrid"));
    }

    #[test]
    fn null_confidence_with_strong_score_still_injects() {
        let mut legacy = with_distance(result("vault", "legacy-null-confidence"), 0.1);
        legacy.confidence = None;
        let envelope =
            apply_bounded_recall(vec![legacy], true, 2000, Some(0.5), Some("hybrid".into()));
        assert_eq!(envelope.decision, SearchDecision::Inject);
        assert_eq!(
            envelope.injected_ids,
            vec!["legacy-null-confidence".to_string()]
        );
        assert!(envelope.results[0].confidence.is_none());
    }

    #[test]
    fn char_budget_truncates_and_drops_overflow() {
        let long = "x".repeat(500);
        let envelope = apply_bounded_recall(
            vec![
                with_content(result("vault", "a"), &long),
                with_content(result("vault", "b"), &long),
            ],
            true,
            400,
            None,
            None,
        );
        assert_eq!(envelope.decision, SearchDecision::Inject);
        assert_eq!(envelope.results.len(), 1);
        let content = envelope.results[0].content.as_deref().unwrap();
        assert!(content.contains("[truncated]"));
        assert!(content.chars().count() <= 420);
    }

    #[test]
    fn char_budget_prefers_pinned_notes() {
        let long = "x".repeat(500);
        let mut pinned = with_content(result("vault", "pinned-note"), &long);
        pinned.pinned = Some(true);
        let envelope = apply_bounded_recall(
            vec![with_content(result("vault", "first"), &long), pinned],
            true,
            400,
            None,
            None,
        );
        assert_eq!(envelope.decision, SearchDecision::Inject);
        assert!(
            envelope.results.iter().any(|r| r.path == "pinned-note"),
            "pinned note must survive char budget: {:?}",
            envelope.results.iter().map(|r| &r.path).collect::<Vec<_>>()
        );
    }

    #[test]
    fn abstains_on_empty_hits() {
        let envelope = apply_bounded_recall(vec![], true, 2000, None, None);
        assert_eq!(envelope.decision, SearchDecision::Abstain);
        assert_eq!(envelope.reason, "no_hits");
    }
}
