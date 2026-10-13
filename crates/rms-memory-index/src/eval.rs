//! Retrieval evaluation: how well an embedding model finds the right note for
//! a query, measured on the user's own vault.

use crate::embedding::EmbeddingProfile;
use crate::indexer::Indexer;
use crate::store::Store;
use anyhow::{Context, Result, bail};
use rms_memory_core::workspace::Workspace;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Instant;

/// Unique notes requested per query; ranks past this count as misses.
const RANKED_NOTES: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvalCase {
    pub query: String,
    /// Vault-relative paths of notes that answer the query.
    pub expect: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Metrics {
    pub hit_at_1: f64,
    pub hit_at_k: f64,
    /// Mean reciprocal rank over the top `RANKED_NOTES` notes.
    pub mrr: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModelEval {
    pub model: String,
    pub dimension: usize,
    /// The project's existing index for this model was synced and reused.
    pub reused_index: bool,
    pub index_seconds: f64,
    pub mean_query_ms: f64,
    /// Embedding similarity alone.
    pub vector: Metrics,
    /// What `rms_search` does: embeddings plus full-text search.
    pub hybrid: Metrics,
    /// Per case, the rank of the expected note: embeddings only, then hybrid.
    pub ranks: Vec<(Option<usize>, Option<usize>)>,
}

/// Load JSONL cases: one `{"query": "...", "expect": ["path.md"]}` per line.
pub fn load_cases(path: &Path) -> Result<Vec<EvalCase>> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Cannot read eval cases {}", path.display()))?;
    let mut cases = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
            continue;
        }
        let case: EvalCase = serde_json::from_str(line)
            .with_context(|| format!("{}:{}: invalid eval case", path.display(), number + 1))?;
        if case.query.trim().is_empty() || case.expect.is_empty() {
            bail!(
                "{}:{}: a case needs a query and at least one expected path",
                path.display(),
                number + 1
            );
        }
        cases.push(case);
    }
    Ok(cases)
}

/// One case per note: its first `# ` heading (or file name) as the query.
/// Quick to start with, but easier than real questions; hand-written cases
/// measure retrieval better.
pub fn title_cases(workspace: &Workspace, max_cases: usize) -> Result<Vec<EvalCase>> {
    let mut candidates = Vec::new();
    for file in workspace.find_markdown_files()? {
        let Ok(relative) = file.strip_prefix(&workspace.root) else {
            continue;
        };
        let Ok((doc, _)) = rms_memory_core::document::Document::parse_for_index(&file) else {
            continue;
        };
        if Indexer::chunk_text(&doc.content).is_empty() {
            continue;
        }
        let heading = doc
            .content
            .lines()
            .find_map(|line| line.strip_prefix("# "))
            .map(str::trim)
            .filter(|heading| !heading.is_empty());
        let query = match heading {
            Some(heading) => heading.to_string(),
            None => relative
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .replace(['-', '_'], " "),
        };
        if query.split_whitespace().count() < 2 {
            continue;
        }
        candidates.push(EvalCase {
            query,
            expect: vec![relative.to_string_lossy().to_string()],
        });
    }
    candidates.sort_by(|a, b| a.expect.cmp(&b.expect));
    if max_cases == 0 || candidates.len() <= max_cases {
        return Ok(candidates);
    }
    let step = candidates.len() as f64 / max_cases as f64;
    Ok((0..max_cases)
        .map(|index| candidates[(index as f64 * step) as usize].clone())
        .collect())
}

/// Rank (1-based) of the first expected note among unique result paths.
fn rank_of(result_paths: &[String], expect: &[String]) -> Option<usize> {
    let mut seen = Vec::<&str>::new();
    for path in result_paths {
        if seen.contains(&path.as_str()) {
            continue;
        }
        seen.push(path);
        if expect.iter().any(|expected| expected == path) {
            return Some(seen.len());
        }
        if seen.len() == RANKED_NOTES {
            break;
        }
    }
    None
}

fn metrics(ranks: &[Option<usize>], k: usize) -> Metrics {
    if ranks.is_empty() {
        return Metrics::default();
    }
    let count = ranks.len() as f64;
    Metrics {
        hit_at_1: ranks.iter().filter(|rank| **rank == Some(1)).count() as f64 / count,
        hit_at_k: ranks
            .iter()
            .filter(|rank| rank.is_some_and(|rank| rank <= k))
            .count() as f64
            / count,
        mrr: ranks
            .iter()
            .map(|rank| rank.map_or(0.0, |rank| 1.0 / rank as f64))
            .sum::<f64>()
            / count,
    }
}

