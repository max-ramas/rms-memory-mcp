use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct Document {
    pub path: PathBuf,
    pub frontmatter: Option<Frontmatter>,
    pub content: String,
    pub original_text: String,
}

#[derive(Deserialize, Serialize, Debug, Clone)]
pub struct Frontmatter {
    pub memory_version: Option<i32>,
    pub id: Option<String>,
    pub alias: Option<String>,
    #[serde(rename = "type")]
    pub doc_type: Option<String>,
    pub status: Option<String>,
    pub link: Option<String>,
    pub last_modified_by: Option<String>,
    pub timestamp: Option<String>,
    pub created_at: Option<String>,
    pub confidence: Option<f64>,
    pub source: Option<String>,
}

impl Document {
    pub fn parse(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let mut frontmatter = None;
        let mut content = text.clone();

        if text.starts_with("---\n") || text.starts_with("---\r\n") {
            let end_idx = text
                .find("\n---\n")
                .or_else(|| text.find("\r\n---\r\n"))
                .with_context(|| format!("Unclosed YAML frontmatter in {}", path.display()))?;
            let fm_text = &text[4..end_idx];
            let fm = serde_yaml::from_str::<Frontmatter>(fm_text)
                .with_context(|| format!("Invalid YAML frontmatter in {}", path.display()))?;
            frontmatter = Some(fm);

            let content_start = end_idx
                + if text[end_idx..].starts_with("\r\n") {
                    7
                } else {
                    5
                };
            if content_start <= text.len() {
                content = text[content_start..].to_string();
            }
        }

        Ok(Document {
            path: path.to_path_buf(),
            frontmatter,
            content,
            original_text: text,
        })
    }

    /// Returns the persisted ID when present, otherwise a deterministic path-derived ID.
    /// Indexing must never mutate vault files merely to add metadata.
    pub fn index_id(&self, relative_path: &Path) -> String {
        self.frontmatter
            .as_ref()
            .and_then(|fm| fm.id.clone())
            .unwrap_or_else(|| {
                format!(
                    "path:{}",
                    blake3::hash(relative_path.to_string_lossy().as_bytes())
                )
            })
    }

    /// Repairs only duplicate top-level `id:` keys, retaining the first ID.
    /// A backup is created before the source file is replaced.
    pub fn repair_duplicate_ids(path: &Path) -> Result<bool> {
        let text = fs::read_to_string(path)?;
        if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
            return Ok(false);
        }

        let (newline, opening_len) = if text.starts_with("---\r\n") {
            ("\r\n", 5)
        } else {
            ("\n", 4)
        };
        let closing_marker = format!("{newline}---{newline}");
        let body = &text[opening_len..];
        let (frontmatter, remainder) = if let Some(relative_end) = body.find(&closing_marker) {
            (&body[..relative_end], body[relative_end..].to_string())
        } else {
            // Recover the observed corruption where the closing delimiter was
            // concatenated to the final scalar: `source: value---\n\n# Body`.
            let paragraph_break = format!("{newline}{newline}");
            let separator = body
                .find(&paragraph_break)
                .with_context(|| format!("Unclosed YAML frontmatter in {}", path.display()))?;
            let candidate = &body[..separator];
            let repaired_frontmatter = candidate
                .strip_suffix("---")
                .with_context(|| format!("Unclosed YAML frontmatter in {}", path.display()))?;
            let document_body = &body[separator + paragraph_break.len()..];
            (
                repaired_frontmatter,
                format!("{newline}---{newline}{newline}{document_body}"),
            )
        };

        let mut id_count = 0usize;
        let mut repaired_lines = Vec::new();
        for line in frontmatter.split(newline) {
            if line.starts_with("id:") {
                id_count += 1;
                if id_count > 1 {
                    continue;
                }
            }
            repaired_lines.push(line);
        }
        if id_count <= 1 {
            return Ok(false);
        }

