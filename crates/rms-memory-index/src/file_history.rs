//! Derived `code_path` git file→commit history cache (Lance).
//!
//! See `decisions/file-git-history-index-2026-09-08.md`:
//! normalized `(file_path, commit_sha)` rows, `--no-merges`, no rename follow,
//! failure-atomic `last_indexed_sha` advancement.

use anyhow::{Context, Result, anyhow};
use futures::stream::StreamExt;
use lancedb::arrow::arrow_array::RecordBatch;
use lancedb::arrow::arrow_array::builder::StringBuilder;
use lancedb::arrow::arrow_array::{Array, StringArray};
use lancedb::arrow::arrow_schema::{DataType, Field, Schema as ArrowSchema};
use lancedb::query::{ExecutableQuery, QueryBase};
use lancedb::table::Table;
use std::collections::HashSet;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use crate::store::Store;

const META_KEY: &str = "default";

/// Max unique commits a lazy catch-up may ingest before failing closed.
pub const MAX_CATCH_UP_COMMITS: usize = 2_000;
/// Max unique commits a full reindex may ingest before failing closed.
pub const MAX_REINDEX_COMMITS: usize = 50_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHistoryRecord {
    pub file_path: String,
    pub commit_sha: String,
    pub author: String,
    pub committed_at: String,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct FileHistoryCatchUpReport {
    pub from_sha: Option<String>,
    pub to_sha: String,
    pub commits_scanned: usize,
    pub rows_upserted: usize,
    pub noop: bool,
}

impl Store {
    pub fn file_history_table_name(&self) -> String {
        format!("{}_file_git_history", self.table_name)
    }

    pub fn file_history_meta_table_name(&self) -> String {
        format!("{}_file_git_history_meta", self.table_name)
    }

    pub fn file_history_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("file_path", DataType::Utf8, false),
            Field::new("commit_sha", DataType::Utf8, false),
            Field::new("author", DataType::Utf8, false),
            Field::new("committed_at", DataType::Utf8, false),
            Field::new("message", DataType::Utf8, true),
        ]))
    }

    pub fn file_history_meta_schema() -> Arc<ArrowSchema> {
        Arc::new(ArrowSchema::new(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("last_indexed_sha", DataType::Utf8, false),
            Field::new("updated_at", DataType::Utf8, false),
        ]))
    }

    async fn open_or_create_named_table(
        &self,
        name: &str,
        schema: Arc<ArrowSchema>,
    ) -> Result<Table> {
        match self.db.open_table(name).execute().await {
            Ok(table) => Ok(table),
            Err(open_error) => match self.db.create_empty_table(name, schema).execute().await {
                Ok(table) => Ok(table),
                Err(create_error) => self.db.open_table(name).execute().await.with_context(|| {
                    format!(
                        "could not open table {name} after create race; open={open_error}; create={create_error}"
                    )
                }),
            },
        }
    }

    pub async fn open_or_create_file_history_tables(&self) -> Result<(Table, Table)> {
        let history = self
            .open_or_create_named_table(
                &self.file_history_table_name(),
                Self::file_history_schema(),
            )
            .await?;
        let meta = self
            .open_or_create_named_table(
                &self.file_history_meta_table_name(),
                Self::file_history_meta_schema(),
            )
            .await?;
        Ok((history, meta))
    }

    pub async fn file_history_last_indexed_sha(&self) -> Result<Option<String>> {
        let (_, meta) = self.open_or_create_file_history_tables().await?;
        let mut stream = meta
            .query()
            .only_if(format!("key = '{}'", escape_filter(META_KEY)))
            .select(lancedb::query::Select::Columns(vec![
                "last_indexed_sha".to_string(),
            ]))
            .execute()
            .await?;
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            if batch.num_rows() == 0 {
                continue;
            }
            let col = batch
                .column_by_name("last_indexed_sha")
                .context("missing last_indexed_sha")?
                .as_any()
                .downcast_ref::<StringArray>()
                .context("last_indexed_sha not Utf8")?;
            return Ok(Some(col.value(0).to_string()));
        }
        Ok(None)
    }

    async fn set_file_history_last_indexed_sha(&self, sha: &str) -> Result<()> {
        validate_git_sha(sha)?;
        let (_, meta) = self.open_or_create_file_history_tables().await?;
        // Delete-then-add: a crash between leaves no meta row (next catch-up
        // replays from empty — expensive but safe). Propagate delete errors so
        // we never add a second row while an old filter still matches.
        meta.delete(&format!("key = '{}'", escape_filter(META_KEY)))
            .await
            .context("failed to clear file_history meta before watermark update")?;
        let batch = meta_record_batch(sha)?;
        meta.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn upsert_file_history_records(&self, records: &[FileHistoryRecord]) -> Result<()> {
        if records.is_empty() {
            return Ok(());
        }
        let (history, _) = self.open_or_create_file_history_tables().await?;
        // Idempotent replay: drop existing pairs then append.
        for record in records {
            validate_git_sha(&record.commit_sha)?;
            let filter = format!(
                "file_path = '{}' AND commit_sha = '{}'",
                escape_filter(&record.file_path),
                escape_filter(&record.commit_sha)
            );
            history.delete(&filter).await.with_context(|| {
                format!(
                    "failed to delete prior file_history row {}@{}",
                    record.file_path, record.commit_sha
                )
            })?;
        }
        let batch = history_record_batch(records)?;
        history.add(vec![batch]).execute().await?;
        Ok(())
    }

    pub async fn query_file_history(
        &self,
        file_path: &str,
        limit: usize,
    ) -> Result<Vec<FileHistoryRecord>> {
        let (history, _) = self.open_or_create_file_history_tables().await?;
        let mut stream = history
            .query()
            .only_if(format!("file_path = '{}'", escape_filter(file_path)))
            .select(lancedb::query::Select::Columns(vec![
                "file_path".to_string(),
                "commit_sha".to_string(),
                "author".to_string(),
                "committed_at".to_string(),
                "message".to_string(),
            ]))
            .execute()
            .await?;
        let mut rows = Vec::new();
        while let Some(batch) = stream.next().await {
            let batch = batch?;
            rows.extend(records_from_batch(&batch)?);
        }
        rows.sort_by(|a, b| {
            b.committed_at
                .cmp(&a.committed_at)
                .then(b.commit_sha.cmp(&a.commit_sha))
        });
        rows.truncate(limit.max(1));
        Ok(rows)
    }

    pub async fn wipe_file_history(&self) -> Result<()> {
        let (history, meta) = self.open_or_create_file_history_tables().await?;
        history
            .delete("commit_sha IS NOT NULL")
            .await
            .context("failed to wipe file_history rows")?;
        meta.delete(&format!("key = '{}'", escape_filter(META_KEY)))
            .await
            .context("failed to wipe file_history meta")?;
        Ok(())
    }
}

