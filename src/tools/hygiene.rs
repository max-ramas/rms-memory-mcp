//! Memory hygiene: a read-only report of consolidation candidates.
//!
//! Nothing here edits the vault. The agent merges duplicates with
//! `rms_write` + `supersedes` and closes stale checkpoints with
//! `rms_checkpoint_done`; old session notes can be pruned or archived.

use super::AppContext;
use super::continuity::{CHECKPOINTS_DIR, SESSIONS_DIR};
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::Path;

pub const STALE_CHECKPOINT_DAYS: i64 = 14;
pub const OLD_SESSION_DAYS: i64 = 90;
/// Embedding similarity that makes a pair a candidate. Small local models
/// rate most notes of one vault as similar, so candidates are confirmed
/// lexically with [`MIN_OVERLAP`].
pub const DEFAULT_MIN_SIMILARITY: f32 = 0.9;
/// Share of word 3-grams two notes must have in common.
pub const MIN_OVERLAP: f32 = 0.5;
/// Stubs and templates shorter than this are not compared.
const MIN_WORDS: usize = 30;
/// Pairwise comparison is quadratic; larger vaults are compared on a prefix.
const MAX_COMPARED_DOCUMENTS: usize = 3000;
const MAX_REPORTED: usize = 30;

#[derive(Serialize, Debug, Clone, PartialEq)]
pub struct SimilarPair {
    pub a: String,
    pub b: String,
    /// Cosine similarity of the note embeddings.
    #[serde(serialize_with = "three_decimals")]
    pub similarity: f32,
    /// Jaccard overlap of word 3-grams.
    #[serde(serialize_with = "three_decimals")]
    pub overlap: f32,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct AgedNote {
    pub path: String,
    pub changed_at: String,
    pub age_days: i64,
}

#[derive(Serialize, Debug, Clone, Default)]
pub struct HygieneReport {
    pub compared_documents: usize,
    /// More documents are indexed than were compared for near duplicates.
    pub truncated: bool,
    #[serde(serialize_with = "three_decimals")]
    pub min_similarity: f32,
    pub near_duplicates: Vec<SimilarPair>,
    /// Groups of notes with identical bodies.
    pub exact_duplicates: Vec<Vec<String>>,
    pub stale_checkpoints: Vec<AgedNote>,
    pub old_sessions: Vec<AgedNote>,
    pub old_sessions_total: usize,
    pub next_steps: Vec<String>,
}

fn parse_time(raw: &str) -> Option<DateTime<Utc>> {
    crate::doctor::parse_fm_datetime(raw)
}

/// Active checkpoints not updated for more than [`STALE_CHECKPOINT_DAYS`].
pub fn stale_checkpoints(vault_root: &Path, now: DateTime<Utc>) -> Result<Vec<AgedNote>> {
    let mut stale = Vec::new();
    for checkpoint in super::continuity::checkpoint_query(vault_root, Some("active"))? {
        let Some(changed_at) = checkpoint.changed_at.or(checkpoint.created_at) else {
            continue;
        };
        let Some(time) = parse_time(&changed_at) else {
            continue;
        };
        let age_days = (now - time).num_days();
        if age_days > STALE_CHECKPOINT_DAYS {
            stale.push(AgedNote {
                path: checkpoint.path,
                changed_at,
                age_days,
            });
        }
    }
    stale.sort_by_key(|note| std::cmp::Reverse(note.age_days));
    Ok(stale)
}

/// Session summaries older than [`OLD_SESSION_DAYS`], oldest first.
pub fn old_sessions(vault_root: &Path, now: DateTime<Utc>) -> Vec<AgedNote> {
    let Ok(entries) = std::fs::read_dir(vault_root.join(SESSIONS_DIR)) else {
        return Vec::new();
    };
    let mut old = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(doc) = crate::document::Document::parse(&path) else {
            continue;
        };
        let fm = doc.frontmatter.unwrap_or_default();
        let Some(changed_at) = fm.created_at.or(fm.timestamp) else {
            continue;
        };
        let Some(time) = parse_time(&changed_at) else {
            continue;
        };
        let age_days = (now - time).num_days();
        if age_days > OLD_SESSION_DAYS {
            old.push(AgedNote {
                path: format!(
                    "{SESSIONS_DIR}/{}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ),
                changed_at,
                age_days,
            });
        }
    }
    old.sort_by_key(|note| std::cmp::Reverse(note.age_days));
    old
}

fn is_session_artifact(path: &str) -> bool {
    path.starts_with(&format!("{CHECKPOINTS_DIR}/"))
        || path.starts_with(&format!("{SESSIONS_DIR}/"))
}

fn shingles(text: &str) -> Option<HashSet<u64>> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect();
    if words.len() < MIN_WORDS {
        return None;
    }
    Some(
        words
            .windows(3)
            .map(|window| {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                window.hash(&mut hasher);
                hasher.finish()
            })
            .collect(),
    )
}

