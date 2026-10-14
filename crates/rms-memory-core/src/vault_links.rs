//! Link extraction and resolution shared by the graph, the index and doctor.
//!
//! Supports CommonMark links (`[text](path.md)`, resolved relative to the
//! source note) and Obsidian links (`[[Note]]`, `[[folder/Note|label]]`,
//! `[[Note#Heading]]`, `![[image.png]]`), resolved by vault path, note name
//! or frontmatter alias the way Obsidian does.

use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSyntax {
    Markdown,
    Wiki,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultLink {
    /// Link target as written, without heading/block anchor or alias label.
    pub target: String,
    pub syntax: LinkSyntax,
    /// `![[...]]` / `![...](...)` embeds.
    pub embed: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkResolution {
    /// Vault-relative path of a Markdown note in the index.
    Note(String),
    /// Vault-relative path of an existing non-Markdown file (image, PDF, ...).
    Attachment(String),
    /// Best-effort normalized target that matches nothing in the vault.
    Missing(String),
}

fn markdown_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(!?)\[([^\]]*)\]\(([^)]+)\)").unwrap())
}

fn wiki_link_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(!?)\[\[([^\[\]\n]+)\]\]").unwrap())
}

/// Replaces fenced code blocks and inline code spans with spaces so link-like
/// examples in code are not treated as links. Byte offsets are preserved.
fn mask_code(content: &str) -> String {
    let mut out = String::with_capacity(content.len());
    let mut fence: Option<String> = None;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let marker = if trimmed.starts_with("```") {
            Some("```")
        } else if trimmed.starts_with("~~~") {
            Some("~~~")
        } else {
            None
        };
        match (&fence, marker) {
            (None, Some(m)) => {
                fence = Some(m.to_string());
                out.push_str(&blank(line));
            }
            (Some(open), Some(m)) if open == m => {
                fence = None;
                out.push_str(&blank(line));
            }
            (Some(_), _) => out.push_str(&blank(line)),
            (None, None) => out.push_str(&mask_inline_code(line)),
        }
    }
    out
}

fn blank(text: &str) -> String {
    text.chars()
        .map(|c| if c == '\n' { '\n' } else { ' ' })
        .collect()
}

fn mask_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(start) = rest.find('`') {
        let ticks = rest[start..].chars().take_while(|c| *c == '`').count();
        let delimiter = &rest[start..start + ticks];
        let after = &rest[start + ticks..];
        match after.find(delimiter) {
            Some(end) => {
                out.push_str(&rest[..start]);
                out.push_str(&blank(&rest[start..start + ticks + end + ticks]));
                rest = &after[end + ticks..];
            }
            None => break,
        }
    }
    out.push_str(rest);
    out
}

/// Extracts internal links from Markdown content. External URLs, `mailto:`
/// and same-note anchors are skipped, as is anything inside code.
pub fn extract(content: &str) -> Vec<VaultLink> {
    let masked = mask_code(content);
    let mut links = Vec::new();
    for cap in markdown_link_re().captures_iter(&masked) {
        let raw = cap[3].trim();
        let raw = raw
            .strip_prefix('<')
            .and_then(|r| r.split_once('>').map(|(inner, _)| inner))
            .unwrap_or_else(|| raw.split_whitespace().next().unwrap_or(""));
        if raw.contains("://") || raw.starts_with("mailto:") || raw.starts_with('#') {
            continue;
        }
        let target = percent_decode(strip_anchor(raw));
        if !target.is_empty() {
            links.push(VaultLink {
                target,
                syntax: LinkSyntax::Markdown,
                embed: !cap[1].is_empty(),
            });
        }
    }
    for cap in wiki_link_re().captures_iter(&masked) {
        // `[[4]](https://…)` is a Markdown link whose text is `[4]`
        // (pasted citations), not a wikilink.
        let end = cap.get(0).map_or(0, |m| m.end());
        if masked[end..].starts_with('(') {
            continue;
        }
        // Inside Markdown tables the alias separator is written `\|`.
        let inner = cap[2].split('|').next().unwrap_or("");
        let target = strip_anchor(inner)
            .trim_end_matches('\\')
            .trim()
            .to_string();
        if !target.is_empty() && !is_template_placeholder(&target) {
            links.push(VaultLink {
                target,
                syntax: LinkSyntax::Wiki,
                embed: !cap[1].is_empty(),
            });
        }
    }
    links
}

/// Templater / Periodic Notes placeholders such as `<% tp.date.now() %>` or
/// `{{monday:YYYY-MM-DD}}` are expanded when a note is created from a template.
fn is_template_placeholder(target: &str) -> bool {
    target.contains("<%") || target.contains("{{")
}

fn strip_anchor(target: &str) -> &str {
    target.split('#').next().unwrap_or("")
}

