use std::path::{Path, PathBuf};
use std::fs::{self, File};
use anyhow::{Result, Context};
use dialoguer::{MultiSelect, Confirm, Select, theme::ColorfulTheme};
use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone)]
struct IdeConfig {
    name: &'static str,
    paths: Vec<&'static str>, // relative to home dir
    key: &'static str,
}

impl IdeConfig {
    fn new(name: &'static str, paths: Vec<&'static str>, key: &'static str) -> Self {
        Self { name, paths, key }
    }
}

fn get_ide_registry() -> Vec<IdeConfig> {
    vec![
        IdeConfig::new("Claude Desktop", vec![
            "Library/Application Support/Claude/claude_desktop_config.json",
            ".config/Claude/claude_desktop_config.json", // linux
        ], "mcpServers"),
        IdeConfig::new("Cursor", vec![
            ".cursor/mcp.json",
        ], "mcpServers"),
        IdeConfig::new("Zed", vec![
            ".config/zed/settings.json",
        ], "context_servers"),
        IdeConfig::new("VSCode (Roo Cline)", vec![
            "Library/Application Support/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
            ".config/Code/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
        ], "mcpServers"),
        IdeConfig::new("Antigravity IDE (Roo Cline)", vec![
            "Library/Application Support/Antigravity IDE/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
            ".config/Antigravity IDE/User/globalStorage/rooveterinaryinc.roo-cline/settings/mcp_settings.json",
        ], "mcpServers"),
        IdeConfig::new("Gemini CLI", vec![
            ".gemini/config/mcp_config.json",
            ".gemini/antigravity/mcp_config.json",
            ".gemini/settings.json",
        ], "mcpServers"),
        IdeConfig::new("QwenCode", vec![
            "Library/Application Support/Qwen/settings.json",
            ".config/Qwen/settings.json",
        ], "mcpServers"),
        IdeConfig::new("OpenCode", vec![
            "Library/Application Support/opencode/opencode.json",
            "Library/Application Support/ai.opencode.desktop/settings.json",
            ".config/opencode/opencode.json",
        ], "mcp"),
        IdeConfig::new("ZCode", vec![
            "Library/Application Support/ZCode/settings.json",
            ".config/ZCode/settings.json",
        ], "mcpServers"),
        IdeConfig::new("Nova", vec![
            "Library/Application Support/Nova/settings.json",
            "Library/Application Support/Nova/Workspaces/Metadata.json",
        ], "mcpServers"),
    ]
}