fn escape_filter(s: &str) -> String {
    s.replace('\'', "''")
}

/// Accept only hex object names suitable as a single `git log` revision arg.
pub fn validate_git_sha(sha: &str) -> Result<()> {
    let ok = (7..=40).contains(&sha.len()) && sha.chars().all(|c| c.is_ascii_hexdigit());
    if !ok {
        return Err(anyhow!(
            "invalid git object name for file history (want 7–40 hex chars): {sha}"
        ));
    }
    Ok(())
}

fn meta_record_batch(sha: &str) -> Result<RecordBatch> {
    let mut key = StringBuilder::new();
    let mut last = StringBuilder::new();
    let mut updated = StringBuilder::new();
    key.append_value(META_KEY);
    last.append_value(sha);
    updated.append_value(chrono::Utc::now().to_rfc3339());
    Ok(RecordBatch::try_new(
        Store::file_history_meta_schema(),
        vec![
            Arc::new(key.finish()),
            Arc::new(last.finish()),
            Arc::new(updated.finish()),
        ],
    )?)
}

fn history_record_batch(records: &[FileHistoryRecord]) -> Result<RecordBatch> {
    let mut file_path = StringBuilder::new();
    let mut commit_sha = StringBuilder::new();
    let mut author = StringBuilder::new();
    let mut committed_at = StringBuilder::new();
    let mut message = StringBuilder::new();
    for record in records {
        file_path.append_value(&record.file_path);
        commit_sha.append_value(&record.commit_sha);
        author.append_value(&record.author);
        committed_at.append_value(&record.committed_at);
        match &record.message {
            Some(m) => message.append_value(m),
            None => message.append_null(),
        }
    }
    Ok(RecordBatch::try_new(
        Store::file_history_schema(),
        vec![
            Arc::new(file_path.finish()),
            Arc::new(commit_sha.finish()),
            Arc::new(author.finish()),
            Arc::new(committed_at.finish()),
            Arc::new(message.finish()),
        ],
    )?)
}

