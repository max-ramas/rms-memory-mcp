//! Lightweight git state of a project's code repository: branch, HEAD,
//! ahead/behind and the names of changed files. Diff contents are never read.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

/// Changed paths reported at most; the rest is only counted.
pub const MAX_CHANGED_FILES: usize = 50;

#[derive(Serialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct GitContext {
    /// Current branch; `None` on a detached HEAD.
    pub branch: Option<String>,
    /// Abbreviated HEAD commit; `None` before the first commit.
    pub head: Option<String>,
    /// Subject line of the HEAD commit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub head_subject: Option<String>,
    /// Commits ahead of / behind the upstream branch, when one is configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ahead: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub behind: Option<usize>,
    pub changed: Vec<GitChange>,
    /// Number of changed paths, including those beyond `changed`.
    pub changed_total: usize,
}

#[derive(Serialize, Debug, Clone, PartialEq, Eq)]
pub struct GitChange {
    pub path: String,
    /// `modified`, `added`, `deleted`, `renamed`, `copied`, `type-changed`,
    /// `untracked` or `conflict`.
    pub status: &'static str,
    /// The change is in the index (staged for commit).
    pub staged: bool,
}

/// Git state of the work tree containing `repo`, or `None` when it is not a
/// git work tree or git is unavailable.
pub fn git_context(repo: &Path) -> Option<GitContext> {
    if !repo.is_dir() {
        return None;
    }
    let status = git(
        repo,
        &[
            "status",
            "--porcelain=v2",
            "--branch",
            "-z",
            "--untracked-files=normal",
        ],
    )?;
    let mut context = parse_status(&status);
    if context.head.is_some() {
        context.head_subject = git(repo, &["log", "-1", "--format=%s"])
            .map(|subject| subject.trim().to_string())
            .filter(|subject| !subject.is_empty());
    }
    Some(context)
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        // `git status` must not refresh or lock the repository index.
        .env("GIT_OPTIONAL_LOCKS", "0")
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Parse `git status --porcelain=v2 --branch -z`.
pub fn parse_status(output: &str) -> GitContext {
    let mut context = GitContext::default();
    let mut records = output.split('\0');
    while let Some(record) = records.next() {
        if let Some(header) = record.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" if value != "(initial)" => {
                    context.head = Some(value.chars().take(12).collect());
                }
                "branch.head" if value != "(detached)" => {
                    context.branch = Some(value.to_string());
                }
                "branch.ab" => {
                    let mut counts = value
                        .split(' ')
                        .map(|count| count.trim_start_matches(['+', '-']).parse().ok());
                    context.ahead = counts.next().flatten();
                    context.behind = counts.next().flatten();
                }
                _ => {}
            }
            continue;
        }
        let change = match record.split_once(' ') {
            Some(("1", rest)) => tracked_change(rest, 7),
            Some(("2", rest)) => {
                // The original path of a rename/copy follows as its own record.
                records.next();
                tracked_change(rest, 8)
            }
            Some(("u", rest)) => rest.splitn(10, ' ').nth(9).map(|path| GitChange {
                path: path.to_string(),
                status: "conflict",
                staged: false,
            }),
            Some(("?", path)) => Some(GitChange {
                path: path.to_string(),
                status: "untracked",
                staged: false,
            }),
            _ => None,
        };
        if let Some(change) = change {
            context.changed_total += 1;
            if context.changed.len() < MAX_CHANGED_FILES {
                context.changed.push(change);
            }
        }
    }
    context
}

/// `XY <fields...> <path>` where the path is the `path_field`-th field.
fn tracked_change(rest: &str, path_field: usize) -> Option<GitChange> {
    let mut fields = rest.splitn(path_field + 1, ' ');
    let xy = fields.next()?;
    let path = fields.nth(path_field - 1)?;
    let mut codes = xy.chars();
    let (index, worktree) = (codes.next()?, codes.next()?);
    let staged = index != '.';
    Some(GitChange {
        path: path.to_string(),
        status: status_name(if staged { index } else { worktree }),
        staged,
    })
}

fn status_name(code: char) -> &'static str {
    match code {
        'A' => "added",
        'D' => "deleted",
        'R' => "renamed",
        'C' => "copied",
        'T' => "type-changed",
        _ => "modified",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_branch_headers_and_changes() {
        let output = [
            "# branch.oid 0123456789abcdef0123456789abcdef01234567",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
            "1 .M N... 100644 100644 100644 aaa bbb src/lib.rs",
            "1 A. N... 000000 100644 100644 000 ccc docs/new file.md",
            "2 R. N... 100644 100644 100644 ddd eee R100 src/new.rs",
            "src/old.rs",
            "u UU N... 100644 100644 100644 100644 a b c src/conflict.rs",
            "? notes.txt",
            "",
        ]
        .join("\0");
        let context = parse_status(&output);
        assert_eq!(context.branch.as_deref(), Some("main"));
        assert_eq!(context.head.as_deref(), Some("0123456789ab"));
        assert_eq!((context.ahead, context.behind), (Some(2), Some(1)));
        let summary: Vec<_> = context
            .changed
            .iter()
            .map(|change| (change.path.as_str(), change.status, change.staged))
            .collect();
        assert_eq!(
            summary,
            vec![
                ("src/lib.rs", "modified", false),
                ("docs/new file.md", "added", true),
                ("src/new.rs", "renamed", true),
                ("src/conflict.rs", "conflict", false),
                ("notes.txt", "untracked", false),
            ]
        );
        assert_eq!(context.changed_total, 5);
    }

    #[test]
    fn detached_and_initial_heads_are_absent() {
        let context = parse_status("# branch.oid (initial)\0# branch.head (detached)\0");
        assert_eq!(context, GitContext::default());
    }

    #[test]
    fn changed_list_is_capped_but_counted() {
        let output: String = (0..MAX_CHANGED_FILES + 5)
            .map(|i| format!("? file{i}.txt\0"))
            .collect();
        let context = parse_status(&output);
        assert_eq!(context.changed.len(), MAX_CHANGED_FILES);
        assert_eq!(context.changed_total, MAX_CHANGED_FILES + 5);
    }

    #[test]
    fn reads_a_real_repository_without_touching_its_index() {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let status = Command::new("git")
                .arg("-C")
                .arg(dir.path())
                .args(args)
                .env("GIT_AUTHOR_NAME", "t")
                .env("GIT_AUTHOR_EMAIL", "t@example.com")
                .env("GIT_COMMITTER_NAME", "t")
                .env("GIT_COMMITTER_EMAIL", "t@example.com")
                .status();
            status.map(|status| status.success()).unwrap_or(false)
        };
        if !run(&["init", "-q", "-b", "work"]) {
            return;
        }
        std::fs::write(dir.path().join("a.txt"), "one").unwrap();
        assert!(run(&["add", "a.txt"]));
        assert!(run(&[
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "--no-verify",
            "-m",
            "first commit"
        ]));
        std::fs::write(dir.path().join("a.txt"), "two").unwrap();
        std::fs::write(dir.path().join("b.txt"), "new").unwrap();
        let context = git_context(dir.path()).unwrap();
        assert_eq!(context.branch.as_deref(), Some("work"));
        assert_eq!(context.head_subject.as_deref(), Some("first commit"));
        assert_eq!(context.changed_total, 2);
        assert!(git_context(&dir.path().join("missing")).is_none());
    }
}
