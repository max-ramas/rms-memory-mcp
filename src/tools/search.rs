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
    /// Source project key when the hit came from a federated `projects` search.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
            project: None,
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
            project: None,
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

    fn with_project(mut self, key: &str) -> Self {
        self.project = Some(key.to_string());
        self
    }

    fn identity(&self) -> String {
        let content_hash = self
            .content
            .as_deref()
            .map(|content| blake3::hash(content.as_bytes()).to_string())
            .unwrap_or_default();
        format!(
            "{}\0{}\0{}\0{}\0{}\0{}\0{}",
            self.project.as_deref().unwrap_or_default(),
            self.source,
            self.path,
            self.heading.as_deref().unwrap_or_default(),
            self.qualified_symbol.as_deref().unwrap_or_default(),
            self.segment_index.unwrap_or_default(),
            content_hash,
        )
    }

    fn inject_id(&self) -> String {
        let local = if let Some(symbol) = self.qualified_symbol.as_deref() {
            format!("{}:{}", self.path, symbol)
        } else if let Some(heading) = self.heading.as_deref().filter(|h| !h.is_empty()) {
            format!("{}#{}", self.path, heading)
        } else {
            self.path.clone()
        };
        match self.project.as_deref() {
            Some(key) => format!("{key}::{local}"),
            None => local,
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

/// Hard-fail error for federated-search argument / policy violations.
///
/// These must surface as tool errors (JSON-RPC / Tauri Err), never as an
/// abstain envelope — silent degrade would make a denied vault search look
/// like an empty index. See ADR `decisions/cross-project-federated-search.md`.
#[derive(Debug)]
pub struct FederatedSearchError(pub String);

impl std::fmt::Display for FederatedSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for FederatedSearchError {}

fn federated_err(message: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(FederatedSearchError(message.into()))
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
        Err(error) if error.downcast_ref::<FederatedSearchError>().is_some() => {
            // Policy / argument errors must surface to the caller, not become abstain.
            Err(error)
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
        Err(error) if error.downcast_ref::<FederatedSearchError>().is_some() => {
            // Hard tool error — never silent-degrade a denied vault federation.
            Err(error)
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

    let project_keys = parse_projects_arg(args)?;
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

    let (results, retrieval_mode) = if let Some(keys) = project_keys {
        // Federated path: open each project's store independently. Does NOT
        // mutate the active MCP bind (read-only composition).
        let registry = crate::workspace::Registry::load()?;
        let resolved = resolve_project_keys(&registry, &keys)?;
        enforce_cross_project_vault_policy(&resolved, corpus)?;
        search_federated(
            &resolved,
            query_vector,
            query,
            limit,
            min_confidence,
            corpus,
        )
        .await?
    } else {
        let store = ctx
            .store
            .as_ref()
            .ok_or_else(|| anyhow!("Store not initialized"))?;
        search_single_store(store, query_vector, query, limit, min_confidence, corpus).await?
    };

    Ok(apply_bounded_recall(
        results,
        include_content,
        max_chars,
        min_score,
        retrieval_mode,
    ))
}

/// Parse the optional `projects` array.
///
/// Returns `None` when the caller is using the single-project bind path
/// (no `projects` argument). An empty array is a hard error.
///
/// When both `project` and `projects` are set, **`projects` wins**: injected
/// agent rules always pass `project`, so hard mutual exclusion would make the
/// federation API unusable for rule-following agents. Cap length after dedupe.
const MAX_FEDERATED_PROJECTS: usize = 8;

fn parse_projects_arg(
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<Option<Vec<String>>> {
    let Some(raw) = args.get("projects") else {
        return Ok(None);
    };
    let Some(array) = raw.as_array() else {
        return Err(federated_err("`projects` must be an array of project keys"));
    };
    let mut keys = Vec::with_capacity(array.len());
    for entry in array {
        let Some(key) = entry.as_str().map(str::trim).filter(|key| !key.is_empty()) else {
            return Err(federated_err(
                "`projects` entries must be non-empty strings",
            ));
        };
        keys.push(key.to_string());
    }
    if keys.is_empty() {
        return Err(federated_err(
            "`projects` must contain at least one project key",
        ));
    }
    // Deduplicate while preserving order.
    let mut seen = std::collections::HashSet::new();
    keys.retain(|key| seen.insert(key.clone()));
    if keys.len() > MAX_FEDERATED_PROJECTS {
        return Err(federated_err(format!(
            "`projects` accepts at most {MAX_FEDERATED_PROJECTS} keys after dedupe (got {})",
            keys.len()
        )));
    }
    Ok(Some(keys))
}

struct ResolvedProject {
    key: String,
    cross_project_vault: bool,
    code_path: String,
}

fn resolve_project_keys(
    registry: &crate::workspace::Registry,
    keys: &[String],
) -> Result<Vec<ResolvedProject>> {
    let mut resolved = Vec::with_capacity(keys.len());
    let mut missing = Vec::new();
    for key in keys {
        match registry.locate_by_project(key) {
            Some(config) => resolved.push(ResolvedProject {
                key: key.clone(),
                cross_project_vault: config.cross_project_vault,
                code_path: config.code_path.clone(),
            }),
            None => missing.push(key.clone()),
        }
    }
    if !missing.is_empty() {
        let mut valid = registry.projects.keys().cloned().collect::<Vec<_>>();
        valid.sort();
        return Err(federated_err(format!(
            "Unknown RMS Memory project key(s): {}. Valid keys: {}",
            missing.join(", "),
            if valid.is_empty() {
                "(none registered)".to_string()
            } else {
                valid.join(", ")
            }
        )));
    }
    Ok(resolved)
}

/// `cross_project_vault` is consulted **iff** `projects.len() > 1`.
///
/// A single-element list is the same scope as `project=A` and bypasses the
/// gate — requiring opt-in there would be a false positive. When more than
/// one project is requested with `corpus=vault|all`, every listed key must
/// have opted in; otherwise this returns a hard FederatedSearchError (no
/// silent degrade to code-only, no partial vault search).
fn enforce_cross_project_vault_policy(projects: &[ResolvedProject], corpus: Corpus) -> Result<()> {
    if projects.len() <= 1 {
        return Ok(());
    }
    let needs_vault = matches!(corpus, Corpus::Vault | Corpus::All);
    if !needs_vault {
        return Ok(());
    }
    let denied = projects
        .iter()
        .filter(|project| !project.cross_project_vault)
        .map(|project| project.key.as_str())
        .collect::<Vec<_>>();
    if denied.is_empty() {
        return Ok(());
    }
    Err(federated_err(format!(
        "Cross-project vault search denied for project(s): {}. \
         Set `cross_project_vault = true` for every listed project \
         (e.g. `rms-memory config --cross-project-vault true --scope <path>`), \
         or pass `corpus=code`. Silent degrade to code-only is forbidden.",
        denied.join(", ")
    )))
}

async fn search_single_store(
    store: &crate::store::Store,
    query_vector: Vec<f32>,
    query: &str,
    limit: usize,
    min_confidence: Option<f32>,
    corpus: Corpus,
) -> Result<(Vec<UnifiedSearchResult>, Option<String>)> {
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
    Ok((results, retrieval_mode))
}

async fn search_federated(
    projects: &[ResolvedProject],
    query_vector: Vec<f32>,
    query: &str,
    limit: usize,
    min_confidence: Option<f32>,
    corpus: Corpus,
) -> Result<(Vec<UnifiedSearchResult>, Option<String>)> {
    // Open stores sequentially (LanceDB open is the heavy part; the query
    // vector was already embedded once above, so we don't contend on Indexer).
    let mut per_project_lists: Vec<Vec<UnifiedSearchResult>> = Vec::with_capacity(projects.len());
    let mut retrieval_mode: Option<String> = None;
    for project in projects {
        let workspace =
            crate::workspace::Workspace::discover(std::path::Path::new(&project.code_path), None)?;
        let store = crate::store::Store::for_workspace(&workspace).await?;
        let (hits, mode) = search_single_store(
            &store,
            query_vector.clone(),
            query,
            limit,
            min_confidence,
            corpus,
        )
        .await?;
        if retrieval_mode.is_none() {
            retrieval_mode = mode;
        }
        per_project_lists.push(
            hits.into_iter()
                .map(|hit| hit.with_project(&project.key))
                .collect(),
        );
    }

    let results = if projects.len() == 1 {
        // Single-element `projects` is the same scope as `project=A` — no
        // cross-list fusion needed; just return the tagged hits.
        per_project_lists.into_iter().next().unwrap_or_default()
    } else {
        reciprocal_rank_fusion_lists(per_project_lists, limit)
    };
    Ok((results, retrieval_mode))
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
    reciprocal_rank_fusion_lists(vec![vault, code], limit)
}

/// Reciprocal Rank Fusion over an arbitrary number of ranked lists.
///
/// Used for both intra-project vault+code fusion and cross-project federation.
pub fn reciprocal_rank_fusion_lists(
    lists: Vec<Vec<UnifiedSearchResult>>,
    limit: usize,
) -> Vec<UnifiedSearchResult> {
    let mut merged = HashMap::<String, UnifiedSearchResult>::new();
    for list in lists {
        for (rank, result) in list.into_iter().enumerate() {
            accumulate_rrf(&mut merged, result, rank);
        }
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
            project: None,
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

    fn resolved(key: &str, allow: bool) -> ResolvedProject {
        ResolvedProject {
            key: key.to_string(),
            cross_project_vault: allow,
            code_path: format!("/tmp/{key}"),
        }
    }

    #[test]
    fn cross_project_vault_gate_skipped_for_single_project() {
        // Single-element projects=[A] + vault without flag → OK (gate not consulted).
        enforce_cross_project_vault_policy(&[resolved("A", false)], Corpus::Vault).unwrap();
        enforce_cross_project_vault_policy(&[resolved("A", false)], Corpus::All).unwrap();
    }

    #[test]
    fn cross_project_vault_gate_allows_code_without_opt_in() {
        enforce_cross_project_vault_policy(
            &[resolved("A", false), resolved("B", false)],
            Corpus::Code,
        )
        .unwrap();
    }

    #[test]
    fn cross_project_vault_hard_fails_without_allow() {
        let error = enforce_cross_project_vault_policy(
            &[resolved("A", false), resolved("B", true)],
            Corpus::Vault,
        )
        .unwrap_err();
        let policy = error
            .downcast_ref::<FederatedSearchError>()
            .expect("policy denial must be FederatedSearchError (hard tool error, not abstain)");
        assert!(
            policy.0.contains("A") && policy.0.contains("cross_project_vault"),
            "got: {}",
            policy.0
        );
    }

    #[test]
    fn cross_project_vault_all_or_nothing_on_partial_allow() {
        // corpus=all with partial allow also fails the whole call.
        let error = enforce_cross_project_vault_policy(
            &[resolved("A", true), resolved("B", false)],
            Corpus::All,
        )
        .unwrap_err();
        assert!(error.downcast_ref::<FederatedSearchError>().is_some());
        assert!(error.to_string().contains("B"));
    }

    #[test]
    fn cross_project_vault_ok_when_all_opted_in() {
        enforce_cross_project_vault_policy(
            &[resolved("A", true), resolved("B", true)],
            Corpus::Vault,
        )
        .unwrap();
    }

    #[test]
    fn parse_projects_prefers_projects_when_project_also_set() {
        let mut args = serde_json::Map::new();
        args.insert("project".into(), serde_json::json!("A"));
        args.insert("projects".into(), serde_json::json!(["A", "B"]));
        let keys = parse_projects_arg(&args).unwrap().unwrap();
        assert_eq!(keys, vec!["A".to_string(), "B".to_string()]);
    }

    #[test]
    fn parse_projects_rejects_over_cap() {
        let mut args = serde_json::Map::new();
        let keys: Vec<String> = (0..9).map(|i| format!("p{i}")).collect();
        args.insert("projects".into(), serde_json::json!(keys));
        let error = parse_projects_arg(&args).unwrap_err();
        assert!(error.downcast_ref::<FederatedSearchError>().is_some());
        assert!(error.to_string().contains("at most"));
    }

    #[test]
    fn parse_projects_rejects_empty_array() {
        let mut args = serde_json::Map::new();
        args.insert("projects".into(), serde_json::json!([]));
        let error = parse_projects_arg(&args).unwrap_err();
        assert!(error.downcast_ref::<FederatedSearchError>().is_some());
    }

    #[test]
    fn parse_projects_dedupes_and_preserves_order() {
        let mut args = serde_json::Map::new();
        args.insert("projects".into(), serde_json::json!(["B", "A", "B"]));
        let keys = parse_projects_arg(&args).unwrap().unwrap();
        assert_eq!(keys, vec!["B".to_string(), "A".to_string()]);
    }

    #[test]
    fn federated_identity_keeps_same_path_in_different_projects_apart() {
        let mut a = result("code", "src/lib.rs");
        a.project = Some("proj-a".into());
        let mut b = result("code", "src/lib.rs");
        b.project = Some("proj-b".into());
        assert_ne!(a.identity(), b.identity());
        let fused = reciprocal_rank_fusion_lists(vec![vec![a], vec![b]], 10);
        assert_eq!(fused.len(), 2);
    }
}
