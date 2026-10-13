use anyhow::{Context, Result};
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

/// Typed view of the keys RMS Memory understands. Values are read leniently so
/// notes written by other tools (Obsidian, static-site generators) still parse:
/// scalars of the wrong type are coerced or ignored instead of failing the note.
/// Unknown keys are ignored here and preserved by writers, which edit the raw
/// YAML mapping rather than re-serializing this struct.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct Frontmatter {
    #[serde(default, deserialize_with = "lenient::int")]
    pub memory_version: Option<i32>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub alias: Option<String>,
    /// Obsidian alternative note names; `[[Alias]]` links resolve to this note.
    #[serde(
        default,
        deserialize_with = "lenient::string_list",
        skip_serializing_if = "Option::is_none"
    )]
    pub aliases: Option<Vec<String>>,
    #[serde(rename = "type", default, deserialize_with = "lenient::string")]
    pub doc_type: Option<String>,
    /// Knowledge lifecycle: `active` (default), `draft`, `superseded`.
    #[serde(default, deserialize_with = "lenient::string")]
    pub status: Option<String>,
    /// Document id this note supersedes (soft replace chain).
    #[serde(default, deserialize_with = "lenient::string")]
    pub supersedes: Option<String>,
    /// Document id that replaced this note when status=superseded.
    #[serde(default, deserialize_with = "lenient::string")]
    pub superseded_by: Option<String>,
    /// Inclusive start of factual validity (RFC3339), optional.
    #[serde(default, deserialize_with = "lenient::string")]
    pub valid_from: Option<String>,
    /// Exclusive/inclusive end of factual validity (RFC3339), optional.
    #[serde(default, deserialize_with = "lenient::string")]
    pub valid_until: Option<String>,
    /// When the fact was learned (RFC3339), optional; used by doctor freshness lint.
    #[serde(default, deserialize_with = "lenient::string")]
    pub learned_at: Option<String>,
    /// Checkpoint goal (docs with `type: checkpoint`).
    #[serde(default, deserialize_with = "lenient::string")]
    pub goal: Option<String>,
    /// Checkpoint pending work description (docs with `type: checkpoint`).
    #[serde(default, deserialize_with = "lenient::string")]
    pub pending: Option<String>,
    /// Vault-relative paths this checkpoint/session references.
    #[serde(default, deserialize_with = "lenient::string_list")]
    pub links: Option<Vec<String>>,
    /// When a checkpoint was closed (RFC3339), set by checkpoint_done.
    #[serde(default, deserialize_with = "lenient::string")]
    pub done_at: Option<String>,
    /// When true, the note stays in recall past temporal/`min_confidence` gates
    /// (still respects status — superseded notes stay hidden).
    #[serde(default, deserialize_with = "lenient::boolean")]
    pub pinned: Option<bool>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub link: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub last_modified_by: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub timestamp: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub created_at: Option<String>,
    #[serde(default, deserialize_with = "lenient::float")]
    pub confidence: Option<f64>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub source: Option<String>,
    #[serde(default, deserialize_with = "lenient::string")]
    pub project: Option<String>,
}

impl Frontmatter {
    /// Alternative names from both the RMS `alias` key and Obsidian `aliases`.
    pub fn all_aliases(&self) -> Vec<String> {
        self.alias
            .iter()
            .chain(self.aliases.iter().flatten())
            .filter(|alias| !alias.trim().is_empty())
            .cloned()
            .collect()
    }
}

mod lenient {
    use serde::{Deserialize, Deserializer};
    use serde_yaml::Value;

    fn scalar(value: &Value) -> Option<String> {
        match value {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Tagged(tagged) => scalar(&tagged.value),
            _ => None,
        }
    }

    pub fn string<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Sequence(items)) => items.iter().find_map(scalar),
            Some(value) => scalar(&value),
            None => None,
        })
    }

    pub fn string_list<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Vec<String>>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Sequence(items)) => Some(items.iter().filter_map(scalar).collect()),
            Some(value) => scalar(&value).map(|s| vec![s]),
            None => None,
        })
    }

    pub fn float<'de, D: Deserializer<'de>>(d: D) -> Result<Option<f64>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Number(n)) => n.as_f64(),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        })
    }

    pub fn int<'de, D: Deserializer<'de>>(d: D) -> Result<Option<i32>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Number(n)) => n.as_i64().and_then(|n| i32::try_from(n).ok()),
            Some(Value::String(s)) => s.trim().parse().ok(),
            _ => None,
        })
    }

    pub fn boolean<'de, D: Deserializer<'de>>(d: D) -> Result<Option<bool>, D::Error> {
        Ok(match Option::<Value>::deserialize(d)? {
            Some(Value::Bool(b)) => Some(b),
            Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
                "true" | "yes" | "on" | "1" => Some(true),
                "false" | "no" | "off" | "0" => Some(false),
                _ => None,
            },
            Some(Value::Number(n)) => n.as_i64().map(|n| n != 0),
            _ => None,
        })
    }
}

