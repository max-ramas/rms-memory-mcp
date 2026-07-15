use anyhow::{Result, bail};
use std::fs;
use std::path::Path;

const CURSOR_RULES: &str = include_str!("../templates/cursor_rules.md");
const CLAUDE_RULES: &str = include_str!("../templates/claude_code_rules.md");
const ZED_RULES: &str = include_str!("../templates/zed_assistant_rules.md");
const GENERAL_RULES: &str = include_str!("../templates/general_mcp_guide.md");

const START_MARKER: &str = "<!-- RMS-MEMORY-START -->";
const END_MARKER: &str = "<!-- RMS-MEMORY-END -->";

#[derive(Default, Clone, Copy)]
pub struct InjectOptions {
    pub dry_run: bool,
    pub force: bool,
    pub full: bool,
    pub interactive: bool,
}

pub fn inject_rules(project_root: &Path, options: InjectOptions) -> Result<()> {
    // List of files to inject rules into
    let target_files = vec![
        (".cursorrules", CURSOR_RULES),
        (".windsurfrules", CURSOR_RULES),
        (".clinerules", CURSOR_RULES),
        (".rules", GENERAL_RULES),
        (".github/copilot-instructions.md", CURSOR_RULES),
        ("AGENT.md", GENERAL_RULES),
        ("AGENTS.md", GENERAL_RULES),
        ("CLAUDE.md", CLAUDE_RULES),
        ("GEMINI.md", GENERAL_RULES),
        (".zed/assistant.md", ZED_RULES),
    ];

    let mut existing_count = 0;
    let mut files_to_inject = Vec::new();

    for (file_path_str, template) in target_files {
        let file_path = project_root.join(file_path_str);
        let exists = file_path.exists();

        if exists {
            existing_count += 1;
            files_to_inject.push((file_path_str, template, true));
        } else if options.full {
            files_to_inject.push((file_path_str, template, false));
        }
    }

    // If no rule files exist and not in full mode, just create AGENT.md as a fallback
    if existing_count == 0 && !options.full {
        files_to_inject.push(("AGENT.md", GENERAL_RULES, false));
    }

    let mut injected_files = Vec::new();

    for (file_path_str, template, exists) in files_to_inject {
        let file_path = project_root.join(file_path_str);

        if exists {
            if append_or_replace_block(&file_path, template, options)? {
                injected_files.push(file_path_str.to_string());
            }
        } else {
            if create_and_write(&file_path, template, options)? {
                injected_files.push(file_path_str.to_string());
            }
        }
    }

    if !injected_files.is_empty() {
        append_to_gitignore(project_root, &injected_files)?;
    }

    Ok(())
}

fn append_to_gitignore(project_root: &Path, files: &[String]) -> Result<()> {
    let gitignore_path = project_root.join(".gitignore");
    let mut content = if gitignore_path.exists() {
        fs::read_to_string(&gitignore_path).unwrap_or_else(|e| {
            tracing::warn!("Cannot read existing .gitignore ({}), starting fresh", e);
            String::new()
        })
    } else {
        String::new()
    };

    let mut needs_update = false;
    let mut to_append = String::new();

    for file in files {
        let line = format!("/{}", file);
        if !content
            .lines()
            .any(|l| l.trim() == line || l.trim() == file)
        {
            to_append.push_str(&line);
            to_append.push('\n');
            needs_update = true;
        }
    }

    if needs_update {
        if !content.is_empty() && !content.ends_with('\n') {
            content.push('\n');
        }
        content.push_str("\n# RMS Memory Agent Rules\n");
        content.push_str(&to_append);
        fs::write(&gitignore_path, content)?;
    }

    Ok(())
}

fn create_and_write(file_path: &Path, template: &str, options: InjectOptions) -> Result<bool> {
    let display_path = file_path.file_name().unwrap_or_default().to_string_lossy();
    if options.dry_run {
        println!("\n[DRY-RUN] Planning to patch: {}", display_path);
        println!("- Destination: {}", file_path.display());
        println!("- Action: Create new file");
        println!("- Preview:\n  + {}", template.replace('\n', "\n  + "));
        return Ok(false);
    }

    if options.interactive && !options.force {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Create new file {} with RMS-Memory rules?",
                display_path
            ))
            .default(true)
            .interact()?;
        if !confirm {
            println!("Skipping {}", display_path);
            return Ok(false);
        }
    }

    if let Some(parent) = file_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(file_path, template)?;
    Ok(true)
}