pub async fn run_installer(auto_yes: bool, dry_run: bool) -> Result<()> {
    println!("[🔍] Scanning known configuration directories...");

    let base_dirs = directories::BaseDirs::new().context("Cannot find base directories")?;
    let home = base_dirs.home_dir();
    
    let registry = get_ide_registry();
    let mut valid_targets = Vec::new();
    let mut found_ides = std::collections::HashSet::new();

    for ide in &registry {
        for rel_path in &ide.paths {
            let candidate = home.join(rel_path);
            if candidate.exists() && candidate.is_file() {
                let content = match fs::read_to_string(&candidate) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let json: serde_json::Value = match serde_json::from_str(&content) {
                    Ok(j) => j,
                    Err(_) => {
                        // Create empty json object string if file is empty
                        if content.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            continue;
                        }
                    }
                };
                if json.is_object() {
                    found_ides.insert(ide.name.to_string());
                    valid_targets.push((candidate.clone(), json, ide.clone(), content));
                    break; // Only pick the first valid path per IDE
                }
            } else if candidate.parent().map(|p| p.exists()).unwrap_or(false) {
                // Base directory exists, but config file doesn't. Propose creating it.
                // We'll treat this as a valid target with an empty JSON object.
                found_ides.insert(ide.name.to_string());
                valid_targets.push((candidate.clone(), serde_json::json!({}), ide.clone(), "{}".to_string()));
                break; // Only pick the first path
            }
        }
    }

    if valid_targets.is_empty() {
        println!("[!] No IDE configurations found.");
        return Ok(());
    }

    let mut ides_list: Vec<_> = found_ides.into_iter().collect();
    ides_list.sort();
    println!("[🔍] Found: {}", ides_list.join(", "));

    let mut selected_targets = Vec::new();

    if auto_yes {
        selected_targets = valid_targets;
    } else {
        let choices = &["[1] Connect All", "[2] Select Manually", "[3] Cancel"];
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("[?] Which IDEs would you like to connect?")
            .default(0)
            .items(&choices[..])
            .interact()?;

        match selection {
            0 => {
                selected_targets = valid_targets;
            }
            1 => {
                let items: Vec<String> = valid_targets.iter().map(|(p, _, i, _)| format!("{} ({})", i.name, p.display())).collect();
                let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select configuration files to patch (Space to toggle, Enter to confirm)")
                    .items(&items)
                    .interact()?;
                
                if chosen.is_empty() {
                    println!("Cancelled.");
                    return Ok(());
                }
                for idx in chosen {
                    selected_targets.push(valid_targets[idx].clone());
                }
            }
            _ => {
                println!("Cancelled.");
                return Ok(());
            }
        }
    }

    let my_exe = std::env::current_exe()?;
    let my_exe_str = my_exe.to_string_lossy().to_string();

    for (candidate, mut json, ide, original_content) in selected_targets {
        let config_payload = if ide.name == "OpenCode" {
            serde_json::json!({
                "enabled": true,
                "type": "local",
                "command": [my_exe_str.clone(), "serve"]
            })
        } else {
            serde_json::json!({
                "command": my_exe_str.clone(),
                "args": ["serve"]
            })
        };

        let patched_content = inject_jsonc(&original_content, ide.key, "rms-memory", &config_payload);
        
        if let Some(out) = patched_content {
            if out == original_content {
                println!("[✅] Already configured in {} ({})", ide.name, candidate.display());
                continue;
            }

            if !auto_yes && !dry_run {
                let display_name = format!("{} ({})", ide.name, candidate.file_name().unwrap_or_default().to_string_lossy());
                let show_diff = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!("[!] Found {}. Show diff before writing?", display_name))
                    .default(false)
                    .interact()?;
                
                if show_diff {
                    let diff = TextDiff::from_lines(&original_content, &out);
                    println!("\n--- Diff for {} ---", candidate.display());
                    for change in diff.iter_all_changes() {
                        let sign = match change.tag() {
                            ChangeTag::Delete => "-",
                            ChangeTag::Insert => "+",
                            ChangeTag::Equal => " ",
                        };
                        print!("{}{}", sign, change);
                    }
                    println!("-------------------\n");
                }

                let write_changes = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt("Write changes?")
                    .default(true)
                    .interact()?;
                
                if !write_changes {
                    println!("Skipping {}", candidate.display());
                    continue;
                }
            }

            if dry_run {
                println!("\n[DRY-RUN] Planning to patch: {}", candidate.display());
            } else {
                if candidate.exists() {
                    let backup_path = format!("{}.bak", candidate.to_string_lossy());
                    let _ = fs::copy(&candidate, &backup_path);
                } else if let Some(p) = candidate.parent() {
                    let _ = fs::create_dir_all(p);
                }
                
                if let Err(e) = fs::write(&candidate, out) {
                    eprintln!("[❌] Failed to write to {}: {}", candidate.display(), e);
                } else {
                    println!("[✅] Successfully added to {} ({})", ide.name, candidate.display());
                }
            }
        } else {
            eprintln!("[⚠️] Failed to safely patch {}. It might be malformed or use an unsupported format.", candidate.display());
        }
    }
    
    
    #[cfg(target_os = "macos")]
    {
        println!("[🔒] Applying macOS entitlements to bypass Library Validation (prevents crashes in sandboxed IDEs)...");
        let entitlements = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>com.apple.security.cs.disable-library-validation</key>
    <true/>
</dict>
</plist>"#;
        let entitlements_path = std::env::temp_dir().join("rms_entitlements.plist");
        if let Ok(_) = fs::write(&entitlements_path, entitlements) {
            let status = std::process::Command::new("codesign")
                .args(&["-s", "-", "-f", "--entitlements", entitlements_path.to_str().unwrap(), &my_exe_str])
                .status();
            match status {
                Ok(s) if s.success() => println!("[✅] Successfully signed executable with entitlements."),
                _ => eprintln!("[⚠️] Failed to sign executable. You may experience crashes in Claude Desktop. Try running: codesign -s - -f --entitlements path/to/entitlements.plist {}", my_exe_str),
            }
            let _ = fs::remove_file(entitlements_path);
        }
    }

    println!("[✅] Installation sweep completed.");
    Ok(())
}


