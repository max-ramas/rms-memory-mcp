//! The rule block RMS Memory injects into IDE rule files (`.cursorrules`,
//! `AGENTS.md`, `CLAUDE.md`, ...). It repeats the memory protocol, so it is
//! kept out of the memory index and out of imports.

pub const START_MARKER: &str = "<!-- RMS-MEMORY-START -->";
pub const END_MARKER: &str = "<!-- RMS-MEMORY-END -->";

/// Removes every injected block, and any marker left without its partner.
pub fn strip(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(START_MARKER) && !text.contains(END_MARKER) {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(START_MARKER) {
        let after_start = &rest[start + START_MARKER.len()..];
        let Some(end) = after_start.find(END_MARKER) else {
            break;
        };
        out.push_str(&rest[..start]);
        rest = &after_start[end + END_MARKER.len()..];
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out.replace(START_MARKER, "").replace(END_MARKER, ""))
}

/// True when the text has nothing besides injected blocks and whitespace.
pub fn is_only_managed(text: &str) -> bool {
    text.contains(START_MARKER) && strip(text).trim().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_blocks_and_stray_markers() {
        let text = format!("# Own rules\n{START_MARKER}\nprotocol\n{END_MARKER}\nkeep\n");
        assert_eq!(strip(&text), "# Own rules\n\nkeep\n");
        assert!(!is_only_managed(&text));

        let only = format!("{START_MARKER}\nprotocol\n{END_MARKER}\n\n");
        assert!(is_only_managed(&only));
        assert_eq!(strip(&only).trim(), "");

        let stray = format!("a {START_MARKER} b {END_MARKER}{END_MARKER} c");
        assert_eq!(strip(&stray), "a  c");
        let unclosed = format!("a {START_MARKER} b");
        assert_eq!(strip(&unclosed), "a  b");

        assert!(matches!(
            strip("plain"),
            std::borrow::Cow::Borrowed("plain")
        ));
        assert!(!is_only_managed("  \n"));
    }
}
