//! Leading YAML frontmatter block handling shared by parsing, auditing, and writes.
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::Path;

/// A Markdown document split at its leading `---` YAML block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrontmatterSplit<'a> {
    /// YAML between the delimiters; may be empty.
    pub yaml: &'a str,
    /// Everything after the closing delimiter line.
    pub body: &'a str,
}

/// Splits `text` at a leading frontmatter block delimited by `---` lines.
///
/// Returns `None` when the text does not open with `---` or the block is never
/// closed. The closing delimiter is searched only after the opening line, so an
/// empty block (`---\n---\n`) is valid.
pub fn split(text: &str) -> Option<FrontmatterSplit<'_>> {
    let rest = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))?;
    let mut offset = 0;
    for line in rest.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            return Some(FrontmatterSplit {
                yaml: &rest[..offset],
                body: &rest[offset + line.len()..],
            });
        }
        offset += line.len();
    }
    None
}

/// Parses frontmatter YAML into a mapping; an empty block is an empty mapping.
pub fn parse_mapping(yaml: &str) -> Result<serde_yaml::Mapping> {
    if yaml.trim().is_empty() {
        return Ok(serde_yaml::Mapping::new());
    }
    serde_yaml::from_str(yaml).context("Invalid YAML frontmatter")
}

/// Renders `mapping` as a frontmatter block followed by `body`.
pub fn compose(mapping: &serde_yaml::Mapping, body: &str) -> Result<String> {
    let yaml = serde_yaml::to_string(mapping).context("Failed to serialize frontmatter")?;
    Ok(format!("---\n{}\n---\n{}", yaml.trim_end(), body))
}

/// Runs `change` on the frontmatter mapping of the Markdown file at `path`
/// (an empty mapping when the file has none). When `change` reports a change
/// and `write` is set, the file is replaced atomically; unknown keys and the
/// body are kept. Returns whether `change` changed anything.
pub fn update_file(
    path: &Path,
    write: bool,
    change: impl FnOnce(&mut serde_yaml::Mapping) -> bool,
) -> Result<bool> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let (mut mapping, body) = match split(&text) {
        Some(split) => (
            parse_mapping(split.yaml)
                .with_context(|| format!("Invalid YAML frontmatter in {}", path.display()))?,
            split.body,
        ),
        None if text.starts_with("---\n") || text.starts_with("---\r\n") => {
            bail!("Unclosed YAML frontmatter in {}", path.display())
        }
        None => (serde_yaml::Mapping::new(), text.as_str()),
    };
    if !change(&mut mapping) {
        return Ok(false);
    }
    if write {
        let parent = path.parent().context("Note path has no parent")?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary.write_all(compose(&mapping, body)?.as_bytes())?;
        temporary
            .persist(path)
            .map_err(|error| error.error)
            .with_context(|| format!("Failed to update frontmatter in {}", path.display()))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_splits_without_panicking() {
        for text in ["---\n---\n# body\n", "---\r\n---\r\n# body\r\n"] {
            let split = split(text).expect("empty frontmatter is a closed block");
            assert_eq!(split.yaml, "");
            assert!(split.body.starts_with("# body"));
        }
    }

    #[test]
    fn splits_yaml_and_body() {
        let split = split("---\nid: a\n---\n\nBody\n").unwrap();
        assert_eq!(split.yaml, "id: a\n");
        assert_eq!(split.body, "\nBody\n");
    }

    #[test]
    fn closing_delimiter_at_end_of_file() {
        let split = split("---\nid: a\n---").unwrap();
        assert_eq!(split.yaml, "id: a\n");
        assert_eq!(split.body, "");
    }

    #[test]
    fn unclosed_or_missing_block_is_none() {
        assert!(split("---\nid: a\n").is_none());
        assert!(split("# no frontmatter\n---\n").is_none());
        assert!(split("----\nid: a\n---\n").is_none());
    }

    #[test]
    fn update_file_keeps_unknown_keys_and_refuses_unclosed_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("note.md");
        std::fs::write(&note, "---\nid: a\ntags:\n- x\n---\n# Body\n").unwrap();
        let set_project = |mapping: &mut serde_yaml::Mapping| {
            mapping.insert("project".into(), "demo".into());
            true
        };
        assert!(update_file(&note, false, set_project).unwrap());
        assert!(!std::fs::read_to_string(&note).unwrap().contains("project"));
        assert!(update_file(&note, true, set_project).unwrap());
        let text = std::fs::read_to_string(&note).unwrap();
        assert!(text.contains("project: demo") && text.contains("- x"));
        assert!(text.ends_with("---\n# Body\n"));

        let plain = dir.path().join("plain.md");
        std::fs::write(&plain, "# Plain\n").unwrap();
        assert!(update_file(&plain, true, set_project).unwrap());
        assert_eq!(
            std::fs::read_to_string(&plain).unwrap(),
            "---\nproject: demo\n---\n# Plain\n"
        );

        let unclosed = dir.path().join("unclosed.md");
        std::fs::write(&unclosed, "---\nid: a\n# Body\n").unwrap();
        assert!(update_file(&unclosed, true, set_project).is_err());
        assert_eq!(
            std::fs::read_to_string(&unclosed).unwrap(),
            "---\nid: a\n# Body\n"
        );
    }

    #[test]
    fn empty_yaml_parses_to_empty_mapping() {
        assert!(parse_mapping("").unwrap().is_empty());
        assert!(parse_mapping("  \n").unwrap().is_empty());
        assert!(parse_mapping("id: [").is_err());
    }
}