impl Document {
    pub fn parse(path: &Path) -> Result<Self> {
        let text = fs::read_to_string(path)?;
        let mut frontmatter = None;
        let mut content = text.clone();

        if text.starts_with("---\n") || text.starts_with("---\r\n") {
            let split = crate::frontmatter::split(&text)
                .with_context(|| format!("Unclosed YAML frontmatter in {}", path.display()))?;
            let fm = if split.yaml.trim().is_empty() {
                Frontmatter::default()
            } else {
                serde_yaml::from_str::<Frontmatter>(split.yaml)
                    .with_context(|| format!("Invalid YAML frontmatter in {}", path.display()))?
            };
            frontmatter = Some(fm);
            content = split.body.to_string();
        }

        Ok(Document {
            path: path.to_path_buf(),
            frontmatter,
            content,
            original_text: text,
        })
    }

    /// Like [`Document::parse`], but a note whose frontmatter is not valid YAML
    /// is still returned (without frontmatter) so its text stays searchable.
    /// The second value is the frontmatter error, if any. Only I/O fails.
    pub fn parse_for_index(path: &Path) -> Result<(Self, Option<anyhow::Error>)> {
        match Self::parse(path) {
            Ok(doc) => Ok((doc, None)),
            Err(error) => {
                let text = fs::read_to_string(path)?;
                let content = crate::frontmatter::split(&text)
                    .map(|split| split.body.to_string())
                    .unwrap_or_else(|| text.clone());
                Ok((
                    Document {
                        path: path.to_path_buf(),
                        frontmatter: None,
                        content,
                        original_text: text,
                    },
                    Some(error),
                ))
            }
        }
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

    /// Repairs duplicate, missing, and observed attached-delimiter frontmatter IDs.
    /// A backup is created before the source file is replaced.
    pub fn repair_duplicate_ids(path: &Path) -> Result<bool> {
        let text = fs::read_to_string(path)?;
        if !text.starts_with("---\n") && !text.starts_with("---\r\n") {
            // Legacy Markdown may not have frontmatter at all. It is still a
            // repairable missing-ID document; create the same rolling backup
            // used by frontmatter repairs before adding a minimal block.
            let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
            let file_name = path.file_name().unwrap_or_default().to_string_lossy();
            let backup = path.with_file_name(format!("{file_name}.bak.frontmatter.{timestamp}"));
            fs::copy(path, &backup)
                .with_context(|| format!("Failed to back up {}", path.display()))?;
            fs::write(path, format!("---\nid: {}\n---\n\n{text}", Uuid::new_v4()))
                .with_context(|| format!("Failed to repair {}", path.display()))?;
            return Ok(true);
        }

        let (newline, opening_len) = if text.starts_with("---\r\n") {
            ("\r\n", 5)
        } else {
            ("\n", 4)
        };
        let closing_marker = format!("{newline}---{newline}");
        let body = &text[opening_len..];
        let mut recovered_attached_delimiter = false;
        let empty_block_rest = body.strip_prefix(&format!("---{newline}"));
        let (frontmatter, remainder) = if let Some(rest) = empty_block_rest {
            ("", format!("{newline}---{newline}{rest}"))
        } else if let Some(relative_end) = body.find(&closing_marker) {
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
            recovered_attached_delimiter = true;
            (
                repaired_frontmatter,
                format!("{newline}---{newline}{newline}{document_body}"),
            )
        };

        // Keep the first id with a real value; null/empty ids left by older
        // writers must not shadow it or survive next to a fresh id.
        let is_real_id = |line: &str| {
            let value = line["id:".len()..].trim();
            !matches!(value, "" | "null" | "~" | "''" | "\"\"")
        };
        let lines: Vec<&str> = frontmatter.split(newline).collect();
        let id_count = lines.iter().filter(|line| line.starts_with("id:")).count();
        let kept_id = lines
            .iter()
            .position(|line| line.starts_with("id:") && is_real_id(line));
        let repaired_lines: Vec<&str> = lines
            .iter()
            .enumerate()
            .filter(|(index, line)| !line.starts_with("id:") || Some(*index) == kept_id)
            .map(|(_, line)| *line)
            .collect();
        let mut repaired_frontmatter = repaired_lines.join(newline);
        let parsed =
            serde_yaml::from_str::<Frontmatter>(&repaired_frontmatter).with_context(|| {
                format!(
                    "Frontmatter has errors beyond duplicate ids in {}",
                    path.display()
                )
            })?;
        let missing_id = parsed.id.is_none();
        if id_count <= 1 && !recovered_attached_delimiter && !missing_id {
            return Ok(false);
        }
        if missing_id {
            repaired_frontmatter.push_str(newline);
            repaired_frontmatter.push_str("id: ");
            repaired_frontmatter.push_str(&Uuid::new_v4().to_string());
        }
        let repaired = format!("---{newline}{repaired_frontmatter}{remainder}");

        let timestamp = chrono::Utc::now().format("%Y%m%d%H%M%S%3f");
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let backup = path.with_file_name(format!("{file_name}.bak.frontmatter.{timestamp}"));
        fs::copy(path, &backup).with_context(|| format!("Failed to back up {}", path.display()))?;
        fs::write(path, repaired)
            .with_context(|| format!("Failed to repair {}", path.display()))?;
        Ok(true)
    }

    /// Internal Markdown and Obsidian (`[[...]]`) links in the note body.
    pub fn vault_links(&self) -> Vec<crate::vault_links::VaultLink> {
        crate::vault_links::extract(&self.content)
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
    fn vault_links_cover_markdown_and_wikilinks() {
        let doc = Document {
            path: PathBuf::new(),
            frontmatter: None,
            content:
                "Check this [link](docs/test.md), [[Other note]] and [another](http://google.com)"
                    .to_string(),
            original_text: "".to_string(),
        };
        let targets: Vec<_> = doc.vault_links().into_iter().map(|l| l.target).collect();
        assert_eq!(targets, vec!["docs/test.md", "Other note"]);
    }

    #[test]
    fn obsidian_style_frontmatter_values_parse_leniently() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("obsidian.md");
        fs::write(
            &file_path,
            "---\nid: 20240101\naliases: [Payments, Billing flow]\ntags:\n  - finance\n\
             confidence: high\npinned: \"yes\"\nmemory_version: \"2\"\ncreated_at: 2024-01-01\n\
             links: notes/a.md\nstatus: [active]\nsource: {nested: map}\n---\n# Body",
        )
        .unwrap();

        let fm = Document::parse(&file_path).unwrap().frontmatter.unwrap();
        assert_eq!(fm.id.as_deref(), Some("20240101"));
        assert_eq!(
            fm.aliases,
            Some(vec!["Payments".to_string(), "Billing flow".to_string()])
        );
        assert_eq!(fm.confidence, None);
        assert_eq!(fm.pinned, Some(true));
        assert_eq!(fm.memory_version, Some(2));
        assert_eq!(fm.created_at.as_deref(), Some("2024-01-01"));
        assert_eq!(fm.links, Some(vec!["notes/a.md".to_string()]));
        assert_eq!(fm.status.as_deref(), Some("active"));
        assert_eq!(fm.source, None);
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
    fn repair_duplicate_ids_keeps_real_id_over_leading_null_and_is_idempotent() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("null-first.md");
        fs::write(
            &file_path,
            "---\nmemory_version: null\nid: null\nsource: test\nid: 0cf2f700-real\n---\n# Content",
        )
        .unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let repaired = fs::read_to_string(&file_path).unwrap();
        assert_eq!(repaired.matches("\nid:").count(), 1);
        let doc = Document::parse(&file_path).unwrap();
        assert_eq!(
            doc.frontmatter.unwrap().id.as_deref(),
            Some("0cf2f700-real")
        );
        assert!(!Document::repair_duplicate_ids(&file_path).unwrap());
        assert_eq!(fs::read_to_string(&file_path).unwrap(), repaired);
    }

    #[test]
    fn repair_replaces_null_id_instead_of_duplicating_it() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("null-only.md");
        fs::write(&file_path, "---\nid: null\ntype: note\n---\n# Content").unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let repaired = fs::read_to_string(&file_path).unwrap();
        assert_eq!(repaired.matches("\nid:").count(), 1);
        assert!(
            Document::parse(&file_path)
                .unwrap()
                .frontmatter
                .unwrap()
                .id
                .is_some()
        );
    }

