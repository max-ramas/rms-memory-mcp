use crate::document::Document;
use std::path::{Path, PathBuf};

/// Resolves a linked document.
/// If the file at `file_path` contains a `link` in its frontmatter,
/// returns the resolved absolute path to the source file.
/// Otherwise, returns the original `file_path` unmodified.
pub fn resolve_link(file_path: &Path) -> PathBuf {
    if let Ok(doc) = Document::parse(file_path)
        && let Some(fm) = doc.frontmatter
        && let Some(link) = fm.link
        && let Some(parent) = file_path.parent()
    {
        let resolved = parent.join(&link);
        // Try to canonicalize if it exists, otherwise return joined path
        if let Ok(canon) = resolved.canonicalize() {
            return canon;
        }
        return resolved;
    }
    file_path.to_path_buf()
}

/// Checks if a file is a linked document and returns the source content if so.
/// Otherwise, returns `None`.
pub fn get_linked_content(file_path: &Path) -> Option<String> {
    if let Ok(doc) = Document::parse(file_path)
        && let Some(fm) = doc.frontmatter
        && let Some(link) = fm.link
        && let Some(parent) = file_path.parent()
    {
        let resolved = parent.join(&link);
        if let Ok(content) = std::fs::read_to_string(&resolved) {
            return Some(content);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_resolve_link_no_frontmatter() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        fs::write(&file_path, "# Just content").unwrap();

        let resolved = resolve_link(&file_path);
        assert_eq!(resolved, file_path);
    }

    #[test]
    fn test_resolve_link_with_frontmatter() {
        let dir = tempdir().unwrap();
        let source_path = dir.path().join("source.md");
        let target_path = dir.path().join("target.md");

        fs::write(&source_path, "Source content").unwrap();
        fs::write(&target_path, "---\nlink: source.md\n---\nLinked content").unwrap();

        let resolved = resolve_link(&target_path);
        // Compare filenames to avoid canonicalize weirdness on macOS temp dirs
        assert_eq!(resolved.file_name(), source_path.file_name());
    }
}