fn jaccard(a: &HashSet<u64>, b: &HashSet<u64>) -> f32 {
    let shared = a.intersection(b).count();
    let total = a.len() + b.len() - shared;
    if total == 0 {
        0.0
    } else {
        shared as f32 / total as f32
    }
}

fn rounded(value: f32) -> f32 {
    (value * 1000.0).round() / 1000.0
}

/// `serde_json::Value` widens f32 to f64, turning 0.9 into 0.8999999761581421.
fn three_decimals<S: serde::Serializer>(value: &f32, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_f64((f64::from(*value) * 1000.0).round() / 1000.0)
}

/// Exact duplicate groups and the most similar remaining pairs.
pub fn similar_documents(
    documents: &[crate::store::DocumentVector],
    min_similarity: f32,
) -> (Vec<SimilarPair>, Vec<Vec<String>>) {
    let candidates: Vec<_> = documents
        .iter()
        .filter(|doc| !is_session_artifact(&doc.path))
        .filter(|doc| {
            !doc.status
                .as_deref()
                .is_some_and(|status| status.eq_ignore_ascii_case("superseded"))
        })
        .filter_map(|doc| shingles(&doc.text).map(|shingles| (doc, shingles)))
        .collect();

    let mut by_hash: BTreeMap<&str, Vec<String>> = BTreeMap::new();
    for (doc, _) in &candidates {
        by_hash
            .entry(doc.content_hash.as_str())
            .or_default()
            .push(doc.path.clone());
    }
    let exact: Vec<Vec<String>> = by_hash
        .into_values()
        .filter(|paths| paths.len() > 1)
        .collect();

    let mut pairs = Vec::new();
    for (index, (left, left_shingles)) in candidates.iter().enumerate() {
        for (right, right_shingles) in &candidates[index + 1..] {
            if left.content_hash == right.content_hash {
                continue;
            }
            let similarity: f32 = left
                .vector
                .iter()
                .zip(&right.vector)
                .map(|(a, b)| a * b)
                .sum();
            if similarity < min_similarity {
                continue;
            }
            let overlap = jaccard(left_shingles, right_shingles);
            if overlap >= MIN_OVERLAP {
                pairs.push(SimilarPair {
                    a: left.path.clone(),
                    b: right.path.clone(),
                    similarity: rounded(similarity),
                    overlap: rounded(overlap),
                });
            }
        }
    }
    pairs.sort_by(|x, y| y.overlap.total_cmp(&x.overlap));
    pairs.truncate(MAX_REPORTED);
    (pairs, exact)
}

