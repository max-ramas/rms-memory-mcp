use std::path::{Path, PathBuf};
use std::fs::{self, File};
use anyhow::{Result, Context};
use std::io::{self, Write};
use walkdir::WalkDir;

pub async fn run_installer(mut auto_yes: bool, dry_run: bool) -> Result<()> {
    let base_dirs = directories::BaseDirs::new().context("Cannot find base directories")?;
    let mut search_dirs = Vec::new();
    
    search_dirs.push(base_dirs.config_dir().to_path_buf());
    search_dirs.push(base_dirs.data_dir().to_path_buf());
    
    search_dirs.push(base_dirs.data_local_dir().to_path_buf());

    let home = base_dirs.home_dir();
    
    // Add additional critical root folders for edge cases
    let claude_dir = home.join(".claude");
    if claude_dir.exists() { search_dirs.push(claude_dir); }
    let cursor_dir = home.join(".cursor");
    if cursor_dir.exists() { search_dirs.push(cursor_dir); }
    
    let mut candidates = Vec::new();
    
    println!("Scanning configuration directories...");
    
    for base_dir in search_dirs {
        if !base_dir.exists() { continue; }
        
        for entry in WalkDir::new(&base_dir).max_depth(4).into_iter().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() { continue; }
            
            let file_name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
            // Match typical MCP json files or specific IDE settings files
            if file_name == "mcp.json" || file_name == "settings.json" || file_name.ends_with("mcp.json") || file_name == "config.json" || file_name == "LSP.sublime-settings" {
                candidates.push(path.to_path_buf());
            }
        }
    }
    
    let my_exe = std::env::current_exe()?;
    let my_exe_str = my_exe.to_string_lossy().to_string();
    
    let mut valid_targets = Vec::new();

    // Verify candidates are actually JSON config files
    for candidate in candidates {
        let content = match fs::read_to_string(&candidate) {
            Ok(c) => c,
            Err(_) => continue,
        };
        
        let json: serde_json::Value = match serde_json::from_str(&content) {
            Ok(j) => j,
            Err(_) => continue, // Not valid json
        };
        
        if json.is_object() {
            valid_targets.push((candidate, json));
        }
    }

    println!("Found {} candidate configuration files.", valid_targets.len());
    
    for (candidate, mut json) in valid_targets {
        let is_mcp_file = candidate.file_name().map(|n| n.to_string_lossy().contains("mcp")).unwrap_or(false);
        let path_str = candidate.to_string_lossy().to_lowercase();
        
        println!("--------------------------------------------------");
        println!("Found config: {}", candidate.display());
        
        if !auto_yes {
            print!("Add RMS Memory to this config? [y/N/all]: ");
            io::stdout().flush()?;
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let input = input.trim().to_lowercase();
            if input == "all" {
                auto_yes = true;
            } else if input != "y" && input != "yes" {
                println!("Skipping.");
                continue;
            }
        }
        
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
            match serde_json::to_string_pretty(&json) {
                Ok(out) => {
                    if dry_run {
                        println!("\n[DRY-RUN] Planning to patch: {}", candidate.display());
                        println!("- Destination: {}", candidate.display());
                        println!("- Action: Inject RMS Memory server configuration");
                        println!("- Preview:\n  + {}", out.replace('\n', "\n  + "));
                    } else {
                        let backup_path = format!("{}.bak", candidate.to_string_lossy());
                        if let Err(e) = fs::copy(&candidate, &backup_path) {
                            eprintln!("Failed to create backup for {:?}: {}", candidate, e);
                        } else {
                            println!("Created backup: {}", backup_path);
                        }
                        
                        if let Err(e) = fs::write(&candidate, out) {
                            eprintln!("Failed to write patched config to {:?}: {}", candidate, e);
                        } else {
                            println!("Successfully patched {}", candidate.display());
                        }
                    }
                }
                Err(e) => eprintln!("Failed to serialize json for {:?}: {}", candidate, e),
            }
        }
    }
    
    println!("--------------------------------------------------");
    println!("Installation sweep completed.");
    Ok(())
}
