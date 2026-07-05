use std::path::{Path, PathBuf};
use std::fs::{self, File};
use anyhow::{Result, Context};
use walkdir::WalkDir;
use dialoguer::{MultiSelect, Confirm, Select, theme::ColorfulTheme};
use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, PartialEq)]
enum IdeType {
    Cursor,
    Zed,
    ClaudeCode,
    Unknown(String),
}

impl IdeType {
    fn from_path(path: &Path) -> Self {
        let p = path.to_string_lossy().to_lowercase();
        if p.contains("cursor") {
            IdeType::Cursor
        } else if p.contains("zed") {
            IdeType::Zed
        } else if p.contains("claude") {
            IdeType::ClaudeCode
        } else {
            IdeType::Unknown("Other".to_string())
        }
    }

    fn name(&self) -> &str {
        match self {
            IdeType::Cursor => "Cursor",
            IdeType::Zed => "Zed",
            IdeType::ClaudeCode => "Claude Desktop/Code",
            IdeType::Unknown(name) => name,
        }
    }
}

pub async fn run_installer(auto_yes: bool, dry_run: bool) -> Result<()> {
    println!("[🔍] Scanning configuration directories...");

    let base_dirs = directories::BaseDirs::new().context("Cannot find base directories")?;
    let mut search_dirs = Vec::new();
    
    search_dirs.push(base_dirs.config_dir().to_path_buf());
    search_dirs.push(base_dirs.data_dir().to_path_buf());
    search_dirs.push(base_dirs.data_local_dir().to_path_buf());

    let home = base_dirs.home_dir();
    let claude_dir = home.join(".claude");
    if claude_dir.exists() { search_dirs.push(claude_dir); }
    let cursor_dir = home.join(".cursor");
    if cursor_dir.exists() { search_dirs.push(cursor_dir); }
    
    let mut candidates = Vec::new();
    
    for base_dir in search_dirs {
        if !base_dir.exists() { continue; }
        for entry in WalkDir::new(&base_dir).max_depth(4).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() { continue; }
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            if file_name == "mcp.json" || file_name == "settings.json" || file_name.ends_with("mcp.json") || file_name == "config.json" || file_name == "claude_desktop_config.json" {
                candidates.push(path.to_path_buf());
            }
        }
    }
    
    let mut valid_targets = Vec::new();
    let mut found_ides = std::collections::HashSet::new();

    for candidate in candidates {
        let content = match fs::read_to_string(&candidate) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(j) => j,
            Err(_) => continue, 
        };
        if json.is_object() {
            let ide_type = IdeType::from_path(&candidate);
            found_ides.insert(ide_type.name().to_string());
            valid_targets.push((candidate, json, ide_type, content));
        }
    }

    if valid_targets.is_empty() {
        println!("[!] No IDE configurations found.");
        return Ok(());
    }

    let ides_list = found_ides.into_iter().collect::<Vec<_>>().join(", ");
    println!("[🔍] Found: {}", ides_list);

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
                let items: Vec<String> = valid_targets.iter().map(|(p, _, i, _)| format!("{} ({})", i.name(), p.file_name().unwrap_or_default().to_string_lossy())).collect();
                let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select configuration files to patch")
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

    for (candidate, mut json, _ide_type, original_content) in selected_targets {
        let is_mcp_file = candidate.file_name().map(|n| n.to_string_lossy().contains("mcp")).unwrap_or(false);
        let path_str = candidate.to_string_lossy().to_lowercase();
        let mut patched = false;
        
        if let Some(obj) = json.as_object_mut() {
            if obj.contains_key("mcpServers") {
                if let Some(servers) = obj.get_mut("mcpServers").and_then(|v| v.as_object_mut()) {
                    servers.insert("rms-memory".to_string(), serde_json::json!({
                        "command": my_exe_str,
                        "args": ["serve"]
                    }));
                    patched = true;
                }
            } else if obj.contains_key("context_servers") {
                if let Some(servers) = obj.get_mut("context_servers").and_then(|v| v.as_object_mut()) {
                    servers.insert("rms-memory".to_string(), serde_json::json!({
                        "command": my_exe_str,
                        "args": ["serve"]
                    }));
                    patched = true;
                }
            } else {
                if path_str.contains("zed") {
                    obj.insert("context_servers".to_string(), serde_json::json!({
                        "rms-memory": {
                            "command": my_exe_str,
                            "args": ["serve"]
                        }
                    }));
                    patched = true;
                } else if is_mcp_file && !path_str.contains("settings") {
                    obj.insert("mcpServers".to_string(), serde_json::json!({
                        "rms-memory": {
                            "command": my_exe_str,
                            "args": ["serve"]
                        }
                    }));
                    patched = true;
                } else {
                    obj.insert("mcpServers".to_string(), serde_json::json!({
                        "rms-memory": {
                            "command": my_exe_str,
                            "args": ["serve"]
                        }
                    }));
                    patched = true;
                }
            }
        }
        
        if patched {
            let out = serde_json::to_string_pretty(&json)?;
            if out == original_content {
                println!("[✅] Already configured in {}", candidate.display());
                continue;
            }

            if !auto_yes && !dry_run {
                let show_diff = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!("[!] Found {}. Show diff before writing?", candidate.file_name().unwrap_or_default().to_string_lossy()))
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
                let backup_path = format!("{}.bak", candidate.to_string_lossy());
                let _ = fs::copy(&candidate, &backup_path);
                
                if let Err(e) = fs::write(&candidate, out) {
                    eprintln!("[❌] Failed to write to {}: {}", candidate.display(), e);
                } else {
                    println!("[✅] Successfully added to {}", candidate.display());
                }
            }
        }
    }
    
    println!("[✅] Installation sweep completed.");
    Ok(())
}