fn append_or_replace_block(
    file_path: &Path,
    template: &str,
    options: InjectOptions,
) -> Result<bool> {
    let content = fs::read_to_string(file_path)?;
    let display_path = file_path.file_name().unwrap_or_default().to_string_lossy();

    let (new_content, action) = replace_managed_block(&content, template)?;

    if new_content == content {
        return Ok(false);
    }

    if options.dry_run {
        println!("\n[DRY-RUN] Planning to patch: {}", display_path);
        println!("- Destination: {}", file_path.display());
        println!("- Action: {}", action);
        return Ok(false);
    }

    if options.interactive && !options.force {
        let show_diff = dialoguer::Confirm::new()
            .with_prompt(format!(
                "[!] Found {}. Show diff before writing?",
                display_path
            ))
            .default(false)
            .interact()?;

        if show_diff {
            let diff = similar::TextDiff::from_lines(&content, &new_content);
            println!("\n--- Diff for {} ---", file_path.display());
            for change in diff.iter_all_changes() {
                let sign = match change.tag() {
                    similar::ChangeTag::Delete => "-",
                    similar::ChangeTag::Insert => "+",
                    similar::ChangeTag::Equal => " ",
                };
                print!("{}{}", sign, change);
            }
            println!("-------------------\n");
        }

        let write_changes = dialoguer::Confirm::new()
            .with_prompt("Write changes?")
            .default(true)
            .interact()?;

        if !write_changes {
            println!("Skipping {}", display_path);
            return Ok(false);
        }
    }

    fs::write(file_path, new_content)?;
    Ok(true)
}

/// Replaces the RMS-managed block without changing bytes outside that block.
///
/// The inclusive range includes exactly one newline after the end marker.  The
/// templates own that newline, which makes a second replacement byte-identical
/// instead of appending an extra blank line on every invocation.
fn replace_managed_block(content: &str, template: &str) -> Result<(String, &'static str)> {
    let starts = marker_positions(content, START_MARKER);
    let ends = marker_positions(content, END_MARKER);

    match (starts.as_slice(), ends.as_slice()) {
        ([], []) => {
            let line_ending = preferred_line_ending(content);
            let normalized_template = normalize_line_endings(template, line_ending);
            let separator = if content.is_empty() {
                String::new()
            } else if ends_with_line_ending(content) {
                line_ending.to_string()
            } else {
                // Keep the previous behaviour of a blank separator before a new
                // managed block, while respecting CRLF files.
                format!("{line_ending}{line_ending}")
            };
            Ok((
                format!("{content}{separator}{normalized_template}"),
                "Append new block to EOF",
            ))
        }
        ([start], [end]) if start < end => {
            let line_ending = line_ending_after_marker(content, *end + END_MARKER.len())
                .unwrap_or_else(|| preferred_line_ending(content));
            let normalized_template = normalize_line_endings(template, line_ending);
            let end_of_block =
                *end + END_MARKER.len() + line_ending_len_at(content, *end + END_MARKER.len());
            Ok((
                format!(
                    "{}{}{}",
                    &content[..*start],
                    normalized_template,
                    &content[end_of_block..]
                ),
                "Replace existing RMS-MEMORY block",
            ))
        }
        ([start], [end]) if end < start => bail!(
            "Malformed RMS-MEMORY markers: end marker appears before start marker; refusing to modify file"
        ),
        _ => bail!(
            "Malformed RMS-MEMORY markers: expected zero markers or one ordered start/end pair (found {} start, {} end); refusing to modify file",
            starts.len(),
            ends.len()
        ),
    }
}

fn marker_positions(content: &str, marker: &str) -> Vec<usize> {
    content
        .match_indices(marker)
        .map(|(index, _)| index)
        .collect()
}

fn preferred_line_ending(content: &str) -> &'static str {
    if content.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

fn line_ending_after_marker(content: &str, marker_end: usize) -> Option<&'static str> {
    match content.get(marker_end..) {
        Some(rest) if rest.starts_with("\r\n") => Some("\r\n"),
        Some(rest) if rest.starts_with('\n') => Some("\n"),
        _ => None,
    }
}

fn line_ending_len_at(content: &str, index: usize) -> usize {
    line_ending_after_marker(content, index).map_or(0, str::len)
}

fn ends_with_line_ending(content: &str) -> bool {
    content.ends_with('\n') || content.ends_with('\r')
}