/// Index the vault with `profile` and score `cases`. Reuses (and syncs) the
/// project's own index for that model when one exists; otherwise builds a
/// throwaway index under `scratch_root` and deletes it afterwards.
pub async fn evaluate_model(
    workspace: &Workspace,
    profile: &EmbeddingProfile,
    cases: &[EvalCase],
    k: usize,
    scratch_root: &Path,
) -> Result<ModelEval> {
    let project_dir = profile.index_dir(
        &rms_memory_core::workspace::base_dir()
            .join("dbs")
            .join(workspace.project_hash()?),
    );
    let reused_index = project_dir.join("metadata.json").exists();
    let storage: PathBuf = if reused_index {
        project_dir
    } else {
        scratch_root.join(format!(
            "{}-{}",
            slug_for_dir(&profile.id),
            std::process::id()
        ))
    };
    let result = run_model(workspace, profile, cases, k, &storage, reused_index).await;
    if !reused_index {
        let _ = std::fs::remove_dir_all(&storage);
    }
    result
}

async fn run_model(
    workspace: &Workspace,
    profile: &EmbeddingProfile,
    cases: &[EvalCase],
    k: usize,
    storage: &Path,
    reused_index: bool,
) -> Result<ModelEval> {
    let mut indexer = Indexer::for_profile(profile.clone())?;
    let store = Store::init_for_profile(&storage.to_string_lossy(), "memory", profile).await?;

    let started = Instant::now();
    crate::indexer::sync_vault(workspace, &store, &mut indexer).await?;
    let index_seconds = started.elapsed().as_secs_f64();

    let mut vector_ranks = Vec::with_capacity(cases.len());
    let mut hybrid_ranks = Vec::with_capacity(cases.len());
    let mut query_time = 0.0;
    // Several chunks of one note can fill the result list.
    let limit = RANKED_NOTES * 4;
    for case in cases {
        let started = Instant::now();
        let vector = indexer.embed_query(&case.query)?;
        query_time += started.elapsed().as_secs_f64();
        let (vector_hits, _) = store
            .search_with_mode(vector.clone(), String::new(), limit, None)
            .await?;
        let (hybrid_hits, _) = store
            .search_with_mode(vector, case.query.clone(), limit, None)
            .await?;
        let paths = |hits: Vec<crate::store::SearchResult>| {
            hits.into_iter().map(|hit| hit.path).collect::<Vec<_>>()
        };
        vector_ranks.push(rank_of(&paths(vector_hits), &case.expect));
        hybrid_ranks.push(rank_of(&paths(hybrid_hits), &case.expect));
    }

    Ok(ModelEval {
        model: profile.id.clone(),
        dimension: profile.dimension,
        reused_index,
        index_seconds,
        mean_query_ms: if cases.is_empty() {
            0.0
        } else {
            query_time * 1000.0 / cases.len() as f64
        },
        vector: metrics(&vector_ranks, k),
        hybrid: metrics(&hybrid_ranks, k),
        ranks: vector_ranks.into_iter().zip(hybrid_ranks).collect(),
    })
}

fn slug_for_dir(id: &str) -> String {
    id.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn rank_counts_unique_notes_not_chunks() {
        let results = paths(&["a.md", "a.md", "b.md", "c.md"]);
        assert_eq!(rank_of(&results, &paths(&["c.md"])), Some(3));
        assert_eq!(rank_of(&results, &paths(&["x.md", "a.md"])), Some(1));
        assert_eq!(rank_of(&results, &paths(&["x.md"])), None);
    }

    #[test]
    fn metrics_average_hits_and_reciprocal_ranks() {
        let scored = metrics(&[Some(1), Some(2), None, Some(5)], 3);
        assert_eq!(scored.hit_at_1, 0.25);
        assert_eq!(scored.hit_at_k, 0.5);
        assert!((scored.mrr - (1.0 + 0.5 + 0.2) / 4.0).abs() < 1e-9);
    }

    #[test]
    fn load_cases_reads_jsonl_and_rejects_empty_expectations() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("cases.jsonl");
        std::fs::write(
            &file,
            "# comment\n{\"query\": \"how do we release\", \"expect\": [\"docs/release.md\"]}\n\n",
        )
        .unwrap();
        let cases = load_cases(&file).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].expect, vec!["docs/release.md".to_string()]);

        std::fs::write(&file, "{\"query\": \"q\", \"expect\": []}\n").unwrap();
        assert!(load_cases(&file).is_err());
    }
}