fn records_from_batch(batch: &RecordBatch) -> Result<Vec<FileHistoryRecord>> {
    let file_path = batch
        .column_by_name("file_path")
        .context("file_path")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("file_path Utf8")?;
    let commit_sha = batch
        .column_by_name("commit_sha")
        .context("commit_sha")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("commit_sha Utf8")?;
    let author = batch
        .column_by_name("author")
        .context("author")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("author Utf8")?;
    let committed_at = batch
        .column_by_name("committed_at")
        .context("committed_at")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("committed_at Utf8")?;
    let message = batch
        .column_by_name("message")
        .context("message")?
        .as_any()
        .downcast_ref::<StringArray>()
        .context("message Utf8")?;
    let mut out = Vec::with_capacity(batch.num_rows());
    for i in 0..batch.num_rows() {
        out.push(FileHistoryRecord {
            file_path: file_path.value(i).to_string(),
            commit_sha: commit_sha.value(i).to_string(),
            author: author.value(i).to_string(),
            committed_at: committed_at.value(i).to_string(),
            message: if message.is_null(i) {
                None
            } else {
                Some(message.value(i).to_string())
            },
        });
    }
    Ok(out)
}

/// Resolve `HEAD` SHA for a git work tree.
pub fn git_head(repo: &Path) -> Result<String> {
    let output = Command::new("git")
        .args(["-C", &repo.to_string_lossy(), "rev-parse", "HEAD"])
        .output()
        .context("failed to spawn git rev-parse")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git rev-parse HEAD failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_string())
}

/// Count non-merge commits in a revision range (`A..B` or a single tip).
pub fn git_rev_list_count(repo: &Path, range: &str) -> Result<usize> {
    let output = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "rev-list",
            "--count",
            "--no-merges",
            range,
        ])
        .output()
        .context("failed to spawn git rev-list --count")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git rev-list --count failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    let text = String::from_utf8(output.stdout)?;
    text.trim()
        .parse::<usize>()
        .with_context(|| format!("git rev-list --count produced non-integer: {text}"))
}

fn ensure_commit_budget(repo: &Path, range: &str, max_commits: usize, op: &str) -> Result<()> {
    let count = git_rev_list_count(repo, range)?;
    if count > max_commits {
        return Err(anyhow!(
            "file history {op} spans {count} commits (max {max_commits}); catch up more often or raise the bound carefully"
        ));
    }
    Ok(())
}

