use std::fs;
use std::path::Path;
use anyhow::Result;

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
}

pub fn inject_rules(project_root: &Path, options: InjectOptions) -> Result<()> {
    let mut injected = false;

    // 1. Cursor
    let cursor_path = project_root.join(".cursorrules");
    if cursor_path.exists() {
        append_or_replace_block(&cursor_path, CURSOR_RULES, options)?;
        injected = true;
    }

    // 2. Claude Code
    let claude_dir = project_root.join(".claude");
    let claude_path = claude_dir.join("CLAUDE.md");
    if claude_path.exists() {
        append_or_replace_block(&claude_path, CLAUDE_RULES, options)?;
        injected = true;
    } else if claude_dir.exists() {
        create_and_write(&claude_path, CLAUDE_RULES, options)?;
        injected = true;
    } else {
        // Also check if CLAUDE.md is in the root
        let root_claude_path = project_root.join("CLAUDE.md");
        if root_claude_path.exists() {
            append_or_replace_block(&root_claude_path, CLAUDE_RULES, options)?;
            injected = true;
        }
    }

    // 3. Zed
    let zed_dir = project_root.join(".zed");
    let zed_path = zed_dir.join("assistant.md");
    if zed_path.exists() {
        append_or_replace_block(&zed_path, ZED_RULES, options)?;
        injected = true;
    } else if zed_dir.exists() {
        create_and_write(&zed_path, ZED_RULES, options)?;
        injected = true;
    }

    // 4. Fallback if no IDE specific config was found
    if !injected {
        let fallback_path = project_root.join("RMS_MEMORY_GUIDE.md");
        if fallback_path.exists() {
            append_or_replace_block(&fallback_path, GENERAL_RULES, options)?;
        } else {
            create_and_write(&fallback_path, GENERAL_RULES, options)?;
        }
    }

    Ok(())
}

fn create_and_write(file_path: &Path, template: &str, options: InjectOptions) -> Result<()> {
    let display_path = file_path.file_name().unwrap_or_default().to_string_lossy();
    if options.dry_run {
        println!("\n[DRY-RUN] Planning to patch: {}", display_path);
        println!("- Destination: {}", file_path.display());
        println!("- Action: Create new file");
        println!("- Preview:\n  + {}", template.replace('\n', "\n  + "));
        return Ok(());
    }

    fs::write(file_path, template)?;
    Ok(())
}

fn append_or_replace_block(file_path: &Path, template: &str, options: InjectOptions) -> Result<()> {
    let content = fs::read_to_string(file_path)?;
    let display_path = file_path.file_name().unwrap_or_default().to_string_lossy();

    let mut action = "Append new block to EOF";
    let new_content = if let (Some(start_idx), Some(end_idx)) = (
        content.find(START_MARKER),
        content.find(END_MARKER)
    ) {
        if start_idx < end_idx {
            action = "Replace existing RMS-MEMORY block";
            let before = &content[..start_idx];
            let after = &content[end_idx + END_MARKER.len()..];
            format!("{}{}{}", before, template, after)
        } else {
            // Malformed markers, just append
            format!("{}\n\n{}", content, template)
        }
    } else {
        // No markers found, append
        let prefix = if content.ends_with('\n') { "\n" } else { "\n\n" };
        format!("{}{}{}", content, prefix, template)
    };

    if options.dry_run {
        println!("\n[DRY-RUN] Planning to patch: {}", display_path);
        println!("- Destination: {}", file_path.display());
        println!("- Action: {}", action);
        println!("- Preview:\n  + {}", template.replace('\n', "\n  + "));
        return Ok(());
    }

    // Execution: Create backup
    let bak_path = file_path.with_extension(format!("{}.bak", file_path.extension().unwrap_or_default().to_string_lossy()));
    fs::copy(file_path, &bak_path)?;

    fs::write(file_path, new_content)?;
    Ok(())
}
