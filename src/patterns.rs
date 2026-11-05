//! Pattern notes (`patterns/**/*.md`): status filter for search and a staleness check for doctor.
//!
//! A pattern is a normal note with frontmatter `type: pattern`, `pattern_status`
//! (`proposed` < `implemented` < `verified-in-runtime`) and `refs` pointing at code
//! (`repo`, `path`, `symbol`, `commit`). It never holds a copy of the code. See `docs/patterns.md`.

use anyhow::{Result, bail};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub const PATTERNS_DIR: &str = "patterns/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum PatternStatus {
    Proposed,
    Implemented,
    VerifiedInRuntime,
}

impl PatternStatus {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "proposed" => Some(Self::Proposed),
            "implemented" => Some(Self::Implemented),
            "verified-in-runtime" | "verified" => Some(Self::VerifiedInRuntime),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "proposed",
            Self::Implemented => "implemented",
            Self::VerifiedInRuntime => "verified-in-runtime",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatternRef {
    pub repo: Option<String>,
    pub path: String,
    pub symbol: Option<String>,
    pub commit: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PatternMeta {
    pub status: PatternStatus,
    pub refs: Vec<PatternRef>,
    pub has_verification: bool,
}

fn yaml_str(map: &serde_yaml::Mapping, key: &str) -> Option<String> {
    map.get(key).and_then(|v| match v {
        serde_yaml::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_yaml::Value::Number(n) => Some(n.to_string()),
        _ => None,
    })
}

/// Metadata of a pattern note. A missing or unknown `pattern_status` counts as `proposed`,
/// so a half-written note is never served as a recommendation.
pub fn parse_meta(note_text: &str) -> PatternMeta {
    let mapping = crate::frontmatter::split(note_text)
        .and_then(|s| crate::frontmatter::parse_mapping(s.yaml).ok())
        .unwrap_or_default();
    let status = yaml_str(&mapping, "pattern_status")
        .and_then(|s| PatternStatus::parse(&s))
        .unwrap_or(PatternStatus::Proposed);
    let refs = match mapping.get("refs") {
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .filter_map(|item| {
                let m = item.as_mapping()?;
                Some(PatternRef {
                    repo: yaml_str(m, "repo"),
                    path: yaml_str(m, "path")?,
                    symbol: yaml_str(m, "symbol"),
                    commit: yaml_str(m, "commit"),
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    let has_verification = mapping
        .get("verified")
        .and_then(|v| v.as_mapping())
        .is_some_and(|m| yaml_str(m, "date").is_some() && yaml_str(m, "how").is_some());
    PatternMeta {
        status,
        refs,
        has_verification,
    }
}

pub fn is_pattern_path(path: &str) -> bool {
    path.starts_with(PATTERNS_DIR) && path.ends_with(".md")
}

/// Which pattern statuses a search may return (a minimum level).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatternFilter {
    pub min: PatternStatus,
}

impl Default for PatternFilter {
    /// Only patterns proven in runtime are recommended unless the caller asks for more.
    fn default() -> Self {
        Self {
            min: PatternStatus::VerifiedInRuntime,
        }
    }
}

impl PatternFilter {
    /// `pattern_status` argument: `verified-in-runtime` (default), `implemented`
    /// (implemented and verified), `proposed` or `all` (everything).
    pub fn from_arg(raw: Option<&str>) -> Result<Self> {
        let Some(raw) = raw else {
            return Ok(Self::default());
        };
        if raw.trim().eq_ignore_ascii_case("all") {
            return Ok(Self {
                min: PatternStatus::Proposed,
            });
        }
        match PatternStatus::parse(raw) {
            Some(min) => Ok(Self { min }),
            None => bail!(
                "pattern_status must be one of: verified-in-runtime (default), implemented, proposed, all (got {raw})"
            ),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn safe_join(root: &Path, relative: &str) -> Option<PathBuf> {
    let rel = Path::new(relative);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) {
        return None;
    }
    Some(root.join(rel))
}

/// Status of the pattern note at `relative` inside `vault_root` (None when unreadable).
pub fn status_of(vault_root: &Path, relative: &str) -> Option<PatternStatus> {
    let text = std::fs::read_to_string(safe_join(vault_root, relative)?).ok()?;
    Some(parse_meta(&text).status)
}

/// Keep a hit unless it is a pattern note below the requested level. Returns the status
/// to show for pattern hits. `cache` avoids reading one note once per chunk.
pub fn admit(
    filter: PatternFilter,
    vault_root: &Path,
    path: &str,
    cache: &mut HashMap<String, Option<PatternStatus>>,
) -> (bool, Option<PatternStatus>) {
    if !is_pattern_path(path) {
        return (true, None);
    }
    let status = *cache
        .entry(path.to_string())
        .or_insert_with(|| status_of(vault_root, path));
    match status {
        Some(s) => (s >= filter.min, Some(s)),
        // Unreadable note: it cannot be proven, so only the "all" level shows it.
        None => (filter.min == PatternStatus::Proposed, None),
    }
}

// ---------------------------------------------------------------------------
// Staleness check
// ---------------------------------------------------------------------------

/// Resolves `refs[].repo` to a checkout. `current` is the project's own code path, used when
/// `repo` is absent or names this project; `others` maps `owner/name` to a local checkout.
pub struct RepoResolver {
    pub current: PathBuf,
    pub current_repo: Option<String>,
    pub others: HashMap<String, PathBuf>,
}

impl RepoResolver {
    fn resolve(&self, repo: Option<&str>) -> Option<&Path> {
        match repo {
            None => Some(&self.current),
            Some(r) if self.current_repo.as_deref() == Some(r) => Some(&self.current),
            Some(r) => self.others.get(r).map(PathBuf::as_path),
        }
    }
}

/// `owner/name` from a git remote URL (https or ssh form).
pub fn repo_from_remote(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let tail = trimmed.rsplit([':', '/']).take(2).collect::<Vec<_>>();
    if tail.len() == 2 && !tail[0].is_empty() && !tail[1].is_empty() {
        Some(format!("{}/{}", tail[1], tail[0]))
    } else {
        None
    }
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn remote_repo_of(root: &Path) -> Option<String> {
    repo_from_remote(&git(root, &["remote", "get-url", "origin"])?)
}

/// The last `::`/`.` segment of a symbol, which is what gets matched in text.
fn symbol_name(symbol: &str) -> &str {
    symbol
        .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
        .find(|part| !part.is_empty())
        .unwrap_or(symbol)
}

/// Where else in the repository `symbol` appears (up to 3 files), to tell a rename or move
/// from a deletion. Uses `git grep`, so it needs no code index.
fn symbol_elsewhere(root: &Path, symbol: &str, not_in: &str) -> Vec<String> {
    let name = symbol_name(symbol);
    if name.chars().count() < 4 {
        return Vec::new();
    }
    git(root, &["grep", "-l", "-w", "-F", "-e", name])
        .map(|out| {
            out.lines()
                .filter(|l| *l != not_in)
                .take(3)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Whole-word occurrence of `symbol` (the last `::`/`.` segment) in `text`.
fn contains_symbol(text: &str, symbol: &str) -> bool {
    let name = symbol_name(symbol);
    if name.is_empty() {
        return false;
    }
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + name.len()..].chars().next();
        !before.is_some_and(is_ident) && !after.is_some_and(is_ident)
    })
}

/// Problems found for one pattern note. Empty = current.
pub fn check_note(note_path: &str, text: &str, resolver: &RepoResolver) -> Vec<String> {
    let meta = parse_meta(text);
    let mut issues = Vec::new();
    if meta.status >= PatternStatus::Implemented && meta.refs.is_empty() {
        issues.push(format!(
            "{note_path}: {} pattern has no `refs` to check against",
            meta.status.as_str()
        ));
    }
    if meta.status == PatternStatus::VerifiedInRuntime && !meta.has_verification {
        issues.push(format!(
            "{note_path}: verified-in-runtime without `verified.date` and `verified.how`"
        ));
    }
    for r in &meta.refs {
        let Some(root) = resolver.resolve(r.repo.as_deref()) else {
            // Not resolvable here (no local checkout): not an error, nothing to compare.
            continue;
        };
        let label = format!("{note_path}: ref {}", r.path);
        let Some(file) = safe_join(root, &r.path) else {
            issues.push(format!("{label}: unsafe path"));
            continue;
        };
        if !file.is_file() {
            issues.push(format!("{label}: file no longer exists"));
            continue;
        }
        if let Some(symbol) = &r.symbol {
            let found = std::fs::read_to_string(&file)
                .map(|t| contains_symbol(&t, symbol))
                .unwrap_or(true); // binary/unreadable: cannot judge
            if !found {
                let elsewhere = symbol_elsewhere(root, symbol, &r.path);
                if elsewhere.is_empty() {
                    issues.push(format!("{label}: symbol `{symbol}` not found"));
                } else {
                    issues.push(format!(
                        "{label}: symbol `{symbol}` not found in this file; it now appears in {} (moved or renamed? update the ref)",
                        elsewhere.join(", ")
                    ));
                }
            }
        }
        if let Some(commit) = &r.commit {
            let spec = format!("{commit}..HEAD");
            match git(root, &["rev-list", "--count", &spec, "--", &r.path]) {
                Some(n) if n != "0" => issues.push(format!(
                    "{label}: changed in {n} commit(s) since {commit}; re-check the pattern"
                )),
                Some(_) => {}
                None => issues.push(format!("{label}: commit {commit} is not in the repository")),
            }
        }
    }
    issues
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTE: &str = "---\ntype: pattern\npattern_status: verified-in-runtime\nrefs:\n  - path: src/lib.rs\n    symbol: do_work\n    commit: HEAD\nverified:\n  date: 2026-11-05\n  how: ran it\n---\n# P\n";

    #[test]
    fn parses_status_refs_and_verification() {
        let m = parse_meta(NOTE);
        assert_eq!(m.status, PatternStatus::VerifiedInRuntime);
        assert_eq!(m.refs.len(), 1);
        assert_eq!(m.refs[0].symbol.as_deref(), Some("do_work"));
        assert!(m.has_verification);
        // Missing or unknown status is never served as verified.
        assert_eq!(
            parse_meta("# no frontmatter").status,
            PatternStatus::Proposed
        );
        assert_eq!(
            parse_meta("---\npattern_status: great\n---\n").status,
            PatternStatus::Proposed
        );
    }

    #[test]
    fn filter_argument_levels() {
        assert_eq!(
            PatternFilter::from_arg(None).unwrap().min,
            PatternStatus::VerifiedInRuntime
        );
        assert!(PatternFilter::from_arg(None).unwrap().is_default());
        assert_eq!(
            PatternFilter::from_arg(Some("all")).unwrap().min,
            PatternStatus::Proposed
        );
        assert_eq!(
            PatternFilter::from_arg(Some("implemented")).unwrap().min,
            PatternStatus::Implemented
        );
        assert!(PatternFilter::from_arg(Some("nope")).is_err());
    }

    fn vault_with(notes: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (path, text) in notes {
            let p = dir.path().join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        }
        dir
    }

    #[test]
    fn admit_hides_unproven_patterns_by_default_only() {
        let vault = vault_with(&[
            ("patterns/a.md", NOTE),
            ("patterns/b.md", "---\npattern_status: implemented\n---\n"),
            ("patterns/c.md", "---\npattern_status: proposed\n---\n"),
            ("rules/x.md", "---\npattern_status: proposed\n---\n"),
        ]);
        let mut cache = HashMap::new();
        let def = PatternFilter::default();
        assert!(admit(def, vault.path(), "patterns/a.md", &mut cache).0);
        assert!(!admit(def, vault.path(), "patterns/b.md", &mut cache).0);
        assert!(!admit(def, vault.path(), "patterns/c.md", &mut cache).0);
        assert!(!admit(def, vault.path(), "patterns/missing.md", &mut cache).0);
        // Non-pattern notes are never filtered, whatever their frontmatter says.
        assert!(admit(def, vault.path(), "rules/x.md", &mut cache).0);
        let impl_level = PatternFilter::from_arg(Some("implemented")).unwrap();
        assert!(admit(impl_level, vault.path(), "patterns/b.md", &mut cache).0);
        assert!(!admit(impl_level, vault.path(), "patterns/c.md", &mut cache).0);
        let all = PatternFilter::from_arg(Some("all")).unwrap();
        let (ok, status) = admit(all, vault.path(), "patterns/c.md", &mut cache);
        assert!(ok);
        assert_eq!(status, Some(PatternStatus::Proposed));
        assert!(admit(all, vault.path(), "patterns/missing.md", &mut cache).0);
    }

    #[test]
    fn admit_rejects_path_escape() {
        let vault = vault_with(&[]);
        let mut cache = HashMap::new();
        assert!(
            !admit(
                PatternFilter::default(),
                vault.path(),
                "patterns/../../etc.md",
                &mut cache
            )
            .0
        );
    }

    #[test]
    fn remote_urls_map_to_owner_name() {
        assert_eq!(
            repo_from_remote("https://github.com/max-ramas/rms-memory-mcp.git").as_deref(),
            Some("max-ramas/rms-memory-mcp")
        );
        assert_eq!(
            repo_from_remote("git@github.com:max-ramas/rms-memory-gui.git").as_deref(),
            Some("max-ramas/rms-memory-gui")
        );
        assert_eq!(repo_from_remote("").as_deref(), None);
    }

    #[test]
    fn symbol_match_is_whole_word_on_last_segment() {
        assert!(contains_symbol("fn do_work() {}", "do_work"));
        assert!(contains_symbol(
            "impl X { fn install_staged() }",
            "updater::install_staged"
        ));
        assert!(!contains_symbol("fn do_work_more() {}", "do_work"));
        assert!(!contains_symbol("nothing here", "do_work"));
    }

    fn git_repo() -> (tempfile::TempDir, String) {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .output()
                .unwrap()
                .status
                .success();
            assert!(ok, "git {args:?}");
        };
        run(&["init", "-q"]);
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join("src/lib.rs"), "fn do_work() {}\n").unwrap();
        run(&["add", "."]);
        run(&["commit", "-q", "-m", "one"]);
        let head = git(dir.path(), &["rev-parse", "HEAD"]).unwrap();
        (dir, head)
    }

    fn resolver(dir: &Path) -> RepoResolver {
        RepoResolver {
            current: dir.to_path_buf(),
            current_repo: None,
            others: HashMap::new(),
        }
    }

    fn note(path: &str, symbol: &str, commit: &str) -> String {
        format!(
            "---\ntype: pattern\npattern_status: verified-in-runtime\nrefs:\n  - path: {path}\n    symbol: {symbol}\n    commit: {commit}\nverified:\n  date: 2026-11-05\n  how: ran it\n---\n"
        )
    }

    #[test]
    fn moved_symbol_is_reported_with_its_new_location() {
        let (dir, head) = git_repo();
        std::fs::write(
            dir.path().join("src/other.rs"),
            "pub fn moved_helper() {}\n",
        )
        .unwrap();
        let add = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "."])
            .output()
            .unwrap();
        assert!(add.status.success());
        let issues = check_note(
            "patterns/p.md",
            &note("src/lib.rs", "moved_helper", &head),
            &resolver(dir.path()),
        );
        assert!(
            issues.iter().any(|i| i.contains("moved_helper")
                && i.contains("src/other.rs")
                && i.contains("moved or renamed")),
            "{issues:?}"
        );
    }

    #[test]
    fn staleness_detects_missing_file_symbol_and_later_changes() {
        let (dir, head) = git_repo();
        let r = resolver(dir.path());
        // Current: nothing to report.
        assert!(check_note("patterns/p.md", &note("src/lib.rs", "do_work", &head), &r).is_empty());
        // Symbol renamed away.
        let issues = check_note("patterns/p.md", &note("src/lib.rs", "gone", &head), &r);
        assert!(
            issues.iter().any(|i| i.contains("symbol `gone` not found")),
            "{issues:?}"
        );
        // File missing.
        let issues = check_note("patterns/p.md", &note("src/nope.rs", "x", &head), &r);
        assert!(
            issues.iter().any(|i| i.contains("no longer exists")),
            "{issues:?}"
        );
        // Unknown commit.
        let issues = check_note(
            "patterns/p.md",
            &note("src/lib.rs", "do_work", "deadbeefdeadbeef"),
            &r,
        );
        assert!(
            issues.iter().any(|i| i.contains("not in the repository")),
            "{issues:?}"
        );
        // File changed after the recorded commit.
        std::fs::write(dir.path().join("src/lib.rs"), "fn do_work() { }\n").unwrap();
        let ok = std::process::Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-am",
                "two",
            ])
            .output()
            .unwrap()
            .status
            .success();
        assert!(ok);
        let issues = check_note("patterns/p.md", &note("src/lib.rs", "do_work", &head), &r);
        assert!(
            issues.iter().any(|i| i.contains("changed in 1 commit")),
            "{issues:?}"
        );
    }

    #[test]
    fn staleness_flags_missing_evidence_and_refs_but_skips_unknown_repos() {
        let (dir, _) = git_repo();
        let r = resolver(dir.path());
        let bare = "---\ntype: pattern\npattern_status: verified-in-runtime\n---\n";
        let issues = check_note("patterns/p.md", bare, &r);
        assert!(issues.iter().any(|i| i.contains("no `refs`")));
        assert!(issues.iter().any(|i| i.contains("verified.date")));
        let proposed = "---\ntype: pattern\npattern_status: proposed\n---\n";
        assert!(check_note("patterns/p.md", proposed, &r).is_empty());
        let foreign =
            "---\npattern_status: implemented\nrefs:\n  - repo: other/repo\n    path: a.rs\n---\n";
        assert!(check_note("patterns/p.md", foreign, &r).is_empty());
    }
}
