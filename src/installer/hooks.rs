//! Thin L3 continuity adapters for IDEs.
//!
//! The canonical automation surface is always `rms-memory hook --event …`.
//! This module only writes thin wrappers (scripts + IDE-native config) that
//! call that CLI — no IDE owns the architecture.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const MANAGED_MARKER: &str = "rms-memory-managed";

/// A hook entry the installer owns: flagged `rmsManaged` or pointing at our script.
fn is_managed_hook_entry(item: &serde_json::Value) -> bool {
    item.get("rmsManaged")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
        || item
            .get("command")
            .and_then(|v| v.as_str())
            .is_some_and(|cmd| cmd.contains("rms-memory-session-continuity"))
}
const CURSOR_HOOKS_REL: &str = ".cursor/hooks.json";
const CURSOR_SCRIPT_REL: &str = ".cursor/hooks/rms-memory-session-continuity.sh";
const CLAUDE_HOOKS_REL: &str = ".claude/hooks/rms-memory-session-continuity.sh";
const NVIM_SCRIPT_REL: &str = ".config/nvim/rms-memory-hook.sh";

fn shared_script_rel() -> &'static str {
    ".rms-memory/hooks/session-continuity.sh"
}

/// Escape a path for a single-quoted bash literal.
///
/// The installer executable path comes from `current_exe` / CLI arguments and may
/// contain `$`, backticks, spaces, or quotes (e.g. usernames). Single-quoting is
/// the safest portable form; only `'` needs special handling.
fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn continuity_script_body(executable: &str) -> String {
    format!(
        r#"#!/usr/bin/env bash
# {MANAGED_MARKER}: thin adapter — calls the editor-agnostic rms-memory hook CLI.
# Do not put IDE-specific logic here; edit `rms-memory hook` instead.
set -euo pipefail
EVENT="${{1:-}}"
case "$EVENT" in
  session_start|sessionStart) EVENT=session_start ;;
  pre_compact|preCompact) EVENT=pre_compact ;;
  session_stop|stop) EVENT=session_stop ;;
  "")
    echo "usage: $0 <session_start|pre_compact|session_stop>" >&2
    exit 2
    ;;
esac
exec {exe} hook --event "$EVENT"
"#,
        exe = shell_single_quote(executable)
    )
}

fn warn_invalid_hooks_json(path: &Path, reason: &str) {
    let message = format!(
        "[⚠️] Failed to parse Cursor hooks.json {}: {}. Backing up and rewriting managed hooks; foreign entries from this file may be lost.",
        path.display(),
        reason
    );
    tracing::warn!("{message}");
    println!("{message}");
}

/// Parse hooks.json; on invalid / non-object content warn and start from `{{}}`.
fn parse_cursor_hooks_json(existing: &str, path: &Path) -> serde_json::Value {
    if existing.trim().is_empty() {
        return serde_json::json!({});
    }
    match serde_json::from_str::<serde_json::Value>(existing) {
        Ok(value) if value.is_object() => value,
        Ok(_) => {
            warn_invalid_hooks_json(path, "root value is not a JSON object");
            serde_json::json!({})
        }
        Err(error) => {
            warn_invalid_hooks_json(path, &error.to_string());
            serde_json::json!({})
        }
    }
}

fn write_executable_script(path: &Path, body: &str, dry_run: bool) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if dry_run {
        println!("[DRY-RUN] Would write L3 hook script {}", path.display());
        return Ok(());
    }
    fs::write(path, body).with_context(|| format!("Failed to write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(path)?.permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms)?;
    }
    println!("[✅] L3 hook script {}", path.display());
    Ok(())
}

/// Merge managed Cursor hook entries without destroying unrelated hooks.
pub fn merge_cursor_hooks_json(
    existing: &str,
    script_path: &str,
    hooks_path: &Path,
) -> Result<String> {
    let mut root = parse_cursor_hooks_json(existing, hooks_path);
    let hooks = root
        .as_object_mut()
        .unwrap()
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() {
        *hooks = serde_json::json!({});
    }
    let hooks_obj = hooks.as_object_mut().unwrap();

    let events = [
        ("sessionStart", "session_start"),
        ("preCompact", "pre_compact"),
        ("stop", "session_stop"),
    ];
    for (cursor_event, cli_event) in events {
        let command = format!("{script_path} {cli_event}");
        let entry = serde_json::json!({
            "command": command,
            "rmsManaged": true
        });
        let list = hooks_obj
            .entry(cursor_event.to_string())
            .or_insert_with(|| serde_json::json!([]));
        if !list.is_array() {
            *list = serde_json::json!([]);
        }
        let arr = list.as_array_mut().unwrap();
        // Drop previous managed entries; keep user-authored ones.
        arr.retain(|item| !is_managed_hook_entry(item));
        arr.push(entry);
    }

    Ok(serde_json::to_string_pretty(&root)?)
}

