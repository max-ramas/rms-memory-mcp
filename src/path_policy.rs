//! Canonical path policy shared by Vault indexing, graph extraction, watchers,
//! code indexing, and Wiki source collection.
//!
//! The generated Wiki is a projection of canonical memory.  It lives on disk
//! under `<vault>/wiki/`, but must never feed back into the canonical corpora.

use std::path::{Component, Path};

pub const WIKI_DIR: &str = "wiki";

/// Return a stable, slash-separated path relative to `root`.
///
/// Relative candidates are accepted as already relative to `root`. Parent
/// traversal that would escape the root is rejected.
pub fn normalized_relative_path(root: &Path, candidate: &Path) -> Option<String> {
    let relative = if candidate.is_absolute() {
        candidate.strip_prefix(root).ok()?
    } else {
        candidate
    };

    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => parts.push(value.to_string_lossy().into_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// Whether a path belongs to the generated Wiki namespace.
///
/// This excludes the complete namespace, including `.generation`, archive,
/// sentinel, and future Wiki-owned artifacts without maintaining parallel
/// special-case lists.
pub fn is_vault_wiki_path(vault_root: &Path, candidate: &Path) -> bool {
    normalized_relative_path(vault_root, candidate)
        .and_then(|path| path.split('/').next().map(str::to_owned))
        .is_some_and(|first| first.eq_ignore_ascii_case(WIKI_DIR))
}

/// Relative-path variant for persisted Vault records.
pub fn is_vault_wiki_relative_path(candidate: &str) -> bool {
    let normalized = candidate.replace('\\', "/");
    let normalized = normalized.trim_start_matches("./");
    normalized
        .split('/')
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(WIKI_DIR))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_complete_wiki_namespace() {
        let root = Path::new("/vault");
        assert!(is_vault_wiki_path(root, Path::new("/vault/wiki/page.md")));
        assert!(is_vault_wiki_path(
            root,
            Path::new("/vault/wiki/.generation/context-pack.md")
        ));
        assert!(is_vault_wiki_path(root, Path::new("wiki/_archive/old.md")));
        assert!(is_vault_wiki_relative_path("wiki\\page.md"));
        assert!(is_vault_wiki_relative_path("./Wiki/page.md"));
    }

    #[test]
    fn does_not_exclude_similarly_named_canonical_memory() {
        let root = Path::new("/vault");
        assert!(!is_vault_wiki_path(root, Path::new("/vault/docs/wiki.md")));
        assert!(!is_vault_wiki_path(root, Path::new("/other/wiki/page.md")));
        assert!(!is_vault_wiki_relative_path("wiki-notes/page.md"));
    }

    #[test]
    fn rejects_parent_escape() {
        assert_eq!(
            normalized_relative_path(Path::new("/vault"), Path::new("../wiki/page.md")),
            None
        );
    }
}
