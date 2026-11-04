//! Markdown section helpers for `rms_read`: outline, one section by heading, character window.
//! Lets an agent read the part of a long note it needs instead of the whole note.

use anyhow::{Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub level: usize,
    pub title: String,
    /// Byte offset of the heading line.
    pub start: usize,
    /// Byte offset where this section ends including sub-sections.
    pub end: usize,
    /// Byte offset where the next heading (any level) starts: the section's own text ends here.
    pub own_end: usize,
}

fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let indent = trimmed.len() - trimmed.trim_start_matches(' ').len();
    if indent > 3 {
        return None;
    }
    let rest = &trimmed[indent..];
    let level = rest.chars().take_while(|c| *c == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let after = &rest[level..];
    if !after.is_empty() && !after.starts_with([' ', '\t']) {
        return None;
    }
    let title = after.trim().trim_end_matches('#').trim().to_string();
    Some((level, title))
}

/// Headings outside code fences and outside the frontmatter block, in document order.
pub fn sections(text: &str) -> Vec<Section> {
    let body_start = crate::frontmatter::split(text)
        .map(|s| text.len() - s.body.len())
        .unwrap_or(0);
    let mut found: Vec<(usize, usize, String)> = Vec::new();
    let mut in_fence = false;
    let mut offset = 0usize;
    for line in text.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        if start < body_start {
            continue;
        }
        let t = line.trim_start();
        if t.starts_with("```") || t.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if let Some((level, title)) = parse_heading(line) {
            found.push((start, level, title));
        }
    }
    (0..found.len())
        .map(|i| {
            let (start, level, ref title) = found[i];
            let end = found[i + 1..]
                .iter()
                .find(|(_, l, _)| *l <= level)
                .map(|(s, _, _)| *s)
                .unwrap_or(text.len());
            let own_end = found.get(i + 1).map(|(s, _, _)| *s).unwrap_or(text.len());
            Section {
                level,
                title: title.clone(),
                start,
                end,
                own_end,
            }
        })
        .collect()
}

fn normalize(heading: &str) -> String {
    heading.trim().trim_start_matches('#').trim().to_lowercase()
}

/// Section text (heading line included) for `heading`: exact title first, then substring.
pub fn section_text<'a>(text: &'a str, heading: &str) -> Result<&'a str> {
    let all = sections(text);
    let want = normalize(heading);
    let hit = all
        .iter()
        .find(|s| s.title.to_lowercase() == want)
        .or_else(|| all.iter().find(|s| s.title.to_lowercase().contains(&want)));
    match hit {
        Some(s) => Ok(&text[s.start..s.end]),
        None => {
            let titles: Vec<String> = all.iter().take(25).map(|s| s.title.clone()).collect();
            bail!(
                "no heading matching `{heading}`; available: {}",
                if titles.is_empty() {
                    "(the note has no headings)".to_string()
                } else {
                    titles.join(" | ")
                }
            )
        }
    }
}

/// Compact table of contents: size of each section's own text, plus frontmatter `summary`.
pub fn outline(path: &str, text: &str, hash: &str) -> String {
    let all = sections(text);
    let total = text.chars().count();
    let mut out = format!(
        "{path} — {total} chars, {} section(s), hash {hash}\n",
        all.len()
    );
    if let Some(summary) = crate::frontmatter::split(text)
        .and_then(|s| crate::frontmatter::parse_mapping(s.yaml).ok())
        .and_then(|m| {
            m.get("summary")
                .and_then(|v| v.as_str().map(|s| s.trim().to_string()))
        })
        .filter(|s| !s.is_empty())
    {
        out.push_str(&format!("summary: {summary}\n"));
    }
    let first = all.first().map(|s| s.start).unwrap_or(text.len());
    let body_start = crate::frontmatter::split(text)
        .map(|s| text.len() - s.body.len())
        .unwrap_or(0);
    if first > body_start && !text[body_start..first].trim().is_empty() {
        out.push_str(&format!(
            "(intro) [{}c]\n",
            text[body_start..first].chars().count()
        ));
    }
    for s in &all {
        let own = text[s.start..s.own_end].chars().count();
        out.push_str(&format!(
            "{}{} {} [{}c]\n",
            "  ".repeat(s.level.saturating_sub(1)),
            "#".repeat(s.level),
            s.title,
            own
        ));
    }
    out.push_str("Read one part with `heading`, or a window with `offset`/`max_chars`.");
    out
}

/// `max_chars` characters starting at character `offset`. Returns the window and, when text
/// remains, the next offset.
pub fn window(text: &str, offset: usize, max_chars: Option<usize>) -> (String, Option<usize>) {
    let total = text.chars().count();
    let start = offset.min(total);
    let end = max_chars.map_or(total, |m| start.saturating_add(m).min(total));
    let slice: String = text.chars().skip(start).take(end - start).collect();
    (slice, (end < total).then_some(end))
}

pub fn content_hash(text: &str) -> String {
    blake3::hash(text.as_bytes()).to_hex()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\nsummary: Short version\n# not a heading in yaml\n---\nintro text\n\n# One\nalpha\n## One.A\nbeta\n```\n# fenced, not a heading\n```\n# Two\ngamma\n";

    #[test]
    fn finds_headings_skipping_frontmatter_and_fences() {
        let titles: Vec<_> = sections(DOC)
            .into_iter()
            .map(|s| (s.level, s.title))
            .collect();
        assert_eq!(
            titles,
            [(1, "One".into()), (2, "One.A".into()), (1, "Two".into())]
        );
        assert!(parse_heading("#nospace").is_none());
        assert!(parse_heading("    # indented code").is_none());
        assert_eq!(parse_heading("## Title ##").unwrap(), (2, "Title".into()));
    }

    #[test]
    fn section_includes_subsections_and_stops_at_same_level() {
        let one = section_text(DOC, "one").unwrap();
        assert!(one.starts_with("# One"));
        assert!(one.contains("beta") && one.contains("fenced") && !one.contains("gamma"));
        let sub = section_text(DOC, "## One.A").unwrap();
        assert!(sub.contains("beta") && !sub.contains("alpha"));
        assert!(section_text(DOC, "tw").unwrap().contains("gamma")); // substring fallback
        let err = section_text(DOC, "missing").unwrap_err().to_string();
        assert!(err.contains("One | One.A | Two"), "{err}");
    }

    #[test]
    fn outline_lists_sizes_summary_and_intro() {
        let o = outline("notes/a.md", DOC, "abc123");
        assert!(
            o.contains("hash abc123") && o.contains("3 section(s)"),
            "{o}"
        );
        assert!(o.contains("summary: Short version"));
        assert!(o.contains("(intro) [") && o.contains("# One [") && o.contains("  ## One.A ["));
        assert!(outline("x.md", "no headings", "h").contains("0 section(s)"));
    }

    #[test]
    fn window_is_char_based_and_reports_next_offset() {
        let text = "héllo wörld";
        assert_eq!(window(text, 0, Some(5)), ("héllo".into(), Some(5)));
        assert_eq!(window(text, 6, Some(100)), ("wörld".into(), None));
        assert_eq!(window(text, 100, Some(5)), (String::new(), None));
        assert_eq!(window(text, 0, None).1, None);
    }

    #[test]
    fn hash_is_stable_and_content_sensitive() {
        assert_eq!(content_hash("a"), content_hash("a"));
        assert_ne!(content_hash("a"), content_hash("b"));
        assert_eq!(content_hash("a").len(), 16);
    }
}