/// Remove managed Cursor hook entries; leave unrelated hooks intact.
pub fn strip_cursor_hooks_json(existing: &str) -> Result<Option<String>> {
    let mut root: serde_json::Value = match serde_json::from_str(existing) {
        Ok(v) => v,
        Err(_) => return Ok(None),
    };
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return Ok(None);
    };
    let mut changed = false;
    for key in ["sessionStart", "preCompact", "stop"] {
        let Some(list) = hooks.get_mut(key).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let before = list.len();
        list.retain(|item| !is_managed_hook_entry(item));
        if list.len() != before {
            changed = true;
        }
        if list.is_empty() {
            hooks.remove(key);
            changed = true;
        }
    }
    if !changed {
        return Ok(None);
    }
    Ok(Some(serde_json::to_string_pretty(&root)?))
}

pub fn install_l3_adapters(home: &Path, executable: &str, dry_run: bool) -> Result<()> {
    let body = continuity_script_body(executable);
    let shared = home.join(shared_script_rel());
    write_executable_script(&shared, &body, dry_run)?;

    // Cursor: managed hooks.json + local script (Cursor prefers paths under ~/.cursor).
    let cursor_dir = home.join(".cursor");
    if cursor_dir.exists() || dry_run {
        let cursor_script = home.join(CURSOR_SCRIPT_REL);
        write_executable_script(&cursor_script, &body, dry_run)?;
        let hooks_path = home.join(CURSOR_HOOKS_REL);
        let existing = if hooks_path.exists() {
            fs::read_to_string(&hooks_path).unwrap_or_default()
        } else {
            String::new()
        };
        let merged =
            merge_cursor_hooks_json(&existing, &cursor_script.to_string_lossy(), &hooks_path)?;
        if dry_run {
            println!("[DRY-RUN] Would patch {}", hooks_path.display());
        } else {
            if let Some(parent) = hooks_path.parent() {
                fs::create_dir_all(parent)?;
            }
            if hooks_path.exists() {
                let _ = fs::copy(&hooks_path, format!("{}.bak", hooks_path.display()));
            }
            fs::write(&hooks_path, merged)?;
            println!("[✅] Cursor L3 hooks {}", hooks_path.display());
        }
    } else {
        println!("[·] Cursor not detected (no ~/.cursor); skipped hooks.json.");
    }

    // Claude Code: drop a callable script under ~/.claude/hooks when the tree exists.
    let claude_dir = home.join(".claude");
    if claude_dir.exists() || dry_run {
        write_executable_script(&home.join(CLAUDE_HOOKS_REL), &body, dry_run)?;
        println!(
            "[·] Claude Code: call `{} session_start|pre_compact|session_stop` from session hooks.",
            home.join(CLAUDE_HOOKS_REL).display()
        );
    }

    // Neovim: thin shell under ~/.config/nvim for autocmd / user wiring.
    let nvim_config = home.join(".config/nvim");
    if nvim_config.exists() || dry_run {
        write_executable_script(&home.join(NVIM_SCRIPT_REL), &body, dry_run)?;
        println!(
            "[·] Neovim: call `{} <event>` from an autocmd or plugin.",
            home.join(NVIM_SCRIPT_REL).display()
        );
    }

    Ok(())
}