        let repaired_frontmatter = repaired_lines.join(newline);
        serde_yaml::from_str::<Frontmatter>(&repaired_frontmatter).with_context(|| {
            format!(
                "Frontmatter has errors beyond duplicate ids in {}",
                path.display()
            )
        })?;
        let repaired = format!("---{newline}{repaired_frontmatter}{remainder}");

        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let backup = path.with_file_name(format!("{file_name}.bak.frontmatter.{timestamp}"));
        fs::copy(path, &backup).with_context(|| format!("Failed to back up {}", path.display()))?;
        fs::write(path, repaired)
            .with_context(|| format!("Failed to repair {}", path.display()))?;
        Ok(true)
    }

    pub fn ensure_id(&mut self) -> Result<String> {
        if let Some(fm) = &self.frontmatter
            && let Some(id) = &fm.id
        {
            return Ok(id.clone());
        }

        let new_id = Uuid::new_v4().to_string();

        let new_text = if self.original_text.starts_with("---\n")
            || self.original_text.starts_with("---\r\n")
        {
            let id_line = format!("id: {}\n", new_id);
            self.original_text
                .replacen("---\n", &format!("---\n{}", id_line), 1)
                .replacen("---\r\n", &format!("---\r\n{}", id_line), 1)
        } else {
            format!("---\nid: {}\n---\n\n{}", new_id, self.original_text)
        };

        fs::write(&self.path, &new_text)?;
        self.original_text = new_text;

        // Re-parse to update frontmatter
        let updated = Self::parse(&self.path)?;
        self.frontmatter = updated.frontmatter;
        self.content = updated.content;

        Ok(new_id)
    }

    pub fn extract_links(&self) -> Vec<String> {
        let re = Regex::new(r"\[([^\]]+)\]\(([^)]+)\)").unwrap();
        let mut links = Vec::new();
        for cap in re.captures_iter(&self.content) {
            let link = cap[2].to_string();
            if !link.starts_with("http") && !link.starts_with("mailto:") && !link.starts_with('#') {
                let cleaned = link.split('#').next().unwrap_or("").to_string();
                if !cleaned.is_empty() {
                    links.push(cleaned);
                }
            }
        }
        links
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_parse_frontmatter() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.md");
        fs::write(&file_path, "---\nid: test-123\n---\n# Content").unwrap();

        let doc = Document::parse(&file_path).unwrap();
        assert_eq!(doc.frontmatter.unwrap().id.unwrap(), "test-123");
        assert_eq!(doc.content, "# Content");
    }

    #[test]
    fn test_extract_links() {
        let doc = Document {
            path: PathBuf::new(),
            frontmatter: None,
            content: "Check this [link](docs/test.md) and [another](http://google.com)".to_string(),
            original_text: "".to_string(),
        };
        let links = doc.extract_links();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0], "docs/test.md");
    }

    #[test]
    fn invalid_frontmatter_is_an_error_and_does_not_mutate_file() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("duplicate.md");
        let original = "---\nid: first\nid: second\n---\n# Content";
        fs::write(&file_path, original).unwrap();

        let error = Document::parse(&file_path).unwrap_err();

        assert!(error.to_string().contains("Invalid YAML frontmatter"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), original);
    }

    #[test]
    fn path_derived_index_id_is_stable_without_writing() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("plain.md");
        fs::write(&file_path, "# Content").unwrap();
        let doc = Document::parse(&file_path).unwrap();

        let first = doc.index_id(Path::new("docs/plain.md"));
        let second = doc.index_id(Path::new("docs/plain.md"));

        assert_eq!(first, second);
        assert!(first.starts_with("path:"));
        assert_eq!(fs::read_to_string(&file_path).unwrap(), "# Content");
    }

    #[test]
    fn repair_duplicate_ids_keeps_first_and_creates_backup() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("duplicate.md");
        fs::write(
            &file_path,
            "---\nid: first\nid: second\ntype: decision\n---\n# Content",
        )
        .unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let repaired = fs::read_to_string(&file_path).unwrap();
        assert_eq!(repaired.matches("id:").count(), 1);
        assert_eq!(
            Document::parse(&file_path)
                .unwrap()
                .frontmatter
                .unwrap()
                .id
                .as_deref(),
            Some("first")
        );
        assert_eq!(
            fs::read_dir(dir.path())
                .unwrap()
                .flatten()
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .contains(".bak.frontmatter."))
                .count(),
            1
        );
    }

    #[test]
    fn repair_duplicate_ids_recovers_attached_closing_delimiter() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("attached.md");
        fs::write(
            &file_path,
            "---\nid: first\nid: second\nsource: test---\n\n# Content",
        )
        .unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let doc = Document::parse(&file_path).unwrap();

        assert_eq!(doc.frontmatter.unwrap().id.as_deref(), Some("first"));
        assert_eq!(doc.content.trim_start(), "# Content");
    }
}
