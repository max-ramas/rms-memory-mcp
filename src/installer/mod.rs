pub mod macos;
pub mod patcher;
pub mod registry;

use anyhow::{Context, Result};
use dialoguer::{Confirm, MultiSelect, Select, theme::ColorfulTheme};
use similar::{ChangeTag, TextDiff};
use std::fs::{self};
use std::path::{Path, PathBuf};

/// Install the currently running standalone CLI into supported IDEs.
///
/// This compatibility entrypoint is for `rms-memory install` only.  Library
/// consumers embedded in another executable (notably the Tauri GUI) must call
/// [`run_installer_with_executable`] with the packaged standalone binary.
pub async fn run_installer(auto_yes: bool, dry_run: bool) -> Result<()> {
    let executable = current_cli_executable()?;
    run_installer_with_executable(auto_yes, dry_run, executable).await
}

/// Install an explicitly supplied standalone `rms-memory` executable.
///
/// Accepting the executable as an argument prevents a host application from
/// accidentally registering itself as the MCP server command.  The path is
/// validated before any IDE configuration is inspected or written.
pub async fn run_installer_with_executable(
    auto_yes: bool,
    dry_run: bool,
    executable: impl AsRef<Path>,
) -> Result<()> {
    let executable = validate_standalone_executable(executable.as_ref())?;
    println!("[🔍] Scanning known configuration directories...");

    let base_dirs = directories::BaseDirs::new().context("Cannot find base directories")?;
    let home = base_dirs.home_dir();

    let registry = registry::get_ide_registry();
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
                        // Try JSONC stripping for configs with comments (e.g. Zed)
                        let stripped = patcher::strip_json_comments(&content);
                        if stripped.trim().is_empty() {
                            serde_json::json!({})
                        } else {
                            match serde_json::from_str(&stripped) {
                                Ok(j) => j,
                                Err(e) => {
                                    tracing::warn!(
                                        "[⚠️] Failed to parse config {}: {}. The file may use an unsupported format.",
                                        candidate.display(),
                                        e
                                    );
                                    continue;
                                }
                            }
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
                valid_targets.push((
                    candidate.clone(),
                    serde_json::json!({}),
                    ide.clone(),
                    "{}".to_string(),
                ));
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
                let items: Vec<String> = valid_targets
                    .iter()
                    .map(|(p, _, i, _)| format!("{} ({})", i.name, p.display()))
                    .collect();
                let chosen = MultiSelect::with_theme(&ColorfulTheme::default())
                    .with_prompt(
                        "Select configuration files to patch (Space to toggle, Enter to confirm)",
                    )
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

    let executable_str = executable.to_string_lossy().to_string();

    for (candidate, _json, ide, original_content) in selected_targets {
        let config_payload = (ide.build_payload)(&executable_str);

        let patched_content = if candidate.to_string_lossy().ends_with(".toml") {
            patcher::inject_toml(&original_content, "rms-memory", &config_payload)
        } else {
            patcher::inject_jsonc(&original_content, ide.key, "rms-memory", &config_payload)
        };

        if let Some(out) = patched_content {
            if out == original_content {
                println!(
                    "[✅] Already configured in {} ({})",
                    ide.name,
                    candidate.display()
                );
                continue;
            }

            if !auto_yes && !dry_run {
                let display_name = format!(
                    "{} ({})",
                    ide.name,
                    candidate.file_name().unwrap_or_default().to_string_lossy()
                );
                let show_diff = Confirm::with_theme(&ColorfulTheme::default())
                    .with_prompt(format!(
                        "[!] Found {}. Show diff before writing?",
                        display_name
                    ))
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
                    println!(
                        "[✅] Successfully added to {} ({})",
                        ide.name,
                        candidate.display()
                    );
                }
            }
        } else {
            eprintln!(
                "[⚠️] Failed to safely patch {}. It might be malformed or use an unsupported format.",
                candidate.display()
            );
        }
    }

    println!("[✅] Installation sweep completed.");
    Ok(())
}

