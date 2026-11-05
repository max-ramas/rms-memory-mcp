//! Glue between the code index and the notes vault.
//!
//! * [`index_info`] tells an agent whether the code index can be trusted right now
//!   (off, never built, behind the working tree, fresh).
//! * [`NoteIndex`] answers "which notes talk about this file / symbol?" so a code hit comes
//!   with the decisions and patterns that apply to it. It is a derived, in-memory view of the
//!   vault (no extra storage) and is rebuilt only when the vault changes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::SystemTime;

use serde_json::{Value, json};

const MAX_NOTES: usize = 5000;
const MAX_NOTE_BYTES: u64 = 512 * 1024;
const MAX_FILES_CHECKED: usize = 20_000;
/// File names too common to mean one specific file when a note mentions them without a directory.
const GENERIC_NAMES: &[&str] = &[
    "mod.rs",
    "lib.rs",
    "main.rs",
    "build.rs",
    "__init__.py",
    "index.ts",
    "index.js",
    "index.tsx",
    "utils.ts",
    "utils.rs",
    "types.ts",
    "readme.md",
];
const CODE_EXTENSIONS: &[&str] = &[
    "rs", "go", "ts", "tsx", "js", "jsx", "mjs", "py", "c", "h", "cc", "cpp", "hpp", "java", "rb",
    "swift", "cs", "php", "kt", "sql", "html", "css", "vue",
];

// ---------------------------------------------------------------------------
// Index freshness
// ---------------------------------------------------------------------------

/// Compact state of the code index for the `code_index` field of a code search.
/// `None` means the question does not apply (the search did not touch the code corpus).
pub async fn index_info(
    workspace: &rms_memory_core::workspace::Workspace,
    store: &crate::store::Store,
) -> Value {
    use rms_memory_core::workspace::CodeIndexMode;
    let mode = match workspace.code_index_mode {
        CodeIndexMode::Off => "off",
        CodeIndexMode::Manual => "manual",
        CodeIndexMode::Watch => "watch",
    };
    let initialized = crate::code_indexer::code_index_is_initialized(&store.storage_path);
    if !initialized {
        let hint = if workspace.code_index_mode == CodeIndexMode::Off {
            "code index is off, so code search returns nothing; ask the user to run `rms-memory config --code-index-mode manual` and `rms-memory reindex --code` (use `watch` to keep it fresh). Until then use grep/read for code."
        } else {
            "code index has not been built yet; ask the user to run `rms-memory reindex --code`. Until then use grep/read for code."
        };
        return json!({ "status": "not_indexed", "mode": mode, "hint": hint });
    }
    let completed = crate::code_indexer::code_index_completed_at(&store.storage_path);
    let files = store.indexed_code_file_paths().await.unwrap_or_default();
    let (changed, removed) = count_changes(&workspace.code_path, &files, completed);
    if changed == 0 && removed == 0 {
        return json!({ "status": "fresh", "mode": mode, "files": files.len() });
    }
    let hint = if workspace.code_index_mode == CodeIndexMode::Watch {
        "the watcher is catching up; results for the listed files may lag, re-read the file if exactness matters"
    } else {
        "index is behind the working tree: re-read changed files, or ask the user to run `rms-memory reindex --code`"
    };
    json!({
        "status": "stale",
        "mode": mode,
        "files": files.len(),
        "changed_since_index": changed,
        "removed_since_index": removed,
        "hint": hint,
    })
}

/// Files modified after the last completed index, and indexed files that no longer exist.
fn count_changes(
    code_path: &Path,
    files: &[String],
    completed: Option<SystemTime>,
) -> (usize, usize) {
    let (mut changed, mut removed) = (0, 0);
    for rel in files.iter().take(MAX_FILES_CHECKED) {
        match std::fs::metadata(code_path.join(rel)) {
            Ok(meta) => {
                if let (Ok(modified), Some(done)) = (meta.modified(), completed)
                    && modified > done
                {
                    changed += 1;
                }
            }
            Err(_) => removed += 1,
        }
    }
    (changed, removed)
}

// ---------------------------------------------------------------------------
// Notes that talk about code
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct NoteEntry {
    path: String,
    title: String,
    pattern_status: Option<String>,
    modified: SystemTime,
    /// Lower-cased path-like tokens (`src/updater.rs`, `updater.rs`).
    paths: Vec<String>,
    /// Backticked identifiers and frontmatter `refs[].symbol` values.
    symbols: Vec<String>,
}

