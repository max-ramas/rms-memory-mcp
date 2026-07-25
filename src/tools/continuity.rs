//! Session continuity: project overview and vault-backed checkpoints.
//!
//! Checkpoints are plain Markdown notes under `artifacts/checkpoints/` with
//! `type: checkpoint` frontmatter. Active checkpoints (`status: active`) are
//! recallable like any other note; closed ones (`status: done`) drop out of
//! the default recall filter automatically. Closing a checkpoint writes a
//! durable session summary under `artifacts/sessions/`.
//!
//! Every function is scoped to exactly one vault root (fail-closed): callers
//! resolve the project first, and there is no cross-project or global view.

use super::AppContext;
use anyhow::Result;
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::Path;

pub const CHECKPOINTS_DIR: &str = "artifacts/checkpoints";
pub const SESSIONS_DIR: &str = "artifacts/sessions";
const DEFAULT_RECENT_LIMIT: usize = 10;
const LINKED_PREVIEW_CHARS: usize = 400;

// ---------------------------------------------------------------------------
// Payloads (shared by MCP, CLI hook, and the Tauri GUI)
// ---------------------------------------------------------------------------

#[derive(Serialize, Debug, Clone)]
pub struct CheckpointPayload {
    pub name: String,
    /// Vault-relative path of the checkpoint document.
    pub path: String,
    pub status: String,
    pub goal: Option<String>,
    pub pending: Option<String>,
    pub links: Vec<String>,
    pub changed_at: Option<String>,
    pub created_at: Option<String>,
    pub done_at: Option<String>,
    pub id: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct LinkedNote {
    pub path: String,
    pub title: Option<String>,
    pub exists: bool,
    /// Bounded preview of the note body (never the full document).
    pub preview: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CheckpointLoadPayload {
    pub checkpoint: CheckpointPayload,
    pub content: String,
    pub linked_notes: Vec<LinkedNote>,
}

#[derive(Serialize, Debug, Clone)]
pub struct CheckpointDonePayload {
    pub checkpoint: CheckpointPayload,
    /// Vault-relative path of the written session summary.
    pub session_path: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct RecentNote {
    pub path: String,
    pub title: Option<String>,
    pub changed_at: Option<String>,
    pub status: Option<String>,
}

#[derive(Serialize, Debug, Clone)]
pub struct OverviewCounts {
    pub total_documents: usize,
    pub by_folder: BTreeMap<String, usize>,
    pub by_status: BTreeMap<String, usize>,
}

#[derive(Serialize, Debug, Clone)]
pub struct OverviewCoverage {
    pub recent_limit: usize,
    pub recent_truncated: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct OverviewPayload {
    /// Registered project key. Overview is always scoped to one project.
    pub project: Option<String>,
    pub generated_at: String,
    pub counts: OverviewCounts,
    pub recent_notes: Vec<RecentNote>,
    pub active_checkpoints: Vec<CheckpointPayload>,
    pub coverage: OverviewCoverage,
}

// ---------------------------------------------------------------------------
// Core (vault-root scoped, reused by MCP, CLI hook, and GUI)
// ---------------------------------------------------------------------------

fn validate_checkpoint_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 128 {
        anyhow::bail!("Checkpoint name must be 1..=128 characters");
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
    {
        anyhow::bail!(
            "Checkpoint name may only contain letters, digits, '-', '_', '.' (got '{name}')"
        );
    }
    if name.starts_with('.') {
        anyhow::bail!("Checkpoint name must not start with '.'");
    }
    Ok(())
}

fn checkpoint_rel_path(name: &str) -> String {
    format!("{CHECKPOINTS_DIR}/{name}.md")
}

fn first_heading(content: &str) -> Option<String> {
    content
        .lines()
        .find(|line| line.starts_with('#'))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .filter(|title| !title.is_empty())
}

fn payload_from_doc(doc: &crate::document::Document, rel_path: &str) -> CheckpointPayload {
    let fm = doc.frontmatter.clone().unwrap_or_default();
    let name = Path::new(rel_path)
        .file_stem()
        .map(|stem| stem.to_string_lossy().to_string())
        .unwrap_or_else(|| rel_path.to_string());
    CheckpointPayload {
        name,
        path: rel_path.to_string(),
        status: fm.status.unwrap_or_else(|| "active".to_string()),
        goal: fm.goal,
        pending: fm.pending,
        links: fm.links.unwrap_or_default(),
        changed_at: fm.timestamp,
        created_at: fm.created_at,
        done_at: fm.done_at,
        id: fm.id,
    }
}

fn yaml_string(value: &str) -> serde_yaml::Value {
    serde_yaml::Value::String(value.to_string())
}

/// Render a checkpoint/session document deterministically.
#[allow(clippy::too_many_arguments)]
fn render_note(
    doc_type: &str,
    extra_fm: &[(&str, serde_yaml::Value)],
    id: &str,
    project: Option<&str>,
    caller_id: &str,
    created_at: &str,
    title: &str,
    sections: &[(&str, &str)],
) -> Result<String> {
    let now = chrono::Utc::now().to_rfc3339();
    let mut mapping = serde_yaml::Mapping::new();
    mapping.insert(yaml_string("id"), yaml_string(id));
    mapping.insert(yaml_string("type"), yaml_string(doc_type));
    for (key, value) in extra_fm {
        mapping.insert(yaml_string(key), value.clone());
    }
    if let Some(pk) = project {
        mapping.insert(yaml_string("project"), yaml_string(pk));
    }
    mapping.insert(yaml_string("created_at"), yaml_string(created_at));
    mapping.insert(yaml_string("timestamp"), yaml_string(&now));
    mapping.insert(yaml_string("last_modified_by"), yaml_string(caller_id));

    let fm_yaml = serde_yaml::to_string(&mapping)?.trim_end().to_string();
    let mut body = format!("---\n{fm_yaml}\n---\n\n# {title}\n");
    for (heading, text) in sections {
        if !text.is_empty() {
            body.push_str(&format!("\n## {heading}\n\n{text}\n"));
        }
    }
    Ok(body)
}

/// Create or update a checkpoint. Updating preserves id/created_at and keeps
/// omitted fields from the existing document.
pub fn checkpoint_save(
    vault_root: &Path,
    caller_id: &str,
    project: Option<&str>,
    name: &str,
    goal: Option<&str>,
    pending: Option<&str>,
    links: Option<Vec<String>>,
) -> Result<CheckpointPayload> {
    validate_checkpoint_name(name)?;
    let rel_path = checkpoint_rel_path(name);
    let file_path = super::validation::resolve_vault_path(vault_root, &rel_path)?;

    let existing = if file_path.exists() {
        Some(crate::document::Document::parse(&file_path)?)
    } else {
        None
    };
    let existing_fm = existing
        .as_ref()
        .and_then(|doc| doc.frontmatter.clone())
        .unwrap_or_default();

    let id = existing_fm
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let created_at = existing_fm
        .created_at
        .clone()
        .unwrap_or_else(|| chrono::Utc::now().to_rfc3339());
    let goal = goal
        .map(str::to_string)
        .or(existing_fm.goal)
        .unwrap_or_default();
    let pending = pending
        .map(str::to_string)
        .or(existing_fm.pending)
        .unwrap_or_default();
    let links = links.or(existing_fm.links).unwrap_or_default();
    let links_yaml =
        serde_yaml::Value::Sequence(links.iter().map(|link| yaml_string(link)).collect());

    let text = render_note(
        "checkpoint",
        &[
            ("status", yaml_string("active")),
            ("goal", yaml_string(&goal)),
            ("pending", yaml_string(&pending)),
            ("links", links_yaml),
        ],
        &id,
        project,
        caller_id,
        &created_at,
        &format!("Checkpoint: {name}"),
        &[("Goal", goal.as_str()), ("Pending", pending.as_str())],
    )?;

    if let Some(parent) = file_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&file_path, text)?;

    let doc = crate::document::Document::parse(&file_path)?;
    Ok(payload_from_doc(&doc, &rel_path))
}

/// Close a checkpoint: mark it `status: done` and write a durable session
/// summary note that stays recallable.
pub fn checkpoint_done(
    vault_root: &Path,
    caller_id: &str,
    project: Option<&str>,
    name: &str,
    summary: &str,
) -> Result<CheckpointDonePayload> {
    validate_checkpoint_name(name)?;
    let rel_path = checkpoint_rel_path(name);
    let file_path = super::validation::resolve_vault_path(vault_root, &rel_path)?;
    if !file_path.exists() {
        anyhow::bail!("Checkpoint '{name}' not found at {rel_path}");
    }

    let doc = crate::document::Document::parse(&file_path)?;
    let fm = doc.frontmatter.clone().unwrap_or_default();
    let now = chrono::Utc::now().to_rfc3339();
    let id = fm
        .id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let created_at = fm.created_at.clone().unwrap_or_else(|| now.clone());
    let goal = fm.goal.clone().unwrap_or_default();
    let pending = fm.pending.clone().unwrap_or_default();
    let links = fm.links.clone().unwrap_or_default();
    let links_yaml =
        serde_yaml::Value::Sequence(links.iter().map(|link| yaml_string(link)).collect());

    // Rewrite the checkpoint itself as done. `status: done` drops it out of
    // the default recall filter without deleting history.
    let closed = render_note(
        "checkpoint",
        &[
            ("status", yaml_string("done")),
            ("goal", yaml_string(&goal)),
            ("pending", yaml_string(&pending)),
            ("links", links_yaml.clone()),
            ("done_at", yaml_string(&now)),
        ],
        &id,
        project,
        caller_id,
        &created_at,
        &format!("Checkpoint: {name}"),
        &[
            ("Goal", goal.as_str()),
            ("Pending", pending.as_str()),
            ("Outcome", summary),
        ],
    )?;
    std::fs::write(&file_path, closed)?;

    // Durable session summary: recallable knowledge about what happened.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let session_rel = format!("{SESSIONS_DIR}/{name}-{stamp}.md");
    let session_path = super::validation::resolve_vault_path(vault_root, &session_rel)?;
    let session_id = uuid::Uuid::new_v4().to_string();
    let session_text = render_note(
        "session",
        &[
            ("status", yaml_string("active")),
            ("checkpoint", yaml_string(name)),
            ("links", links_yaml),
        ],
        &session_id,
        project,
        caller_id,
        &now,
        &format!("Session summary: {name}"),
        &[
            ("Goal", goal.as_str()),
            ("Done", summary),
            ("Remaining", pending.as_str()),
        ],
    )?;
    if let Some(parent) = session_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&session_path, session_text)?;

    let doc = crate::document::Document::parse(&file_path)?;
    Ok(CheckpointDonePayload {
        checkpoint: payload_from_doc(&doc, &rel_path),
        session_path: session_rel,
    })
}

/// Load one checkpoint with its full body and bounded previews of linked notes.
pub fn checkpoint_load(vault_root: &Path, name: &str) -> Result<CheckpointLoadPayload> {
    validate_checkpoint_name(name)?;
    let rel_path = checkpoint_rel_path(name);
    let file_path = super::validation::resolve_vault_path(vault_root, &rel_path)?;
    if !file_path.exists() {
        anyhow::bail!("Checkpoint '{name}' not found at {rel_path}");
    }
    let doc = crate::document::Document::parse(&file_path)?;
    let payload = payload_from_doc(&doc, &rel_path);

    let mut linked_notes = Vec::new();
    for link in &payload.links {
        let resolved = super::validation::resolve_vault_path(vault_root, link);
        let note = match resolved {
            Ok(path) if path.exists() => match crate::document::Document::parse(&path) {
                Ok(linked) => {
                    let trimmed = linked.content.trim();
                    let preview: String = trimmed.chars().take(LINKED_PREVIEW_CHARS).collect();
                    LinkedNote {
                        path: link.clone(),
                        title: first_heading(&linked.content),
                        exists: true,
                        preview: Some(preview),
                    }
                }
                Err(_) => LinkedNote {
                    path: link.clone(),
                    title: None,
                    exists: true,
                    preview: None,
                },
            },
            _ => LinkedNote {
                path: link.clone(),
                title: None,
                exists: false,
                preview: None,
            },
        };
        linked_notes.push(note);
    }

    Ok(CheckpointLoadPayload {
        checkpoint: payload,
        content: doc.content,
        linked_notes,
    })
}

/// List checkpoints, optionally filtered by status (`active`, `done`).
/// Returns full pending text (no preview truncation), newest first.
pub fn checkpoint_query(
    vault_root: &Path,
    status_filter: Option<&str>,
) -> Result<Vec<CheckpointPayload>> {
    let dir = vault_root.join(CHECKPOINTS_DIR);
    let mut payloads = Vec::new();
    if !dir.exists() {
        return Ok(payloads);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let Ok(doc) = crate::document::Document::parse(&path) else {
            continue;
        };
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        let rel_path = format!("{CHECKPOINTS_DIR}/{file_name}");
        let payload = payload_from_doc(&doc, &rel_path);
        if let Some(filter) = status_filter
            && payload.status != filter
        {
            continue;
        }
        payloads.push(payload);
    }
    payloads.sort_by(|a, b| b.changed_at.cmp(&a.changed_at));
    Ok(payloads)
}

/// Build a structured overview of exactly one project vault: counts, recent
/// notes, and active checkpoints. Never aggregates across projects.
pub fn overview(
    vault_root: &Path,
    project: Option<&str>,
    recent_limit: usize,
) -> Result<OverviewPayload> {
    let recent_limit = recent_limit.clamp(1, 50);
    let pattern = vault_root.join("**/*.md");
    let mut by_folder: BTreeMap<String, usize> = BTreeMap::new();
    let mut by_status: BTreeMap<String, usize> = BTreeMap::new();
    let mut total = 0usize;
    let mut recent: Vec<RecentNote> = Vec::new();

    for entry in glob::glob(&pattern.to_string_lossy())? {
        let Ok(path) = entry else { continue };
        if crate::path_policy::is_vault_wiki_path(vault_root, &path) {
            continue;
        }
        let rel = path
            .strip_prefix(vault_root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();
        total += 1;
        let folder = if rel.contains('/') {
            rel.split('/').next().unwrap_or("(root)").to_string()
        } else {
            "(root)".to_string()
        };
        *by_folder.entry(folder).or_insert(0) += 1;

        let (status, changed_at, title) = match crate::document::Document::parse(&path) {
            Ok(doc) => {
                let fm = doc.frontmatter.clone().unwrap_or_default();
                (
                    fm.status.unwrap_or_else(|| "active".to_string()),
                    fm.timestamp.or(fm.created_at),
                    first_heading(&doc.content),
                )
            }
            Err(_) => ("unparsable".to_string(), None, None),
        };
        *by_status.entry(status.clone()).or_insert(0) += 1;
        recent.push(RecentNote {
            path: rel,
            title,
            changed_at,
            status: Some(status),
        });
    }

    recent.sort_by(|a, b| b.changed_at.cmp(&a.changed_at));
    let recent_truncated = recent.len() > recent_limit;
    recent.truncate(recent_limit);

    Ok(OverviewPayload {
        project: project.map(str::to_string),
        generated_at: chrono::Utc::now().to_rfc3339(),
        counts: OverviewCounts {
            total_documents: total,
            by_folder,
            by_status,
        },
        recent_notes: recent,
        active_checkpoints: checkpoint_query(vault_root, Some("active"))?,
        coverage: OverviewCoverage {
            recent_limit,
            recent_truncated,
        },
    })
}

/// Canonical memory-usage protocol, returned by `rms_system_instructions` so
/// agents can self-bootstrap without relying on injected rule files.
pub fn system_instructions(project: Option<&str>) -> String {
    let project_line = match project {
        Some(key) => format!("This connection is bound to project `{key}`."),
        None => "No project is bound yet; pass the registered key in the `project` argument (see `rms_projects`).".to_string(),
    };
    format!(
        r#"# RMS Memory usage protocol

{project_line}

## Core loop
1. SEARCH FIRST: call `rms_search` before substantial changes. It returns a decision envelope (inject/abstain); trust an abstain instead of forcing weak context.
2. READ CONTEXT: call `rms_read` on relevant hits (ADRs, rules) to ingest full documents.
3. ORIENT: call `rms_overview` at session start for counts, recent notes, and active checkpoints of the current project.
4. PERSIST: at task end, save new conventions, tricky fixes, and decisions with `rms_write`. Folders: architecture/, rules/, decisions/, artifacts/, docs/, api/.

## Session continuity
- Before context compaction or a long pause, save progress: `rms_checkpoint_save` with name, goal, pending work, and linked vault paths.
- Resuming: `rms_checkpoint_query` (status=active), then `rms_checkpoint_load` for the full state and linked note previews.
- Finished: `rms_checkpoint_done` with a summary of what was done; this closes the checkpoint and writes a durable session summary.

## Rules
- All tools are scoped to exactly one project; never mix vaults.
- Use `supersedes` in `rms_write` to soft-replace outdated notes instead of deleting them.
- Do not write into the generated `wiki/` namespace.
"#
    )
}

// ---------------------------------------------------------------------------
// MCP wrappers
// ---------------------------------------------------------------------------

fn require_root(ctx: &AppContext) -> Result<&std::path::PathBuf> {
    ctx.workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))
}

pub async fn execute_overview(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let root = require_root(ctx)?;
    let recent_limit = args
        .get("recent_limit")
        .and_then(|v| v.as_u64())
        .map(|v| v as usize)
        .unwrap_or(DEFAULT_RECENT_LIMIT);
    let payload = overview(root, ctx.project_key.as_deref(), recent_limit)?;
    super::response::json_structured_response(&payload)
}

pub async fn execute_checkpoint_save(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let root = require_root(ctx)?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`name` is required"))?;
    let links = args.get("links").and_then(|v| v.as_array()).map(|items| {
        items
            .iter()
            .filter_map(|item| item.as_str().map(str::to_string))
            .collect::<Vec<_>>()
    });
    let payload = checkpoint_save(
        root,
        &ctx.caller_id,
        ctx.project_key.as_deref(),
        name,
        args.get("goal").and_then(|v| v.as_str()),
        args.get("pending").and_then(|v| v.as_str()),
        links,
    )?;
    super::response::json_structured_response(&payload)
}

pub async fn execute_checkpoint_done(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let root = require_root(ctx)?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`name` is required"))?;
    let summary = args
        .get("summary")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`summary` is required"))?;
    let payload = checkpoint_done(
        root,
        &ctx.caller_id,
        ctx.project_key.as_deref(),
        name,
        summary,
    )?;
    super::response::json_structured_response(&payload)
}

pub async fn execute_checkpoint_load(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let root = require_root(ctx)?;
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("`name` is required (use rms_checkpoint_query to list)"))?;
    let payload = checkpoint_load(root, name)?;
    super::response::json_structured_response(&payload)
}