fn percent_decode(input: &str) -> String {
    fn hex(byte: u8) -> Option<u8> {
        (byte as char).to_digit(16).map(|digit| digit as u8)
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(&[high, low]) = bytes.get(i + 1..i + 3)
            && let (Some(high), Some(low)) = (hex(high), hex(low))
        {
            out.push(high << 4 | low);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| input.to_string())
}

/// Joins `link` onto `base_dir` (both vault-relative, `/`-separated),
/// collapsing `.` and `..`. Returns `None` when the path escapes the vault.
fn join_relative(base_dir: &str, link: &str) -> Option<String> {
    let mut parts: Vec<&str> = if link.starts_with('/') {
        Vec::new()
    } else {
        base_dir.split('/').filter(|p| !p.is_empty()).collect()
    };
    for part in link.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

fn parent_dir(path: &str) -> &str {
    path.rsplit_once('/').map(|(dir, _)| dir).unwrap_or("")
}

fn has_non_markdown_extension(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    match name.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && !ext.is_empty()
                && !ext.eq_ignore_ascii_case("md")
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

fn note_key(name: &str) -> String {
    name.trim_end_matches(".md").to_lowercase()
}

/// Resolves links against the set of indexed vault notes.
pub struct VaultLinkIndex {
    root: std::path::PathBuf,
    notes: HashSet<String>,
    notes_lower: HashMap<String, String>,
    by_name: HashMap<String, Vec<String>>,
    attachments: OnceLock<HashMap<String, Vec<String>>>,
}

impl VaultLinkIndex {
    /// `notes` are vault-relative `/`-separated Markdown paths.
    pub fn new(root: &Path, notes: impl IntoIterator<Item = String>) -> Self {
        let mut index = Self {
            root: root.to_path_buf(),
            notes: HashSet::new(),
            notes_lower: HashMap::new(),
            by_name: HashMap::new(),
            attachments: OnceLock::new(),
        };
        for note in notes {
            let name = note.rsplit('/').next().unwrap_or(&note);
            index
                .by_name
                .entry(note_key(name))
                .or_default()
                .push(note.clone());
            index.notes_lower.insert(note.to_lowercase(), note.clone());
            index.notes.insert(note);
        }
        for candidates in index.by_name.values_mut() {
            candidates.sort();
        }
        index
    }

    /// Builds the index from absolute Markdown paths inside `root`.
    pub fn from_files(root: &Path, files: &[std::path::PathBuf]) -> Self {
        Self::new(
            root,
            files.iter().filter_map(|file| {
                file.strip_prefix(root)
                    .ok()
                    .map(|rel| rel.to_string_lossy().replace('\\', "/"))
            }),
        )
    }

    /// Registers Obsidian `aliases` of a note so `[[Alias]]` resolves to it.
    pub fn add_aliases(&mut self, note: &str, aliases: &[String]) {
        for alias in aliases {
            let key = note_key(alias.trim());
            if key.is_empty() {
                continue;
            }
            let candidates = self.by_name.entry(key).or_default();
            if !candidates.iter().any(|c| c == note) {
                candidates.push(note.to_string());
            }
        }
    }

    pub fn contains(&self, note: &str) -> bool {
        self.notes.contains(note)
    }

    pub fn resolve(&self, source: &str, link: &VaultLink) -> LinkResolution {
        match link.syntax {
            LinkSyntax::Markdown => self.resolve_markdown(source, &link.target),
            LinkSyntax::Wiki => self.resolve_wiki(source, &link.target),
        }
    }

    fn resolve_markdown(&self, source: &str, target: &str) -> LinkResolution {
        let Some(relative) = join_relative(parent_dir(source), target) else {
            return LinkResolution::Missing(target.to_string());
        };
        if let Some(found) = self.find_note(&relative) {
            return LinkResolution::Note(found);
        }
        // Older vault notes link vault-root-relative paths.
        if let Some(rooted) = join_relative("", target)
            && let Some(found) = self.find_note(&rooted)
        {
            return LinkResolution::Note(found);
        }
        for candidate in [Some(relative.clone()), join_relative("", target)]
            .into_iter()
            .flatten()
        {
            if self.root.join(&candidate).exists() {
                return LinkResolution::Attachment(candidate);
            }
        }
        // Obsidian resolves Markdown links like wikilinks when the path does
        // not exist: by file name anywhere in the vault.
        match self.resolve_wiki(source, target) {
            LinkResolution::Missing(_) => LinkResolution::Missing(relative),
            found => found,
        }
    }

    fn resolve_wiki(&self, source: &str, target: &str) -> LinkResolution {
        let target = target.trim_start_matches('/');
        // `[[Node.js]]` names a note whose title contains a dot.
        if has_non_markdown_extension(target) {
            match self.resolve_attachment(source, target) {
                LinkResolution::Missing(_) => {}
                found => return found,
            }
        }
        if target.contains('/') {
            for candidate in [
                join_relative("", target),
                join_relative(parent_dir(source), target),
            ]
            .into_iter()
            .flatten()
            {
                if let Some(found) = self.find_note(&candidate) {
                    return LinkResolution::Note(found);
                }
            }
            let suffix = format!("/{}", note_key(target));
            let name = note_key(target.rsplit('/').next().unwrap_or(target));
            if let Some(found) = self.pick(
                source,
                self.by_name
                    .get(&name)
                    .into_iter()
                    .flatten()
                    .filter(|path| note_key(&format!("/{path}")).ends_with(&suffix)),
            ) {
                return LinkResolution::Note(found);
            }
            // Aliases may themselves contain `/` (e.g. an old path kept after a move).
            if let Some(found) = self.pick(
                source,
                self.by_name.get(&note_key(target)).into_iter().flatten(),
            ) {
                return LinkResolution::Note(found);
            }
            return LinkResolution::Missing(target.to_string());
        }
        match self.pick(
            source,
            self.by_name.get(&note_key(target)).into_iter().flatten(),
        ) {
            Some(found) => LinkResolution::Note(found),
            None => LinkResolution::Missing(target.to_string()),
        }
    }

    fn resolve_attachment(&self, source: &str, target: &str) -> LinkResolution {
        for candidate in [
            join_relative("", target),
            join_relative(parent_dir(source), target),
        ]
        .into_iter()
        .flatten()
        {
            if self.root.join(&candidate).is_file() {
                return LinkResolution::Attachment(candidate);
            }
        }
        let name = target.rsplit('/').next().unwrap_or(target).to_lowercase();
        match self.attachment_names().get(&name).and_then(|c| c.first()) {
            Some(found) => LinkResolution::Attachment(found.clone()),
            None => LinkResolution::Missing(target.to_string()),
        }
    }

    fn find_note(&self, path: &str) -> Option<String> {
        let with_ext = if path.to_lowercase().ends_with(".md") {
            path.to_string()
        } else {
            format!("{path}.md")
        };
        [path.to_string(), with_ext].into_iter().find_map(|p| {
            if self.notes.contains(&p) {
                Some(p)
            } else {
                self.notes_lower.get(&p.to_lowercase()).cloned()
            }
        })
    }

    /// Obsidian prefers a note next to the source, then the shortest path.
    fn pick<'a>(
        &self,
        source: &str,
        candidates: impl Iterator<Item = &'a String>,
    ) -> Option<String> {
        let source_dir = parent_dir(source);
        candidates
            .min_by_key(|path| {
                (
                    parent_dir(path) != source_dir,
                    path.matches('/').count(),
                    path.len(),
                    (*path).clone(),
                )
            })
            .cloned()
    }

    fn attachment_names(&self) -> &HashMap<String, Vec<String>> {
        self.attachments.get_or_init(|| {
            let mut names: HashMap<String, Vec<String>> = HashMap::new();
            let entries = walkdir::WalkDir::new(&self.root)
                .into_iter()
                .filter_entry(|entry| {
                    entry.depth() == 0 || !entry.file_name().to_string_lossy().starts_with('.')
                })
                .flatten();
            for entry in entries {
                let Ok(relative) = entry.path().strip_prefix(&self.root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !entry.file_type().is_file() || relative.ends_with(".md") {
                    continue;
                }
                let name = relative.rsplit('/').next().unwrap_or(&relative);
                names
                    .entry(name.to_lowercase())
                    .or_default()
                    .push(relative.clone());
            }
            for candidates in names.values_mut() {
                candidates.sort_by_key(|path| (path.matches('/').count(), path.clone()));
            }
            names
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wiki(target: &str) -> VaultLink {
        VaultLink {
            target: target.into(),
            syntax: LinkSyntax::Wiki,
            embed: false,
        }
    }

    fn markdown(target: &str) -> VaultLink {
        VaultLink {
            target: target.into(),
            syntax: LinkSyntax::Markdown,
            embed: false,
        }
    }

    #[test]
    fn extracts_markdown_and_obsidian_links_outside_code() {
        let content = "See [db](../decisions/db.md#why) and [[Payments Flow|payments]], \
                       ![[diagram.png]], [[#Local heading]], [web](https://x.dev), \
                       [spaced](my%20note.md).\n\n`[[not a link]]`\n\n```\n[[also not]]\n[x](nope.md)\n```\n";
        let links = extract(content);
        assert_eq!(
            links,
            vec![
                markdown("../decisions/db.md"),
                markdown("my note.md"),
                wiki("Payments Flow"),
                VaultLink {
                    target: "diagram.png".into(),
                    syntax: LinkSyntax::Wiki,
                    embed: true
                },
            ]
        );
    }

    #[test]
    fn aliases_containing_a_path_resolve() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = VaultLinkIndex::new(dir.path(), ["en/UI/Ribbon.md".to_string()]);
        index.add_aliases(
            "en/UI/Ribbon.md",
            &["User interface/Workspace/Ribbon".to_string()],
        );
        assert_eq!(
            index.resolve("en/a.md", &wiki("User interface/Workspace/Ribbon")),
            LinkResolution::Note("en/UI/Ribbon.md".into())
        );
    }

    #[test]
    fn escaped_table_alias_is_stripped() {
        assert_eq!(
            extract("| [[Table view\\|Table]] | [[Cards#Size\\|cards]] |"),
            vec![wiki("Table view"), wiki("Cards")]
        );
    }

    #[test]
    fn citations_and_template_placeholders_are_not_wikilinks() {
        let content = "Bias.[[4]](https://en.wikipedia.org/wiki/X#cite) \
                       [[<% tp.date.now(\"YYYY-MM-DD\", -1) %>]] [[{{monday:YYYY-MM-DD}}]] [[Real]]";
        assert_eq!(extract(content), vec![wiki("Real")]);
    }

    #[test]
    fn resolves_wiki_links_by_name_path_and_alias() {
        let dir = tempfile::tempdir().unwrap();
        let mut index = VaultLinkIndex::new(
            dir.path(),
            [
                "notes/Payments Flow.md".to_string(),
                "archive/Payments Flow.md".to_string(),
                "decisions/db.md".to_string(),
                "Inbox.md".to_string(),
            ],
        );
        index.add_aliases("decisions/db.md", &["Database choice".to_string()]);

        assert_eq!(
            index.resolve("notes/today.md", &wiki("payments flow")),
            LinkResolution::Note("notes/Payments Flow.md".into())
        );
        assert_eq!(
            index.resolve("other/x.md", &wiki("archive/Payments Flow")),
            LinkResolution::Note("archive/Payments Flow.md".into())
        );
        assert_eq!(
            index.resolve("x.md", &wiki("Database choice")),
            LinkResolution::Note("decisions/db.md".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &wiki("Inbox.md")),
            LinkResolution::Note("Inbox.md".into())
        );
        assert_eq!(
            index.resolve("x.md", &wiki("Nowhere")),
            LinkResolution::Missing("Nowhere".into())
        );
    }

    #[test]
    fn resolves_markdown_links_relative_then_from_root() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("assets")).unwrap();
        std::fs::write(dir.path().join("assets/pic.png"), b"png").unwrap();
        let index = VaultLinkIndex::new(
            dir.path(),
            ["decisions/db.md".to_string(), "notes/a.md".to_string()],
        );

        assert_eq!(
            index.resolve("notes/a.md", &markdown("../decisions/db.md")),
            LinkResolution::Note("decisions/db.md".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("decisions/db.md")),
            LinkResolution::Note("decisions/db.md".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("../assets/pic.png")),
            LinkResolution::Attachment("assets/pic.png".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("../../escape.md")),
            LinkResolution::Missing("../../escape.md".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("db.md")),
            LinkResolution::Note("decisions/db.md".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("pic.png")),
            LinkResolution::Attachment("assets/pic.png".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &markdown("/assets")),
            LinkResolution::Attachment("assets".into())
        );
    }

    #[test]
    fn malformed_percent_escapes_are_kept_as_text() {
        for (link, expected) in [
            ("%1ж.md", "%1ж.md"),
            ("%a€.md", "%a€.md"),
            ("%+5.md", "%+5.md"),
            ("100%", "100%"),
            ("my%20note.md", "my note.md"),
            ("%D0%B6.md", "ж.md"),
        ] {
            assert_eq!(percent_decode(link), expected, "{link}");
        }
    }

    #[test]
    fn note_names_with_dots_are_notes_not_attachments() {
        let dir = tempfile::tempdir().unwrap();
        let index = VaultLinkIndex::new(
            dir.path(),
            [
                "Node.js.md".to_string(),
                "Meeting 2024.01.15.md".to_string(),
            ],
        );
        for name in ["Node.js", "Meeting 2024.01.15"] {
            assert_eq!(
                index.resolve("a.md", &wiki(name)),
                LinkResolution::Note(format!("{name}.md"))
            );
        }
    }

    #[test]
    fn embeds_find_attachments_anywhere_by_file_name() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("files/img")).unwrap();
        std::fs::write(dir.path().join("files/img/Diagram.png"), b"png").unwrap();
        let index = VaultLinkIndex::new(dir.path(), ["notes/a.md".to_string()]);

        assert_eq!(
            index.resolve("notes/a.md", &wiki("diagram.png")),
            LinkResolution::Attachment("files/img/Diagram.png".into())
        );
        assert_eq!(
            index.resolve("notes/a.md", &wiki("missing.pdf")),
            LinkResolution::Missing("missing.pdf".into())
        );
    }
}