#[derive(Debug, Default)]
pub struct NoteIndex {
    notes: Vec<NoteEntry>,
}

/// How a note relates to a code hit; higher is a stronger match.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Via {
    Name,
    Symbol,
    Path,
    PathAndSymbol,
}

impl Via {
    fn as_str(self) -> &'static str {
        match self {
            Via::Name => "name",
            Via::Symbol => "symbol",
            Via::Path => "path",
            Via::PathAndSymbol => "path+symbol",
        }
    }
}

type Stamp = (usize, u128, u64);

type Cache = Mutex<HashMap<PathBuf, (Stamp, Arc<NoteIndex>)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn skip_dir(name: &str) -> bool {
    matches!(
        name,
        ".git" | ".obsidian" | "wiki" | "node_modules" | "pruned"
    )
}

fn walk_markdown(root: &Path) -> Vec<(PathBuf, std::fs::Metadata)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            let name = entry.file_name().to_string_lossy().to_string();
            if meta.is_dir() {
                if !skip_dir(&name) {
                    stack.push(path);
                }
            } else if meta.is_file()
                && name.ends_with(".md")
                && meta.len() <= MAX_NOTE_BYTES
                && out.len() < MAX_NOTES
            {
                out.push((path, meta));
            }
        }
    }
    out
}

fn stamp_of(files: &[(PathBuf, std::fs::Metadata)]) -> Stamp {
    let mut newest = 0u128;
    let mut bytes = 0u64;
    for (_, meta) in files {
        bytes = bytes.wrapping_add(meta.len());
        if let Ok(m) = meta.modified()
            && let Ok(d) = m.duration_since(SystemTime::UNIX_EPOCH)
        {
            newest = newest.max(d.as_nanos());
        }
    }
    (files.len(), newest, bytes)
}

impl NoteIndex {
    /// Cached index of the vault at `root`; rebuilt when a note is added, removed or edited.
    pub fn for_vault(root: &Path) -> Arc<NoteIndex> {
        let files = walk_markdown(root);
        let stamp = stamp_of(&files);
        if let Ok(guard) = cache().lock()
            && let Some((cached, index)) = guard.get(root)
            && *cached == stamp
        {
            return index.clone();
        }
        let index = Arc::new(Self::build(root, &files));
        if let Ok(mut guard) = cache().lock() {
            guard.insert(root.to_path_buf(), (stamp, index.clone()));
        }
        index
    }

    fn build(root: &Path, files: &[(PathBuf, std::fs::Metadata)]) -> NoteIndex {
        let mut notes = Vec::new();
        for (path, meta) in files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let rel = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/");
            if let Some(entry) = parse_note(
                &rel,
                &text,
                meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
            ) {
                notes.push(entry);
            }
        }
        NoteIndex { notes }
    }

    /// Notes that mention `file_path` (and optionally `symbol`), strongest first.
    pub fn related(&self, file_path: &str, symbols: &[&str], limit: usize) -> Vec<Related> {
        let file = file_path.replace('\\', "/").to_lowercase();
        let base = file.rsplit('/').next().unwrap_or(&file).to_string();
        let base_is_generic = GENERIC_NAMES.contains(&base.as_str());
        let syms: Vec<(String, String)> = symbols
            .iter()
            .map(|s| {
                let last = s.rsplit([':', '.']).next().unwrap_or(s);
                (s.to_string(), last.to_string())
            })
            .collect();
        let mut hits: Vec<(Via, &NoteEntry)> = Vec::new();
        for note in &self.notes {
            let mut path_hit = false;
            let mut name_hit = false;
            for mention in &note.paths {
                if mention.contains('/') {
                    if file == *mention || file.ends_with(&format!("/{mention}")) {
                        path_hit = true;
                        break;
                    }
                } else if *mention == base && !base_is_generic {
                    name_hit = true;
                }
            }
            let sym_hit = syms.iter().any(|(full, last)| {
                note.symbols
                    .iter()
                    .any(|s| s == full || (last.chars().count() >= 6 && s == last))
            });
            let qualified_hit = syms.iter().any(|(full, _)| {
                (full.contains("::") || full.contains('.'))
                    && note.symbols.iter().any(|s| s == full)
            });
            let via = match (path_hit, name_hit, sym_hit) {
                (true, _, true) => Via::PathAndSymbol,
                (true, _, false) => Via::Path,
                (false, true, true) => Via::PathAndSymbol,
                (false, true, false) => Via::Name,
                // A symbol match alone is only trusted when the symbol is qualified.
                (false, false, true) if qualified_hit => Via::Symbol,
                _ => continue,
            };
            hits.push((via, note));
        }
        hits.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| {
                    status_rank(&b.1.pattern_status).cmp(&status_rank(&a.1.pattern_status))
                })
                .then_with(|| b.1.modified.cmp(&a.1.modified))
        });
        hits.into_iter()
            .take(limit)
            .map(|(via, note)| Related {
                path: note.path.clone(),
                title: note.title.clone(),
                via,
                pattern_status: note.pattern_status.clone(),
                modified: note.modified,
            })
            .collect()
    }
}