pub async fn execute_checkpoint_query(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    let root = require_root(ctx)?;
    let status = args
        .get("status")
        .and_then(|v| v.as_str())
        .filter(|s| *s != "all");
    let payload = checkpoint_query(root, status)?;
    super::response::json_structured_response(&payload)
}

pub async fn execute_system_instructions(
    ctx: &AppContext,
    _args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    Ok(super::response::json_text_response(&system_instructions(
        ctx.project_key.as_deref(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn vault() -> tempfile::TempDir {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("decisions")).unwrap();
        dir
    }

    #[test]
    fn checkpoint_round_trip_save_load_done() {
        let dir = vault();
        let root = &std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(
            root.join("decisions/db.md"),
            "---\nid: d1\n---\n\n# DB choice\n\nUse LanceDB.\n",
        )
        .unwrap();

        let saved = checkpoint_save(
            root,
            "test-agent",
            Some("proj-a"),
            "migration",
            Some("Migrate store"),
            Some("Finish phase 2"),
            Some(vec!["decisions/db.md".to_string()]),
        )
        .expect("save");
        assert_eq!(saved.status, "active");
        assert_eq!(saved.goal.as_deref(), Some("Migrate store"));

        // Update keeps id and created_at, merges omitted fields.
        let updated = checkpoint_save(
            root,
            "test-agent",
            Some("proj-a"),
            "migration",
            None,
            Some("Finish phase 3"),
            None,
        )
        .expect("update");
        assert_eq!(updated.id, saved.id);
        assert_eq!(updated.goal.as_deref(), Some("Migrate store"));
        assert_eq!(updated.pending.as_deref(), Some("Finish phase 3"));
        assert_eq!(updated.links, vec!["decisions/db.md".to_string()]);

        let loaded = checkpoint_load(root, "migration").expect("load");
        assert_eq!(loaded.linked_notes.len(), 1);
        assert!(loaded.linked_notes[0].exists);
        assert_eq!(loaded.linked_notes[0].title.as_deref(), Some("DB choice"));

        let done = checkpoint_done(root, "test-agent", Some("proj-a"), "migration", "Shipped")
            .expect("done");
        assert_eq!(done.checkpoint.status, "done");
        assert!(done.checkpoint.done_at.is_some());
        assert!(root.join(&done.session_path).exists());
        let session = std::fs::read_to_string(root.join(&done.session_path)).unwrap();
        assert!(session.contains("type: session"));
        assert!(session.contains("Shipped"));

        // Done checkpoints drop out of the active query.
        assert!(checkpoint_query(root, Some("active")).unwrap().is_empty());
        assert_eq!(checkpoint_query(root, Some("done")).unwrap().len(), 1);
        assert_eq!(checkpoint_query(root, None).unwrap().len(), 1);
    }

    #[test]
    fn checkpoint_name_is_validated_fail_closed() {
        let dir = vault();
        let root = &std::fs::canonicalize(dir.path()).unwrap();
        for bad in ["", "../escape", "a/b", ".hidden", "name with spaces"] {
            assert!(
                checkpoint_save(root, "t", None, bad, None, None, None).is_err(),
                "expected rejection for {bad:?}"
            );
        }
    }

    #[test]
    fn overview_counts_and_active_checkpoints_single_project() {
        let dir = vault();
        let root = &std::fs::canonicalize(dir.path()).unwrap();
        std::fs::write(
            root.join("decisions/one.md"),
            "---\nid: a\nstatus: active\ntimestamp: '2026-01-01T00:00:00Z'\n---\n\n# One\n",
        )
        .unwrap();
        std::fs::write(
            root.join("decisions/two.md"),
            "---\nid: b\nstatus: superseded\ntimestamp: '2026-01-02T00:00:00Z'\n---\n\n# Two\n",
        )
        .unwrap();
        checkpoint_save(root, "t", Some("p"), "cp1", Some("g"), Some("p"), None).unwrap();

        let payload = overview(root, Some("p"), 10).expect("overview");
        assert_eq!(payload.project.as_deref(), Some("p"));
        assert_eq!(payload.counts.total_documents, 3);
        assert_eq!(payload.counts.by_folder.get("decisions"), Some(&2));
        assert_eq!(payload.counts.by_status.get("superseded"), Some(&1));
        assert_eq!(payload.active_checkpoints.len(), 1);
        assert!(!payload.coverage.recent_truncated);
        // Newest first by frontmatter timestamp.
        assert!(payload.recent_notes[0].changed_at >= payload.recent_notes[1].changed_at);
    }

    #[test]
    fn system_instructions_mention_binding_and_tools() {
        let bound = system_instructions(Some("proj-x"));
        assert!(bound.contains("proj-x"));
        assert!(bound.contains("rms_checkpoint_save"));
        let unbound = system_instructions(None);
        assert!(unbound.contains("rms_projects"));
    }
}
