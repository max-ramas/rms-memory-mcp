//! Editor-agnostic continuity hooks.
//!
//! `rms-memory hook --event <name>` is the canonical automation interface:
//! Cursor, VS Code, Neovim, CI, or the Tauri GUI call this CLI instead of
//! depending on any IDE-specific hook format. Output is machine-readable
//! JSON on stdout.
//!
//! Scoping is fail-closed: the project comes from an explicit `--project`
//! key or from a directory that resolves to exactly one registered project.
//! There is no global or cross-project mode.

use anyhow::Result;
use clap::{Args, ValueEnum};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum HookEvent {
    /// Session begins: emit the project overview and active checkpoints.
    #[value(name = "session_start")]
    SessionStart,
    /// Context is about to be compacted: emit a checkpoint reminder, or with
    /// --apply create/update a checkpoint stub.
    #[value(name = "pre_compact")]
    PreCompact,
    /// Session ends: emit active checkpoints, or with --apply close one.
    #[value(name = "session_stop")]
    SessionStop,
}

impl HookEvent {
    fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::PreCompact => "pre_compact",
            Self::SessionStop => "session_stop",
        }
    }
}

#[derive(Args, Debug)]
pub struct HookArgs {
    /// Hook event to process
    #[arg(long, value_enum)]
    pub event: HookEvent,

    /// Registered project key (fail-closed alternative to cwd resolution)
    #[arg(long)]
    pub project: Option<String>,

    /// Resolve the project from this directory instead of the current one
    #[arg(long)]
    pub cwd: Option<PathBuf>,

    /// Apply side effects (create/update or close a checkpoint) instead of
    /// emitting a hint only
    #[arg(long)]
    pub apply: bool,

    /// Checkpoint name (required with --apply)
    #[arg(long)]
    pub name: Option<String>,

    /// Checkpoint goal (pre_compact --apply)
    #[arg(long)]
    pub goal: Option<String>,

    /// Pending work description (pre_compact --apply)
    #[arg(long)]
    pub pending: Option<String>,

    /// Summary of completed work (session_stop --apply)
    #[arg(long)]
    pub summary: Option<String>,
}

/// Resolve exactly one registered project, fail-closed. Never guesses and
/// never falls back to a global view.
pub fn resolve_project(
    project: Option<&str>,
    cwd: Option<&Path>,
) -> Result<(String, crate::workspace::Workspace)> {
    if let Some(key) = project {
        let registry = crate::workspace::Registry::load()?;
        if let Some((resolved_key, config)) = registry.locate_by_project_key(key) {
            let workspace =
                crate::workspace::Workspace::discover(Path::new(&config.code_path), None)?;
            return Ok((resolved_key.to_string(), workspace));
        }
        if let Some(message) = registry.migration_redirect_message(key) {
            anyhow::bail!(message);
        }
        anyhow::bail!(
            "Unknown RMS Memory project key: '{key}'. Use `rms-memory projects list` to see registered keys."
        );
    }

    let dir = match cwd {
        Some(dir) => dir.to_path_buf(),
        None => std::env::current_dir()?,
    };
    let workspace = crate::workspace::Workspace::discover(&dir, None).map_err(|error| {
        anyhow::anyhow!(
            "Cannot resolve a registered project from '{}': {error}. Pass --project <key> explicitly (fail-closed, no global fallback).",
            dir.display()
        )
    })?;
    let key = workspace.project_key().ok_or_else(|| {
        anyhow::anyhow!(
            "Directory '{}' resolved to a vault without a registry key. Pass --project <key> explicitly.",
            dir.display()
        )
    })?;
    Ok((key, workspace))
}

impl HookArgs {
    pub fn run(&self) -> Result<()> {
        let (project_key, workspace) =
            resolve_project(self.project.as_deref(), self.cwd.as_deref())?;
        let output = self.dispatch(&project_key, &workspace.root)?;
        println!("{}", serde_json::to_string_pretty(&output)?);
        Ok(())
    }

    fn dispatch(&self, project_key: &str, vault_root: &Path) -> Result<serde_json::Value> {
        use crate::tools::continuity;
        let caller = "rms-memory-hook";
        let event = self.event.as_str();

        match self.event {
            HookEvent::SessionStart => {
                let overview = continuity::overview(vault_root, Some(project_key), 10)?;
                Ok(serde_json::json!({
                    "event": event,
                    "project": project_key,
                    "overview": overview,
                }))
            }
            HookEvent::PreCompact => {
                if self.apply {
                    let name = self.name.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("--apply for pre_compact requires --name")
                    })?;
                    let checkpoint = continuity::checkpoint_save(
                        vault_root,
                        caller,
                        Some(project_key),
                        name,
                        self.goal.as_deref(),
                        self.pending.as_deref(),
                        None,
                    )?;
                    Ok(serde_json::json!({
                        "event": event,
                        "project": project_key,
                        "applied": true,
                        "checkpoint": checkpoint,
                    }))
                } else {
                    let active = continuity::checkpoint_query(vault_root, Some("active"))?;
                    Ok(serde_json::json!({
                        "event": event,
                        "project": project_key,
                        "applied": false,
                        "active_checkpoints": active,
                        "action_required": "Save or update a checkpoint (rms_checkpoint_save or `rms-memory hook --event pre_compact --apply --name <n> --goal <g> --pending <p>`) before compaction.",
                    }))
                }
            }
            HookEvent::SessionStop => {
                if self.apply {
                    let name = self.name.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("--apply for session_stop requires --name")
                    })?;
                    let summary = self.summary.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("--apply for session_stop requires --summary")
                    })?;
                    let done = continuity::checkpoint_done(
                        vault_root,
                        caller,
                        Some(project_key),
                        name,
                        summary,
                    )?;
                    Ok(serde_json::json!({
                        "event": event,
                        "project": project_key,
                        "applied": true,
                        "checkpoint": done.checkpoint,
                        "session_path": done.session_path,
                    }))
                } else {
                    let active = continuity::checkpoint_query(vault_root, Some("active"))?;
                    Ok(serde_json::json!({
                        "event": event,
                        "project": project_key,
                        "applied": false,
                        "active_checkpoints": active,
                        "hint": "Close finished work with rms_checkpoint_done (or `--apply --name <n> --summary <s>`); leave unfinished checkpoints active for the next session.",
                    }))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_project_key_fails_closed() {
        let error = resolve_project(Some("definitely-not-registered-xyz"), None)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("Unknown RMS Memory project key") || error.contains("was migrated to"),
            "got: {error}"
        );
    }

    #[test]
    fn unregistered_directory_fails_closed() {
        let dir = tempfile::tempdir().unwrap();
        let error = resolve_project(None, Some(dir.path()))
            .unwrap_err()
            .to_string();
        assert!(error.contains("--project"), "got: {error}");
    }
}