/// Parse `git log --no-merges --name-only` for `from_exclusive..to_inclusive`.
/// When `from_exclusive` is `None`, walks the full history to `to_inclusive`.
pub fn git_log_file_commits(
    repo: &Path,
    from_exclusive: Option<&str>,
    to_inclusive: &str,
) -> Result<Vec<FileHistoryRecord>> {
    validate_git_sha(to_inclusive)?;
    if let Some(from) = from_exclusive {
        validate_git_sha(from)?;
    }
    let range = match from_exclusive {
        Some(from) if from == to_inclusive => return Ok(Vec::new()),
        Some(from) => format!("{from}..{to_inclusive}"),
        None => to_inclusive.to_string(),
    };
    let output = Command::new("git")
        .args([
            "-C",
            &repo.to_string_lossy(),
            "log",
            "--no-merges",
            "--name-only",
            "--pretty=format:>>>COMMIT %H|%an|%aI|%s",
            &range,
        ])
        .output()
        .context("failed to spawn git log")?;
    if !output.status.success() {
        return Err(anyhow!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(parse_git_log_name_only(&String::from_utf8(output.stdout)?))
}
pub fn parse_git_log_name_only(stdout: &str) -> Vec<FileHistoryRecord> {
    let mut records = Vec::new();
    let mut current: Option<(String, String, String, String)> = None;
    let mut seen = HashSet::new();

    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix(">>>COMMIT ") {
            current = {
                let mut parts = rest.splitn(4, '|');
                let sha = parts.next().unwrap_or("").to_string();
                let author = parts.next().unwrap_or("").to_string();
                let at = parts.next().unwrap_or("").to_string();
                let subject = parts.next().unwrap_or("").to_string();
                if sha.is_empty() {
                    None
                } else {
                    Some((sha, author, at, subject))
                }
            };
            continue;
        }
        let path = line.trim();
        if path.is_empty() {
            continue;
        }
        let Some((sha, author, at, subject)) = current.clone() else {
            continue;
        };
        let key = (path.to_string(), sha.clone());
        if !seen.insert(key) {
            continue;
        }
        records.push(FileHistoryRecord {
            file_path: path.to_string(),
            commit_sha: sha,
            author,
            committed_at: at,
            message: if subject.is_empty() {
                None
            } else {
                Some(subject)
            },
        });
    }
    records
}

/// Failure-atomic catch-up: append rows for `last..HEAD`, then advance meta.
pub async fn catch_up_file_history(
    store: &Store,
    code_path: &Path,
) -> Result<FileHistoryCatchUpReport> {
    let head = git_head(code_path)?;
    validate_git_sha(&head)?;
    let last = store.file_history_last_indexed_sha().await?;
    if let Some(ref last_sha) = last {
        validate_git_sha(last_sha)?;
    }
    if last.as_deref() == Some(head.as_str()) {
        return Ok(FileHistoryCatchUpReport {
            from_sha: last,
            to_sha: head,
            commits_scanned: 0,
            rows_upserted: 0,
            noop: true,
        });
    }

    let range = match last.as_deref() {
        Some(from) => format!("{from}..{head}"),
        None => head.clone(),
    };
    ensure_commit_budget(code_path, &range, MAX_CATCH_UP_COMMITS, "catch-up")?;

    let records = git_log_file_commits(code_path, last.as_deref(), &head)?;
    let commits_scanned = records
        .iter()
        .map(|r| r.commit_sha.clone())
        .collect::<HashSet<_>>()
        .len();
    let rows = records.len();
    store.upsert_file_history_records(&records).await?;
    // Advance meta only after successful upsert (failure-atomic).
    store.set_file_history_last_indexed_sha(&head).await?;
    Ok(FileHistoryCatchUpReport {
        from_sha: last,
        to_sha: head,
        commits_scanned,
        rows_upserted: rows,
        noop: false,
    })
}

/// Explicit full rebuild: wipe, then index entire history to HEAD.
pub async fn reindex_file_history(
    store: &Store,
    code_path: &Path,
) -> Result<FileHistoryCatchUpReport> {
    let head = git_head(code_path)?;
    validate_git_sha(&head)?;
    ensure_commit_budget(code_path, &head, MAX_REINDEX_COMMITS, "reindex")?;
    store.wipe_file_history().await?;
    let records = git_log_file_commits(code_path, None, &head)?;
    let commits_scanned = records
        .iter()
        .map(|r| r.commit_sha.clone())
        .collect::<HashSet<_>>()
        .len();
    let rows = records.len();
    store.upsert_file_history_records(&records).await?;
    store.set_file_history_last_indexed_sha(&head).await?;
    Ok(FileHistoryCatchUpReport {
        from_sha: None,
        to_sha: head,
        commits_scanned,
        rows_upserted: rows,
        noop: false,
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_name_only_log() {
        let stdout = "\
>>>COMMIT aaa|Alice|2026-01-01T00:00:00Z|first
src/a.rs
src/b.rs

>>>COMMIT bbb|Bob|2026-01-02T00:00:00Z|second
src/a.rs
";
        let rows = parse_git_log_name_only(stdout);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].file_path, "src/a.rs");
        assert_eq!(rows[0].commit_sha, "aaa");
        assert_eq!(rows[2].author, "Bob");
    }

    #[test]
    fn equal_range_is_empty_without_spawning() {
        // Unit-level: git_log with equal ends short-circuits before Command.
        let rows = git_log_file_commits(Path::new("/nonexistent"), Some("abcdef0"), "abcdef0")
            .expect("equal range");
        assert!(rows.is_empty());
    }

    #[test]
    fn rejects_non_hex_sha() {
        assert!(validate_git_sha("deadbeef").is_ok());
        assert!(validate_git_sha("--all").is_err());
        assert!(validate_git_sha("xyz").is_err());
        assert!(validate_git_sha("abcd").is_err());
    }
}