fn status_rank(status: &Option<String>) -> u8 {
    match status.as_deref() {
        Some("verified-in-runtime") => 3,
        Some("implemented") => 2,
        Some("proposed") => 1,
        _ => 2,
    }
}

#[derive(Debug, Clone)]
pub struct Related {
    pub path: String,
    pub title: String,
    via: Via,
    pub pattern_status: Option<String>,
    modified: SystemTime,
}

impl Related {
    /// Compact JSON for an agent. `code_changed_since_note` is set when the code file was
    /// modified after the note was last edited, so the note may describe older code.
    pub fn to_json(&self, code_file: Option<&Path>) -> Value {
        let mut v = json!({
            "path": self.path,
            "title": self.title,
            "via": self.via.as_str(),
        });
        if let Some(status) = &self.pattern_status {
            v["pattern_status"] = json!(status);
        }
        if let Some(file) = code_file
            && let Ok(meta) = std::fs::metadata(file)
            && let Ok(modified) = meta.modified()
            && modified > self.modified
        {
            v["code_changed_since_note"] = json!(true);
        }
        v
    }
}

fn parse_note(rel: &str, text: &str, modified: SystemTime) -> Option<NoteEntry> {
    let mut paths = Vec::new();
    let mut symbols = Vec::new();
    let mut pattern_status = None;
    let mut body = text;
    if let Some(split) = crate::frontmatter::split(text) {
        body = split.body;
        if let Ok(map) = crate::frontmatter::parse_mapping(split.yaml) {
            let status = map.get("status").and_then(|v| v.as_str()).unwrap_or("");
            if status.eq_ignore_ascii_case("superseded") {
                return None;
            }
            if rel.starts_with("patterns/") {
                let meta = crate::patterns::parse_meta(text);
                pattern_status = Some(meta.status.as_str().to_string());
                for r in meta.refs {
                    paths.push(r.path.to_lowercase());
                    if let Some(s) = r.symbol {
                        symbols.push(s);
                    }
                }
            }
        }
    }
    extract_mentions(body, &mut paths, &mut symbols);
    if paths.is_empty() && symbols.is_empty() {
        return None;
    }
    paths.sort();
    paths.dedup();
    symbols.sort();
    symbols.dedup();
    let title = body
        .lines()
        .find_map(|l| l.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| {
            rel.rsplit('/')
                .next()
                .unwrap_or(rel)
                .trim_end_matches(".md")
                .to_string()
        });
    Some(NoteEntry {
        path: rel.to_string(),
        title,
        pattern_status,
        modified,
        paths,
        symbols,
    })
}

fn is_path_token(token: &str) -> bool {
    let Some((stem, ext)) = token.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && stem.len() + ext.len() >= 4
        && CODE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str())
        && !token.starts_with("http")
}

fn is_symbol_token(token: &str) -> bool {
    let mut chars = token.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && token.chars().count() >= 5
        && token
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.'))
}