fn strip_json_comments(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut in_string = false;
    let mut in_comment = false;
    let mut in_multiline_comment = false;
    let mut chars = json.chars().peekable();
    
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(next_c) = chars.next() {
                    out.push(next_c);
                }
            } else if c == '"' {
                in_string = false;
            }
            continue;
        }
        if in_comment {
            if c == '\n' {
                in_comment = false;
                out.push(c);
            } else {
                out.push(' ');
            }
            continue;
        }
        if in_multiline_comment {
            if c == '*' {
                if let Some(&'/') = chars.peek() {
                    chars.next();
                    in_multiline_comment = false;
                    out.push_str("  ");
                } else {
                    out.push(' ');
                }
            } else if c == '\n' {
                out.push('\n');
            } else {
                out.push(' ');
            }
            continue;
        }
        if c == '/' {
            if let Some(&'/') = chars.peek() {
                chars.next();
                in_comment = true;
                out.push_str("  ");
                continue;
            } else if let Some(&'*') = chars.peek() {
                chars.next();
                in_multiline_comment = true;
                out.push_str("  ");
                continue;
            }
        }
        if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

fn inject_jsonc(original: &str, key: &str, tool_name: &str, tool_config: &serde_json::Value) -> Option<String> {
    if original.trim().is_empty() || original.trim() == "{}" {
        let tool_config_str = serde_json::to_string_pretty(tool_config).unwrap().replace("\n", "\n      ");
        let injection = format!("\"{}\": {}", tool_name, tool_config_str);
        return Some(format!("{{\n  \"{}\": {{\n    {}\n  }}\n}}", key, injection.replace("      ", "    ")));
    }

    let stripped = strip_json_comments(original);
    let mut json = serde_json::from_str::<serde_json::Value>(&stripped).ok()?;
    
    let obj = json.as_object_mut()?;
    if let Some(mcp) = obj.get(key) {
        if let Some(mcp_obj) = mcp.as_object() {
            if mcp_obj.contains_key(tool_name) {
                // Already exists — replace the existing block in-place
                // Find "rms-memory": { ... } in the original text and replace it
                let entry_pattern = format!(
                    r#""{}"\s*:\s*\{{[^{{}}]*\}}"#,
                    regex::escape(tool_name)
                );
                if let Ok(re) = regex::Regex::new(&entry_pattern) {
                    if let Some(mat) = re.find(original) {
                        // Detect indentation from the matched block
                        let before_match = &original[..mat.start()];
                        let indent = before_match
                            .rfind('\n')
                            .map(|nl| {
                                let line_start = nl + 1;
                                let spaces: String = before_match[line_start..]
                                    .chars()
                                    .take_while(|c| c.is_whitespace())
                                    .collect();
                                spaces
                            })
                            .unwrap_or_else(|| "    ".to_string());
                        let inner_indent = format!("{}  ", indent);
                        
                        let new_config_str = serde_json::to_string_pretty(tool_config).unwrap();
                        let new_config_indented = new_config_str.replace("\n", &format!("\n{}", inner_indent));
                        let replacement = format!("\"{}\": {}", tool_name, new_config_indented);
                        
                        let mut patched = original.to_string();
                        patched.replace_range(mat.range(), &replacement);
                        return Some(patched);
                    }
                }
                // Regex didn't match (nested braces?) — skip to avoid corruption
                return Some(original.to_string());
            }
        }
    }
    
    let tool_config_str = serde_json::to_string_pretty(tool_config).unwrap();
    // indent it
    let tool_config_str = tool_config_str.replace("\n", "\n      ");
    let injection = format!("\"{}\": {}", tool_name, tool_config_str);
    
    if obj.contains_key(key) {
        // Simple regex to find "key": {
        let pattern = format!(r#"("{}"\s*:\s*\{{)"#, key);
        let re = regex::Regex::new(&pattern).unwrap();
        if let Some(mat) = re.find(original) {
            let mut patched = original.to_string();
            // check if the dictionary is empty
            let after_brace = &original[mat.end()..];
            let just_whitespace_then_close = after_brace.trim_start().starts_with("}");
            if just_whitespace_then_close {
                patched.insert_str(mat.end(), &format!("\n      {}\n    ", injection));
            } else {
                patched.insert_str(mat.end(), &format!("\n      {},", injection));
            }
            return Some(patched);
        }
    } else {
        // Insert right before the last closing brace
        if let Some(last_brace) = original.rfind('}') {
            let mut patched = original[..last_brace].to_string();
            let trimmed = patched.trim_end();
            patched.truncate(trimmed.len());
            let needs_comma = !patched.ends_with(',') && !patched.ends_with('{');
            if needs_comma {
                patched.push(',');
            }
            patched.push_str(&format!("\n  \"{}\": {{\n    {}\n  }}\n}}", key, injection.replace("      ", "    ")));
            return Some(patched);
        }
    }
    
    None
}