pub fn uninstall_l3_adapters(home: &Path, dry_run: bool) -> Result<u32> {
    let mut removed = 0u32;
    let hooks_path = home.join(CURSOR_HOOKS_REL);
    if hooks_path.exists() {
        let existing = fs::read_to_string(&hooks_path)?;
        if let Some(stripped) = strip_cursor_hooks_json(&existing)? {
            if dry_run {
                println!(
                    "[DRY-RUN] Would strip managed Cursor hooks from {}",
                    hooks_path.display()
                );
            } else {
                let _ = fs::copy(&hooks_path, format!("{}.bak", hooks_path.display()));
                fs::write(&hooks_path, stripped)?;
                println!(
                    "[🗑️] Removed managed Cursor L3 hooks from {}",
                    hooks_path.display()
                );
            }
            removed += 1;
        }
    }

    for rel in [
        CURSOR_SCRIPT_REL,
        CLAUDE_HOOKS_REL,
        NVIM_SCRIPT_REL,
        shared_script_rel(),
    ] {
        let path = home.join(rel);
        if path.exists() {
            if dry_run {
                println!("[DRY-RUN] Would remove {}", path.display());
            } else {
                let _ = fs::remove_file(&path);
                println!("[🗑️] Removed {}", path.display());
            }
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn managed_script_paths(home: &Path) -> Vec<PathBuf> {
    vec![
        home.join(shared_script_rel()),
        home.join(CURSOR_SCRIPT_REL),
        home.join(CLAUDE_HOOKS_REL),
        home.join(NVIM_SCRIPT_REL),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_cursor_hooks_preserves_foreign_entries() {
        let existing = r#"{
  "hooks": {
    "sessionStart": [{ "command": "echo mine" }],
    "preCompact": [{ "command": "old rms-memory-session-continuity.sh pre_compact", "rmsManaged": true }]
  }
}"#;
        let path = Path::new("/tmp/hooks.json");
        let merged = merge_cursor_hooks_json(existing, "/tmp/rms.sh", path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        let start = value["hooks"]["sessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 2);
        assert_eq!(start[0]["command"], "echo mine");
        assert!(
            start[1]["command"]
                .as_str()
                .unwrap()
                .contains("session_start")
        );
        let compact = value["hooks"]["preCompact"].as_array().unwrap();
        assert_eq!(compact.len(), 1);
        assert_eq!(compact[0]["rmsManaged"], true);
    }

    #[test]
    fn merge_invalid_hooks_json_starts_fresh_with_managed_entries() {
        let path = Path::new("/tmp/broken-hooks.json");
        let merged = merge_cursor_hooks_json("{ not json", "/tmp/rms.sh", path).unwrap();
        let value: serde_json::Value = serde_json::from_str(&merged).unwrap();
        assert!(value["hooks"]["sessionStart"].as_array().unwrap().len() == 1);
        assert_eq!(value["hooks"]["sessionStart"][0]["rmsManaged"], true);
    }

    #[test]
    fn shell_single_quote_neutralizes_expansions_and_quotes() {
        assert_eq!(shell_single_quote("/opt/rms-memory"), "'/opt/rms-memory'");
        assert_eq!(
            shell_single_quote("/Users/$USER/bin/rms-memory"),
            "'/Users/$USER/bin/rms-memory'"
        );
        assert_eq!(
            shell_single_quote("/tmp/weird`name'/rms-memory"),
            "'/tmp/weird`name'\\''/rms-memory'"
        );
        let body = continuity_script_body("/Users/$USER/App Support/rms-memory");
        assert!(body.contains("exec '/Users/$USER/App Support/rms-memory' hook"));
        assert!(!body.contains("exec \"/Users/$USER"));
    }

    #[test]
    fn strip_cursor_hooks_removes_only_managed() {
        let existing = r#"{
  "hooks": {
    "sessionStart": [
      { "command": "echo mine" },
      { "command": "/x/rms-memory-session-continuity.sh session_start", "rmsManaged": true }
    ]
  }
}"#;
        let stripped = strip_cursor_hooks_json(existing).unwrap().unwrap();
        let value: serde_json::Value = serde_json::from_str(&stripped).unwrap();
        let start = value["hooks"]["sessionStart"].as_array().unwrap();
        assert_eq!(start.len(), 1);
        assert_eq!(start[0]["command"], "echo mine");
    }

    #[test]
    fn install_writes_shared_and_cursor_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        fs::create_dir_all(home.join(".cursor")).unwrap();
        install_l3_adapters(home, "/usr/local/bin/rms-memory", false).unwrap();
        assert!(home.join(shared_script_rel()).exists());
        assert!(home.join(CURSOR_SCRIPT_REL).exists());
        assert!(home.join(CURSOR_HOOKS_REL).exists());
        let hooks = fs::read_to_string(home.join(CURSOR_HOOKS_REL)).unwrap();
        assert!(hooks.contains("session_start"));
        assert!(hooks.contains("rmsManaged"));
        let script = fs::read_to_string(home.join(CURSOR_SCRIPT_REL)).unwrap();
        assert!(script.contains("exec '/usr/local/bin/rms-memory' hook"));
    }
}