/// Path-like tokens anywhere in the text; identifiers only inside backticks (prose words
/// would match far too much).
fn extract_mentions(body: &str, paths: &mut Vec<String>, symbols: &mut Vec<String>) {
    for raw in
        body.split(|c: char| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '/' | '-')))
    {
        let token = raw
            .trim_matches(|c| c == '.' || c == '/')
            .trim_start_matches("./");
        if is_path_token(token) {
            paths.push(token.to_lowercase());
        }
    }
    let mut rest = body;
    while let Some(start) = rest.find('`') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('`') else { break };
        let span = rest[..end].trim().trim_end_matches("()");
        if is_symbol_token(span) && !is_path_token(span) {
            symbols.push(span.to_string());
        }
        rest = &rest[end + 1..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(notes: &[(&str, &str)]) -> NoteIndex {
        let dir = tempfile::tempdir().unwrap();
        for (rel, text) in notes {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        let files = walk_markdown(dir.path());
        NoteIndex::build(dir.path(), &files)
    }

    #[test]
    fn path_mention_links_note_to_code_hit() {
        let idx = index(&[(
            "decisions/updater.md",
            "# Updater design\nThe swap lives in `src/updater.rs` and is atomic.\n",
        )]);
        let hits = idx.related("src/updater.rs", &["updater::install_staged"], 3);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].path, "decisions/updater.md");
        assert_eq!(hits[0].title, "Updater design");
        assert_eq!(hits[0].via.as_str(), "path");
        assert!(idx.related("src/other.rs", &[], 3).is_empty());
    }

    #[test]
    fn suffix_of_a_longer_path_matches_but_a_different_dir_does_not() {
        let idx = index(&[("a.md", "see crates/x/src/updater.rs\n")]);
        assert_eq!(idx.related("crates/x/src/updater.rs", &[], 3).len(), 1);
        assert!(idx.related("crates/y/src/updater.rs", &[], 3).is_empty());
    }

    #[test]
    fn bare_generic_file_names_are_not_matched() {
        let idx = index(&[("a.md", "edit mod.rs and updater.rs\n")]);
        assert!(idx.related("src/tools/mod.rs", &[], 3).is_empty());
        let hits = idx.related("src/updater.rs", &[], 3);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].via.as_str(), "name");
    }

    #[test]
    fn qualified_symbol_alone_matches_only_when_qualified() {
        let idx = index(&[("a.md", "call `Store::search_code` first\n")]);
        let hits = idx.related("elsewhere.rs", &["Store::search_code"], 3);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].via.as_str(), "symbol");
        // A bare identifier shared by unrelated code must not link by itself.
        let idx = index(&[("b.md", "the `process` step\n")]);
        assert!(idx.related("x.rs", &["process"], 3).is_empty());
    }

    #[test]
    fn superseded_notes_are_skipped_and_patterns_rank_by_status() {
        let idx = index(&[
            (
                "old.md",
                "---\nstatus: superseded\n---\n# Old\nsrc/updater.rs\n",
            ),
            (
                "patterns/p.md",
                "---\ntype: pattern\npattern_status: verified-in-runtime\nrefs:\n  - path: src/updater.rs\n    symbol: install_staged\n---\n# P\n",
            ),
            ("new.md", "# New\nsrc/updater.rs\n"),
        ]);
        let hits = idx.related("src/updater.rs", &["install_staged"], 5);
        let paths: Vec<_> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(paths, vec!["patterns/p.md", "new.md"]);
        assert_eq!(
            hits[0].pattern_status.as_deref(),
            Some("verified-in-runtime")
        );
    }

    #[test]
    fn json_flags_code_newer_than_note() {
        let dir = tempfile::tempdir().unwrap();
        let note = Related {
            path: "n.md".into(),
            title: "N".into(),
            via: Via::Path,
            pattern_status: None,
            modified: SystemTime::UNIX_EPOCH,
        };
        let file = dir.path().join("a.rs");
        std::fs::write(&file, "x").unwrap();
        assert_eq!(note.to_json(Some(&file))["code_changed_since_note"], true);
        assert!(note.to_json(None).get("code_changed_since_note").is_none());
    }

    #[test]
    fn count_changes_sees_edits_and_removals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "x").unwrap();
        let done = SystemTime::now() - std::time::Duration::from_secs(60);
        let files = vec!["a.rs".to_string(), "gone.rs".to_string()];
        assert_eq!(count_changes(dir.path(), &files, Some(done)), (1, 1));
        let later = SystemTime::now() + std::time::Duration::from_secs(60);
        assert_eq!(count_changes(dir.path(), &files, Some(later)), (0, 1));
    }
}
