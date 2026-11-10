//! DeepSeek Harness (dsh) integration.
//!
//! dsh does not read JSON MCP configs like the other IDEs. Its profile is a stack of loader patches and the
//! user layer is `~/.dsh/profiles/desktop/cordis.patch.yml`: a top-level YAML array of entries. An MCP server is
//! an entry that configures `@deepseek-ai/dsh-mcp-client`; its tools then appear as `mcp__rms-memory__<tool>`.
//! A NEW row must be added with an `- insert:` item (a bare `- id: ...` item targets an existing row and the
//! loader warns `patch: entry not found`); this is the shape of dsh's own `templates/mcp/cordis.patch.yml`.
//!
//! The server must be registered as an MCP server. The agent's `bash` tool runs in a `workspace-write` sandbox
//! where `~/.rms-memory` is not writable, so the agent must write memory through the RMS Memory tools and never
//! by calling the CLI from a shell.

use std::path::{Path, PathBuf};

/// Profile layer the user owns, relative to the home directory.
pub const PATCH_REL: &str = ".dsh/profiles/desktop/cordis.patch.yml";
const ENTRY_ID: &str = "mcp-rms-memory";

pub fn patch_path(home: &Path) -> PathBuf {
    home.join(PATCH_REL)
}

/// dsh is considered installed when its desktop profile directory exists.
pub fn is_installed(home: &Path) -> bool {
    patch_path(home).parent().is_some_and(Path::is_dir)
}

fn entry(executable: &str) -> String {
    // A JSON string is a valid YAML double-quoted scalar, which keeps spaces and backslashes in paths safe.
    let command =
        serde_json::to_string(executable).unwrap_or_else(|_| "\"rms-memory\"".to_string());
    format!(
        "- insert:\n    - id: {ENTRY_ID}\n      name: \"@deepseek-ai/dsh-mcp-client\"\n      config:\n        serverName: rms-memory\n        transport: stdio\n        command: {command}\n        args: [\"serve\"]\n        failOnStartupError: false\n"
    )
}

/// Removes the managed entry, leaving every other line untouched. Also removes the bare `- id:` form written by
/// an early development build, which the loader ignores.
pub fn remove(original: &str) -> String {
    let lines: Vec<&str> = original.split_inclusive('\n').collect();
    let mut out = String::with_capacity(original.len());
    let mut i = 0;
    while i < lines.len() {
        let bare = lines[i].trim_end_matches(['\n', '\r']).trim_end();
        let next_bare = lines
            .iter()
            .skip(i + 1)
            .map(|l| l.trim_end_matches(['\n', '\r']).trim())
            .find(|l| !l.is_empty());
        let managed_insert =
            bare == "- insert:" && next_bare == Some(format!("- id: {ENTRY_ID}").as_str());
        let legacy = bare == format!("- id: {ENTRY_ID}");
        if managed_insert || legacy {
            i += 1;
            // The entry is indented under its first line; the first non-indented, non-blank line ends it.
            while i < lines.len() {
                let l = lines[i].trim_end_matches(['\n', '\r']);
                if l.is_empty() || l.starts_with(' ') || l.starts_with('\t') {
                    i += 1;
                } else {
                    break;
                }
            }
            continue;
        }
        out.push_str(lines[i]);
        i += 1;
    }
    out
}

/// Returns the patch layer with the managed entry present and pointing at `executable`.
pub fn inject(original: &str, executable: &str) -> String {
    let mut out = remove(original);
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(&entry(executable));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# Your patch layer\n- id: agent-default-model\n  name: \"@deepseek-ai/dsh-agent-default-model\"\n  config:\n    provider: deepseek-account\n- id: ui-settings\n  name: \"@deepseek-ai/dsh-client-ui-settings\"\n  config:\n    enabled: true\n";

    #[test]
    fn appends_the_entry_and_keeps_existing_ones() {
        let out = inject(SAMPLE, "/usr/local/bin/rms-memory");
        assert!(out.starts_with(SAMPLE));
        assert!(out.contains("- insert:\n    - id: mcp-rms-memory\n"));
        assert!(out.contains("        serverName: rms-memory\n"));
        assert!(out.contains("        command: \"/usr/local/bin/rms-memory\"\n"));
        assert!(out.contains("        args: [\"serve\"]\n"));
    }

    #[test]
    fn injecting_twice_changes_nothing() {
        let once = inject(SAMPLE, "/usr/local/bin/rms-memory");
        assert_eq!(inject(&once, "/usr/local/bin/rms-memory"), once);
    }

    #[test]
    fn a_moved_binary_updates_the_command_in_place() {
        let once = inject(SAMPLE, "/old/rms-memory");
        let moved = inject(&once, "/new/rms-memory");
        assert!(moved.contains("/new/rms-memory"));
        assert!(!moved.contains("/old/rms-memory"));
        assert_eq!(moved.matches("- id: mcp-rms-memory").count(), 1);
    }

    #[test]
    fn remove_restores_the_original_exactly() {
        let installed = inject(SAMPLE, "/usr/local/bin/rms-memory");
        assert_eq!(remove(&installed), SAMPLE);
    }

    #[test]
    fn remove_keeps_entries_that_follow_the_managed_one() {
        let installed = format!(
            "{}{}",
            inject("", "/x/rms-memory"),
            "- id: after\n  name: \"@x/y\"\n"
        );
        let removed = remove(&installed);
        assert_eq!(removed, "- id: after\n  name: \"@x/y\"\n");
    }

    #[test]
    fn paths_with_spaces_and_quotes_stay_valid_yaml() {
        let out = inject("", "/Users/a b/\"q\"/rms-memory");
        let parsed: serde_json::Value = {
            let line = out
                .lines()
                .find(|l| l.trim_start().starts_with("command:"))
                .unwrap();
            let raw = line.trim_start().trim_start_matches("command:").trim();
            serde_json::from_str(raw).expect("double-quoted scalar parses as JSON")
        };
        assert_eq!(parsed, "/Users/a b/\"q\"/rms-memory");
    }

    #[test]
    fn an_empty_file_gets_just_the_entry() {
        assert_eq!(inject("", "/x/rms-memory"), entry("/x/rms-memory"));
    }

    #[test]
    fn detects_the_profile_directory() {
        let home = tempfile::tempdir().unwrap();
        assert!(!is_installed(home.path()));
        std::fs::create_dir_all(home.path().join(".dsh/profiles/desktop")).unwrap();
        assert!(is_installed(home.path()));
    }

    #[test]
    fn the_legacy_bare_entry_is_replaced_by_the_insert_form() {
        let legacy = format!(
            "{SAMPLE}- id: mcp-rms-memory\n  name: \"@deepseek-ai/dsh-mcp-client\"\n  config:\n    serverName: rms-memory\n    transport: stdio\n    command: \"/old/rms-memory\"\n    args: [\"serve\"]\n    failOnStartupError: false\n"
        );
        assert_eq!(remove(&legacy), SAMPLE);
        let fixed = inject(&legacy, "/new/rms-memory");
        assert!(fixed.starts_with(SAMPLE));
        assert!(fixed.contains("- insert:\n    - id: mcp-rms-memory\n"));
        assert!(!fixed.contains("/old/rms-memory"));
        assert_eq!(fixed.matches("mcp-rms-memory").count(), 1);
    }

    #[test]
    fn another_insert_block_is_never_touched() {
        let other = format!("{SAMPLE}- insert:\n    - id: someone-else\n      name: \"@x/y\"\n");
        assert_eq!(remove(&other), other);
    }
}
