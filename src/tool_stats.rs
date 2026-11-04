//! Local, content-free usage statistics: how many characters each MCP method/tool put into
//! the agent's context. Used to find where tokens go (`rms-memory stats`).
//!
//! Stored in `~/.rms-memory/tool-stats.jsonl`: timestamp, method, tool name, project key,
//! success flag and response size. Never queries, paths or note content. Nothing leaves the machine.
//! `RMS_MEMORY_NO_STATS=1` disables recording.

use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
/// Rough chars-per-token ratio used for estimates (varies by language and tokenizer).
pub const CHARS_PER_TOKEN: usize = 4;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Entry {
    pub ts: DateTime<Utc>,
    /// JSON-RPC method (`tools/call`, `tools/list`, `initialize`, ...).
    pub method: String,
    /// Tool name for `tools/call`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub ok: bool,
    pub chars: usize,
}

pub fn disabled() -> bool {
    std::env::var("RMS_MEMORY_NO_STATS").is_ok_and(|v| !v.is_empty() && v != "0" && v != "false")
}

pub fn default_path() -> PathBuf {
    crate::workspace::base_dir().join("tool-stats.jsonl")
}

/// Append one entry. Failures are ignored: statistics must never break a tool call.
pub fn record(path: &Path, entry: &Entry) {
    let _ = try_record(path, entry);
}

fn try_record(path: &Path, entry: &Entry) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    if std::fs::metadata(path).is_ok_and(|m| m.len() > MAX_FILE_BYTES) {
        // Keep one previous generation; older history is dropped.
        let _ = std::fs::rename(path, path.with_extension("jsonl.1"));
    }
    let mut line = serde_json::to_string(entry)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())?;
    Ok(())
}

pub fn read_all(path: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    for p in [path.with_extension("jsonl.1"), path.to_path_buf()] {
        if let Ok(text) = std::fs::read_to_string(p) {
            entries.extend(
                text.lines()
                    .filter_map(|l| serde_json::from_str::<Entry>(l).ok()),
            );
        }
    }
    entries
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Row {
    pub name: String,
    pub calls: usize,
    pub errors: usize,
    pub total_chars: usize,
    pub avg_chars: usize,
    pub max_chars: usize,
    pub approx_tokens: usize,
}

/// Aggregate entries newer than `now - days`, largest total first.
pub fn summarize(entries: &[Entry], days: i64, now: DateTime<Utc>) -> Vec<Row> {
    let since = now - Duration::days(days);
    let mut map: BTreeMap<String, (usize, usize, usize, usize)> = BTreeMap::new();
    for e in entries.iter().filter(|e| e.ts >= since) {
        let name = e.tool.clone().unwrap_or_else(|| e.method.clone());
        let slot = map.entry(name).or_default();
        slot.0 += 1;
        slot.1 += usize::from(!e.ok);
        slot.2 += e.chars;
        slot.3 = slot.3.max(e.chars);
    }
    let mut rows: Vec<Row> = map
        .into_iter()
        .map(|(name, (calls, errors, total, max))| Row {
            name,
            calls,
            errors,
            total_chars: total,
            avg_chars: total / calls.max(1),
            max_chars: max,
            approx_tokens: total / CHARS_PER_TOKEN,
        })
        .collect();
    rows.sort_by(|a, b| b.total_chars.cmp(&a.total_chars).then(a.name.cmp(&b.name)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(ts: &str, method: &str, tool: Option<&str>, ok: bool, chars: usize) -> Entry {
        Entry {
            ts: DateTime::parse_from_rfc3339(ts)
                .unwrap()
                .with_timezone(&Utc),
            method: method.into(),
            tool: tool.map(str::to_string),
            project: None,
            ok,
            chars,
        }
    }

    #[test]
    fn record_read_and_summarize_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stats.jsonl");
        let now = DateTime::parse_from_rfc3339("2026-11-10T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        record(
            &path,
            &entry(
                "2026-11-09T10:00:00Z",
                "tools/call",
                Some("rms_search"),
                true,
                2000,
            ),
        );
        record(
            &path,
            &entry(
                "2026-11-09T11:00:00Z",
                "tools/call",
                Some("rms_search"),
                true,
                4000,
            ),
        );
        record(
            &path,
            &entry(
                "2026-11-09T12:00:00Z",
                "tools/call",
                Some("rms_read"),
                false,
                100,
            ),
        );
        record(
            &path,
            &entry("2026-11-09T12:00:00Z", "tools/list", None, true, 18000),
        );
        record(
            &path,
            &entry(
                "2026-09-01T12:00:00Z",
                "tools/call",
                Some("rms_search"),
                true,
                999_999,
            ),
        );
        let rows = summarize(&read_all(&path), 30, now);
        assert_eq!(rows[0].name, "tools/list");
        assert_eq!(rows[0].approx_tokens, 4500);
        let search = rows.iter().find(|r| r.name == "rms_search").unwrap();
        assert_eq!(
            (
                search.calls,
                search.total_chars,
                search.avg_chars,
                search.max_chars
            ),
            (2, 6000, 3000, 4000)
        );
        let read = rows.iter().find(|r| r.name == "rms_read").unwrap();
        assert_eq!(read.errors, 1);
    }

    #[test]
    fn entries_hold_no_content_fields_and_bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        record(
            &path,
            &entry(
                "2026-11-09T10:00:00Z",
                "tools/call",
                Some("rms_write"),
                true,
                10,
            ),
        );
        let text = std::fs::read_to_string(&path).unwrap();
        for forbidden in ["query", "path\"", "content", "text"] {
            assert!(!text.contains(forbidden), "{text}");
        }
        std::fs::write(&path, format!("garbage\n{text}")).unwrap();
        assert_eq!(read_all(&path).len(), 1);
    }

    #[test]
    fn oversized_file_rotates() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.jsonl");
        std::fs::write(&path, vec![b'x'; (MAX_FILE_BYTES + 1) as usize]).unwrap();
        record(
            &path,
            &entry("2026-11-09T10:00:00Z", "initialize", None, true, 5),
        );
        assert!(path.with_extension("jsonl.1").exists());
        assert_eq!(read_all(&path).len(), 1);
    }
}