    #[test]
    fn repair_adds_id_to_empty_frontmatter_block() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("empty.md");
        fs::write(&file_path, "---\n---\n# Content\n\n---\n\nmore").unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let doc = Document::parse(&file_path).unwrap();
        assert!(doc.frontmatter.unwrap().id.is_some());
        assert!(doc.content.contains("# Content\n\n---\n\nmore"));
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

    #[test]
    fn repair_frontmatter_recovers_attached_delimiter_without_duplicate_id() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("attached.md");
        fs::write(&file_path, "---\nid: first\nsource: test---\n\n# Content").unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let doc = Document::parse(&file_path).unwrap();
        assert_eq!(doc.frontmatter.unwrap().source.as_deref(), Some("test"));
        assert_eq!(doc.content.trim_start(), "# Content");
    }

    #[test]
    fn repair_frontmatter_adds_an_id_to_legacy_document() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("legacy.md");
        fs::write(&file_path, "---\ntype: decision\n---\n# Content").unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        assert!(
            Document::parse(&file_path)
                .unwrap()
                .frontmatter
                .and_then(|frontmatter| frontmatter.id)
                .is_some()
        );
    }

    #[test]
    fn repair_frontmatter_adds_a_block_to_plain_markdown_and_keeps_a_backup() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("plain.md");
        fs::write(&file_path, "# Legacy note\n").unwrap();

        assert!(Document::repair_duplicate_ids(&file_path).unwrap());
        let repaired = fs::read_to_string(&file_path).unwrap();
        assert!(repaired.starts_with("---\nid: "));
        assert!(repaired.ends_with("# Legacy note\n"));
        assert!(fs::read_dir(dir.path()).unwrap().flatten().any(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .contains(".bak.frontmatter.")
        }));
    }
}
