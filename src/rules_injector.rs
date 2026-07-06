use anyhow::Result;
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
        if file_path.exists() {
            existing_count += 1;
            files_to_inject.push((file_path_str, template, true));
        }
    }

    // If no rule files exist, just create AGENT.md as a fallback
    if existing_count == 0 {
        files_to_inject.push(("AGENT.md", GENERAL_RULES, false));
    }

    let mut injected_files = Vec::new();

    for (file_path_str, template, exists) in files_to_inject {
        let file_path = project_root.join(file_path_str);

        // Ensure parent directories exist
        if let Some(parent) = file_path.parent()
            && !parent.exists()
        {
            let _ = std::fs::create_dir_all(parent);
        }

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
        fs::read_to_string(&gitignore_path).unwrap_or_default()
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

    let mut action = "Append new block to EOF";
    let new_content = if let (Some(start_idx), Some(end_idx)) =
        (content.find(START_MARKER), content.find(END_MARKER))
    {
        if start_idx < end_idx {
            action = "Replace existing RMS-MEMORY block";
            let before = &content[..start_idx];
            let after = &content[end_idx + END_MARKER.len()..];
            format!("{}{}{}", before, template, after)
        } else {
            format!("{}\n\n{}", content, template)
        }
    } else {
        let prefix = if content.ends_with('\n') {
            "\n"
        } else {
            "\n\n"
        };
        format!("{}{}{}", content, prefix, template)
    };

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
