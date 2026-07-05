use std::path::{Path, PathBuf};
use crate::document::Document;

/// Resolves a linked document.
/// If the file at `file_path` contains a `link` in its frontmatter,
/// returns the resolved absolute path to the source file.
/// Otherwise, returns the original `file_path` unmodified.
pub fn resolve_link(file_path: &Path) -> PathBuf {
    if let Ok(doc) = Document::parse(file_path) {
        if let Some(fm) = doc.frontmatter {
            if let Some(link) = fm.link {
                if let Some(parent) = file_path.parent() {
                    let resolved = parent.join(&link);
                    // Try to canonicalize if it exists, otherwise return joined path
                    if let Ok(canon) = resolved.canonicalize() {
                        return canon;
                    }
                    return resolved;
                }
            }
        }
    }
    file_path.to_path_buf()
}

/// Checks if a file is a linked document and returns the source content if so.
/// Otherwise, returns `None`.
pub fn get_linked_content(file_path: &Path) -> Option<String> {
    if let Ok(doc) = Document::parse(file_path) {
        if let Some(fm) = doc.frontmatter {
            if let Some(link) = fm.link {
                if let Some(parent) = file_path.parent() {
                    let resolved = parent.join(&link);
                    if let Ok(content) = std::fs::read_to_string(&resolved) {
                        return Some(content);
                    }
                }
            }
        }
    }
    None
}
