//! Leading YAML frontmatter block handling shared by parsing, auditing, and writes.
use anyhow::{Context, Result};

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
    fn empty_yaml_parses_to_empty_mapping() {
        assert!(parse_mapping("").unwrap().is_empty());
        assert!(parse_mapping("  \n").unwrap().is_empty());
        assert!(parse_mapping("id: [").is_err());
    }
}