fn normalize_line_endings(template: &str, line_ending: &str) -> String {
    template
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .replace('\n', line_ending)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    const TEMPLATE: &str = "<!-- RMS-MEMORY-START -->\nmanaged\n<!-- RMS-MEMORY-END -->\n";

    fn options(dry_run: bool) -> InjectOptions {
        InjectOptions {
            dry_run,
            force: true,
            full: false,
            interactive: false,
        }
    }

    #[test]
    fn replacement_is_byte_idempotent_for_one_hundred_runs() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("AGENTS.md");
        fs::write(&path, "user heading\n\nuser footer\n").unwrap();

        assert!(append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
        let expected = fs::read(&path).unwrap();
        for _ in 0..99 {
            assert!(!append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
            assert_eq!(fs::read(&path).unwrap(), expected);
        }
    }

    #[test]
    fn all_supported_rule_files_are_stable_after_the_first_injection() {
        let directory = tempdir().unwrap();
        let files = [
            ".cursorrules",
            ".windsurfrules",
            ".clinerules",
            ".rules",
            ".github/copilot-instructions.md",
            "AGENT.md",
            "AGENTS.md",
            "CLAUDE.md",
            "GEMINI.md",
            ".zed/assistant.md",
        ];
        for file in files {
            let path = directory.path().join(file);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, "user content\n").unwrap();
        }

        inject_rules(directory.path(), options(false)).unwrap();
        let snapshots: Vec<_> = files
            .iter()
            .map(|file| fs::read(directory.path().join(file)).unwrap())
            .collect();
        let gitignore = fs::read(directory.path().join(".gitignore")).unwrap();

        for _ in 0..99 {
            inject_rules(directory.path(), options(false)).unwrap();
            for (file, expected) in files.iter().zip(&snapshots) {
                assert_eq!(fs::read(directory.path().join(file)).unwrap(), *expected);
            }
            assert_eq!(
                fs::read(directory.path().join(".gitignore")).unwrap(),
                gitignore
            );
        }
    }

    #[test]
    fn replacement_preserves_crlf_and_bytes_outside_managed_block() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("AGENTS.md");
        fs::write(
            &path,
            b"before\r\n<!-- RMS-MEMORY-START -->\r\nold\r\n<!-- RMS-MEMORY-END -->\r\nafter\r\n",
        )
        .unwrap();

        assert!(append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
        assert_eq!(
            fs::read(&path).unwrap(),
            b"before\r\n<!-- RMS-MEMORY-START -->\r\nmanaged\r\n<!-- RMS-MEMORY-END -->\r\nafter\r\n"
        );
        assert!(!append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
    }

    #[test]
    fn replacement_keeps_user_owned_extra_blank_lines_after_block() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("AGENTS.md");
        fs::write(
            &path,
            "<!-- RMS-MEMORY-START -->\nold\n<!-- RMS-MEMORY-END -->\n\n\nuser content\n",
        )
        .unwrap();

        assert!(append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "<!-- RMS-MEMORY-START -->\nmanaged\n<!-- RMS-MEMORY-END -->\n\n\nuser content\n"
        );
    }

    #[test]
    fn replacement_handles_a_block_without_a_final_newline() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("AGENTS.md");
        fs::write(
            &path,
            "before\n<!-- RMS-MEMORY-START -->\nold\n<!-- RMS-MEMORY-END -->",
        )
        .unwrap();

        assert!(append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "before\n<!-- RMS-MEMORY-START -->\nmanaged\n<!-- RMS-MEMORY-END -->\n"
        );
        assert!(!append_or_replace_block(&path, TEMPLATE, options(false)).unwrap());
    }

    #[test]
    fn malformed_or_duplicate_markers_are_non_destructive() {
        for content in [
            "<!-- RMS-MEMORY-END -->\n<!-- RMS-MEMORY-START -->\n",
            "<!-- RMS-MEMORY-START -->\n<!-- RMS-MEMORY-START -->\n<!-- RMS-MEMORY-END -->\n",
            "<!-- RMS-MEMORY-START -->\nno end\n",
        ] {
            let original = content.to_string();
            assert!(replace_managed_block(&original, TEMPLATE).is_err());
            assert_eq!(original, content);
        }
    }

    #[test]
    fn dry_run_does_not_write_files_or_create_parent_directories() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("nested").join("AGENTS.md");
        assert!(!create_and_write(&path, TEMPLATE, options(true)).unwrap());
        assert!(!path.exists());
        assert!(!path.parent().unwrap().exists());

        fs::write(directory.path().join("existing.md"), "user\n").unwrap();
        let existing = directory.path().join("existing.md");
        let before = fs::read(&existing).unwrap();
        assert!(!append_or_replace_block(&existing, TEMPLATE, options(true)).unwrap());
        assert_eq!(fs::read(&existing).unwrap(), before);
    }
}