pub async fn report(
    workspace: &crate::workspace::Workspace,
    store: &crate::store::Store,
    min_similarity: f32,
) -> Result<HygieneReport> {
    let now = Utc::now();
    let table = store.open_table().await?;
    let mut documents = store.document_vectors(&table).await?;
    let truncated = documents.len() > MAX_COMPARED_DOCUMENTS;
    documents.truncate(MAX_COMPARED_DOCUMENTS);
    let (near_duplicates, exact_duplicates) = similar_documents(&documents, min_similarity);
    let stale_checkpoints = stale_checkpoints(&workspace.root, now)?;
    let mut old_sessions = old_sessions(&workspace.root, now);
    let old_sessions_total = old_sessions.len();
    old_sessions.truncate(MAX_REPORTED);

    let mut next_steps = Vec::new();
    if !near_duplicates.is_empty() || !exact_duplicates.is_empty() {
        next_steps.push("Read each duplicate pair; keep the better note, fold in what the other adds, and write it with `rms_write` setting `supersedes` to the other note's id.".to_string());
    }
    if !stale_checkpoints.is_empty() {
        next_steps.push("Close finished stale checkpoints with `rms_checkpoint_done`, or refresh them with `rms_checkpoint_save` if the work continues.".to_string());
    }
    if old_sessions_total > 0 {
        next_steps.push("Fold lasting knowledge from old session notes into decisions/ or rules/ notes written with `supersedes` set to the session's id; `rms_prune` then archives the superseded sessions.".to_string());
    }
    Ok(HygieneReport {
        compared_documents: documents.len(),
        truncated,
        min_similarity,
        near_duplicates,
        exact_duplicates,
        stale_checkpoints,
        old_sessions,
        old_sessions_total,
        next_steps,
    })
}

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let store = ctx
        .store
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
    let workspace = super::graph::workspace_from_ctx(ctx)?;
    let min_similarity = args
        .get("min_similarity")
        .and_then(|v| v.as_f64())
        .map(|v| (v as f32).clamp(0.5, 1.0))
        .unwrap_or(DEFAULT_MIN_SIMILARITY);
    let payload = report(&workspace, store, min_similarity).await?;
    super::response::json_structured_response(&payload)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "The billing service stores every payment attempt in Postgres and retries failed charges three times with exponential backoff before it marks the invoice as failed and notifies the account owner by email so support can follow up manually";

    fn doc(path: &str, text: &str, vector: [f32; 2]) -> crate::store::DocumentVector {
        let norm = (vector[0] * vector[0] + vector[1] * vector[1]).sqrt();
        crate::store::DocumentVector {
            path: path.to_string(),
            status: None,
            content_hash: blake3::hash(text.as_bytes()).to_string(),
            text: text.to_string(),
            vector: vector.iter().map(|v| v / norm).collect(),
        }
    }

    #[test]
    fn scores_serialize_with_three_decimals() {
        let report = HygieneReport {
            min_similarity: 0.9,
            ..Default::default()
        };
        let value = serde_json::to_value(&report).unwrap();
        assert_eq!(value["min_similarity"].to_string(), "0.9");
    }

    #[test]
    fn reports_exact_and_near_duplicates_but_not_session_artifacts() {
        let edited = format!("{BODY} and the retry count is configurable per merchant");
        let unrelated = BODY.split(' ').rev().collect::<Vec<_>>().join(" ");
        let mut superseded = doc("decisions/old.md", &format!("{BODY} old"), [1.0, 0.0]);
        superseded.status = Some("superseded".to_string());
        let documents = vec![
            doc("decisions/db.md", BODY, [1.0, 0.0]),
            doc("decisions/db-copy.md", BODY, [1.0, 0.0]),
            doc("rules/db.md", &edited, [0.99, 0.05]),
            doc("docs/same-topic.md", &unrelated, [0.99, 0.05]),
            doc("docs/far.md", &format!("{edited} far"), [0.0, 1.0]),
            doc("docs/stub.md", "# Notes", [1.0, 0.0]),
            doc("docs/stub-copy.md", "# Notes", [1.0, 0.0]),
            doc("artifacts/sessions/s.md", BODY, [1.0, 0.0]),
            superseded,
        ];
        let (pairs, exact) = similar_documents(&documents, 0.9);
        assert_eq!(
            exact,
            vec![vec![
                "decisions/db.md".to_string(),
                "decisions/db-copy.md".to_string()
            ]]
        );
        let names: Vec<_> = pairs
            .iter()
            .map(|pair| (pair.a.as_str(), pair.b.as_str()))
            .collect();
        assert_eq!(
            names,
            vec![
                ("decisions/db.md", "rules/db.md"),
                ("decisions/db-copy.md", "rules/db.md")
            ]
        );
    }

    #[test]
    fn stale_checkpoints_and_old_sessions_are_aged_from_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(CHECKPOINTS_DIR)).unwrap();
        std::fs::create_dir_all(root.join(SESSIONS_DIR)).unwrap();
        std::fs::write(
            root.join(CHECKPOINTS_DIR).join("old.md"),
            "---\nid: c1\ntype: checkpoint\nstatus: active\ntimestamp: '2026-01-01T00:00:00Z'\n---\n# Checkpoint: old\n",
        )
        .unwrap();
        std::fs::write(
            root.join(CHECKPOINTS_DIR).join("fresh.md"),
            "---\nid: c2\ntype: checkpoint\nstatus: active\ntimestamp: '2026-10-10T00:00:00Z'\n---\n# Checkpoint: fresh\n",
        )
        .unwrap();
        std::fs::write(
            root.join(SESSIONS_DIR).join("s.md"),
            "---\nid: s1\ntype: session\ncreated_at: '2026-01-01T00:00:00Z'\n---\n# Session\n",
        )
        .unwrap();
        let now = parse_time("2026-10-13T00:00:00Z").unwrap();
        let stale = stale_checkpoints(root, now).unwrap();
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].path, "artifacts/checkpoints/old.md");
        assert_eq!(stale[0].age_days, 285);
        let sessions = old_sessions(root, now);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].path, "artifacts/sessions/s.md");
    }
}