/// Validate the binary that will be persisted in an IDE MCP configuration.
///
/// We deliberately require the product binary name instead of accepting the
/// caller's `current_exe`: doing otherwise records a GUI process followed by
/// `serve`, which is not an independent stdio MCP server.
pub fn validate_standalone_executable(path: &Path) -> Result<PathBuf> {
    let canonical = std::fs::canonicalize(path).with_context(|| {
        format!(
            "Standalone rms-memory executable not found: {}",
            path.display()
        )
    })?;
    if !canonical.is_file() {
        anyhow::bail!(
            "Standalone rms-memory executable is not a file: {}",
            canonical.display()
        );
    }

    let file_name = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .trim_end_matches(".exe");
    if file_name != "rms-memory" {
        anyhow::bail!(
            "Expected standalone executable named rms-memory, got {}. \
             Use run_installer_with_executable with the packaged rms-memory binary.",
            canonical.display()
        );
    }
    Ok(canonical)
}

fn current_cli_executable() -> Result<PathBuf> {
    let current = std::env::current_exe().context("Cannot resolve current executable")?;
    validate_standalone_executable(&current).with_context(|| {
        "This installer was invoked from a host application. Pass the standalone rms-memory binary explicitly instead"
    })
}

pub async fn run_uninstaller(auto_yes: bool, dry_run: bool) -> Result<()> {
    println!("[🗑️] Scanning for rms-memory installations...");

    let base_dirs = directories::BaseDirs::new().context("Cannot find base directories")?;
    let home = base_dirs.home_dir();

    let registry = registry::get_ide_registry();
    let mut uninstalled = 0u32;

    for ide in &registry {
        for rel_path in &ide.paths {
            let candidate = home.join(rel_path);
            if candidate.exists() && candidate.is_file() {
                let content = match fs::read_to_string(&candidate) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                if let Some(removed) = patcher::remove_key(&content, ide.key, "rms-memory")
                    && removed != content
                {
                    if dry_run {
                        println!(
                            "[DRY-RUN] Planning to remove rms-memory from {} ({})",
                            ide.name,
                            candidate.display()
                        );
                        uninstalled += 1;
                        continue;
                    }
                    if !auto_yes {
                        let should_remove = Confirm::with_theme(&ColorfulTheme::default())
                            .with_prompt(format!(
                                "Remove rms-memory from {} ({})?",
                                ide.name,
                                candidate.display()
                            ))
                            .default(true)
                            .interact()?;
                        if !should_remove {
                            println!("Skipping {}", candidate.display());
                            continue;
                        }
                    }
                    if candidate.exists() {
                        let bak = format!("{}.bak", candidate.to_string_lossy());
                        let _ = fs::copy(&candidate, &bak);
                    }
                    fs::write(&candidate, &removed)?;
                    println!("[🗑️] Removed from {} ({})", ide.name, candidate.display());
                    uninstalled += 1;
                }
            }
        }
    }

    if uninstalled == 0 {
        println!("[!] No rms-memory installations found.");
    } else {
        println!(
            "[✅] Removed rms-memory from {} IDE configuration(s).",
            uninstalled
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_the_standalone_product_binary() {
        let directory = tempfile::tempdir().unwrap();
        let binary = directory.path().join("rms-memory");
        std::fs::write(&binary, b"fixture").unwrap();

        assert_eq!(
            validate_standalone_executable(&binary).unwrap(),
            binary.canonicalize().unwrap()
        );

        let gui = directory.path().join("RMS Memory");
        std::fs::write(&gui, b"fixture").unwrap();
        let error = validate_standalone_executable(&gui)
            .unwrap_err()
            .to_string();
        assert!(error.contains("Expected standalone executable named rms-memory"));
    }

    #[test]
    fn rejects_missing_or_directory_executables() {
        let directory = tempfile::tempdir().unwrap();
        assert!(validate_standalone_executable(&directory.path().join("rms-memory")).is_err());
        assert!(validate_standalone_executable(directory.path()).is_err());
    }
}
