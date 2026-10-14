//! MCP resources (`rms://<project>/...`) and prompts.
//!
//! Resources are read-only views over one project vault: the overview, the
//! usage protocol, checkpoints and individual notes. Prompts are reusable
//! task starters that point the agent at the matching tools.

use super::AppContext;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};

pub const SCHEME: &str = "rms://";

/// A parsed `rms://<project>/<kind>[/<rest>]` URI.
#[derive(Debug, PartialEq, Eq)]
pub struct ResourceUri {
    pub project: String,
    pub target: ResourceTarget,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResourceTarget {
    Overview,
    Instructions,
    Checkpoint(String),
    Note(String),
}

impl ResourceUri {
    pub fn parse(uri: &str) -> Result<Self> {
        let rest = uri.strip_prefix(SCHEME).ok_or_else(|| {
            anyhow!("Unsupported resource URI {uri:?}; expected {SCHEME}<project>/...")
        })?;
        let (project, path) = rest
            .split_once('/')
            .ok_or_else(|| anyhow!("Resource URI {uri:?} has no resource path"))?;
        if project.is_empty() {
            bail!("Resource URI {uri:?} has no project key");
        }
        let (kind, tail) = path.split_once('/').unwrap_or((path, ""));
        let tail = percent_decode(tail)?;
        let target = match (kind, tail.is_empty()) {
            ("overview", true) => ResourceTarget::Overview,
            ("instructions", true) => ResourceTarget::Instructions,
            ("checkpoint", false) => ResourceTarget::Checkpoint(tail),
            ("note", false) => ResourceTarget::Note(tail),
            _ => bail!(
                "Unknown resource {uri:?}; use overview, instructions, checkpoint/<name> or note/<path>"
            ),
        };
        Ok(Self {
            project: percent_decode(project)?,
            target,
        })
    }
}

fn percent_decode(text: &str) -> Result<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text
                .get(index + 1..index + 3)
                .filter(|hex| hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
                .and_then(|hex| u8::from_str_radix(hex, 16).ok())
                .ok_or_else(|| anyhow!("Invalid percent-encoding in {text:?}"))?;
            out.push(hex);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).map_err(|_| anyhow!("Resource path {text:?} is not valid UTF-8"))
}

fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

fn resource(uri: String, name: String, description: &str, mime_type: &str) -> Value {
    json!({
        "uri": uri,
        "name": name,
        "description": description,
        "mimeType": mime_type,
    })
}

/// Resources of the bound project only; nothing is listed before binding.
pub fn list(ctx: &AppContext) -> Result<Value> {
    let (Some(project), Some(root)) = (ctx.project_key.as_deref(), ctx.workspace_root.as_ref())
    else {
        return Ok(json!({ "resources": [] }));
    };
    let base = format!("{SCHEME}{}", percent_encode_path(project));
    let mut resources = vec![
        resource(
            format!("{base}/overview"),
            format!("{project} overview"),
            "Document counts, recent notes, active checkpoints and git state",
            "application/json",
        ),
        resource(
            format!("{base}/instructions"),
            format!("{project} memory protocol"),
            "How agents should search, read and persist memory in this project",
            "text/markdown",
        ),
    ];
    for checkpoint in super::continuity::checkpoint_query(root, Some("active"))? {
        resources.push(resource(
            format!(
                "{base}/checkpoint/{}",
                percent_encode_path(&checkpoint.name)
            ),
            format!("Checkpoint: {}", checkpoint.name),
            checkpoint.goal.as_deref().unwrap_or("Active checkpoint"),
            "text/markdown",
        ));
    }
    Ok(json!({ "resources": resources }))
}

pub fn templates() -> Value {
    json!({
        "resourceTemplates": [
            {
                "uriTemplate": format!("{SCHEME}{{project}}/note/{{path}}"),
                "name": "Vault note",
                "description": "One Markdown note by vault-relative path (for example decisions/db.md)",
                "mimeType": "text/markdown"
            },
            {
                "uriTemplate": format!("{SCHEME}{{project}}/checkpoint/{{name}}"),
                "name": "Checkpoint",
                "description": "A session checkpoint with its goal, pending work and outcome",
                "mimeType": "text/markdown"
            },
            {
                "uriTemplate": format!("{SCHEME}{{project}}/overview"),
                "name": "Project overview",
                "description": "Counts, recent notes, active checkpoints, index lag and git state",
                "mimeType": "application/json"
            }
        ]
    })
}

/// Read a resource of the bound project; the caller binds `uri.project` first.
pub async fn read(ctx: &AppContext, uri: &str, parsed: &ResourceUri) -> Result<Value> {
    let root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow!("Workspace root not initialized"))?;
    let (mime_type, text) = match &parsed.target {
        ResourceTarget::Overview => {
            let response =
                super::continuity::execute_overview(ctx, &serde_json::Map::new()).await?;
            let text = response
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ("application/json", text)
        }
        ResourceTarget::Instructions => (
            "text/markdown",
            super::continuity::system_instructions(ctx.project_key.as_deref()),
        ),
        ResourceTarget::Checkpoint(name) => {
            let loaded = super::continuity::checkpoint_load(root, name)?;
            ("text/markdown", loaded.content)
        }
        ResourceTarget::Note(path) => {
            let mut args = serde_json::Map::new();
            args.insert("path".to_string(), json!(path));
            let response = super::read::execute(ctx, &args).await?;
            let text = response
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            ("text/markdown", text)
        }
    };
    Ok(json!({
        "contents": [{ "uri": uri, "mimeType": mime_type, "text": text }]
    }))
}

fn prompt_argument(name: &str, description: &str, required: bool) -> Value {
    json!({ "name": name, "description": description, "required": required })
}

pub fn prompts() -> Value {
    let project = prompt_argument(
        "project",
        "Registered project key; defaults to the bound project",
        false,
    );
    json!({
        "prompts": [
            {
                "name": "session_start",
                "description": "Orient in the project memory: overview, active checkpoints, what to read first",
                "arguments": [project]
            },
            {
                "name": "save_progress",
                "description": "Save a resumable checkpoint and persist what was learned",
                "arguments": [
                    prompt_argument("name", "Checkpoint name (letters, digits, - _ .)", true),
                    project
                ]
            },
            {
                "name": "record_decision",
                "description": "Record a technical decision without duplicating or contradicting existing notes",
                "arguments": [
                    prompt_argument("topic", "What was decided", true),
                    project
                ]
            }
        ]
    })
}

fn prompt_message(description: &str, text: String) -> Value {
    json!({
        "description": description,
        "messages": [{ "role": "user", "content": { "type": "text", "text": text } }]
    })
}

/// Render a prompt for the bound project; the caller binds `project` first.
pub async fn get_prompt(
    ctx: &AppContext,
    name: &str,
    args: &serde_json::Map<String, Value>,
) -> Result<Value> {
    let project = ctx
        .project_key
        .as_deref()
        .ok_or_else(|| anyhow!("No project bound; pass the `project` argument"))?;
    let required = |key: &str| {
        args.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow!("Prompt `{name}` requires the `{key}` argument"))
    };
    match name {
        "session_start" => {
            let overview =
                super::continuity::execute_overview(ctx, &serde_json::Map::new()).await?;
            let overview = overview
                .pointer("/content/0/text")
                .and_then(Value::as_str)
                .unwrap_or("{}");
            Ok(prompt_message(
                "Orient in the project memory",
                format!(
                    "You are starting a session in RMS Memory project `{project}`. Pass `project: \"{project}\"` on every RMS Memory tool call.\n\n\
                     Current overview:\n```json\n{overview}\n```\n\n\
                     1. If an active checkpoint matches the task, load it with `rms_checkpoint_load` and continue from its pending work.\n\
                     2. Before substantial changes, `rms_search` for related decisions and rules, then `rms_read` the relevant hits.\n\
                     3. Note the git branch and changed files above; they show where work stopped.\n\
                     Summarize the state in a few sentences before starting."
                ),
            ))
        }
        "save_progress" => {
            let checkpoint = required("name")?;
            Ok(prompt_message(
                "Save a resumable checkpoint",
                format!(
                    "Save progress in RMS Memory project `{project}`.\n\n\
                     1. Call `rms_checkpoint_save` with `project: \"{project}\"`, `name: \"{checkpoint}\"`, a one-sentence `goal`, concrete `pending` steps a fresh session can follow, and `links` to the vault notes involved.\n\
                     2. If you settled a convention, fixed a tricky bug or made a decision, persist it with `rms_write` (decisions/, rules/ or architecture/) before saving the checkpoint.\n\
                     3. Never include credentials or tokens; RMS Memory refuses them."
                ),
            ))
        }
        "record_decision" => {
            let topic = required("topic")?;
            Ok(prompt_message(
                "Record a technical decision",
                format!(
                    "Record this decision in RMS Memory project `{project}`: {topic}\n\n\
                     1. `rms_search` for existing notes on the topic and `rms_read` the closest hits.\n\
                     2. If a note already covers it, update that note; if the decision replaces an old one, write a new note with `supersedes` set to the old note's id.\n\
                     3. Otherwise write `decisions/<short-slug>.md` with `rms_write`: context, the decision, alternatives considered and consequences. Use `dry_run: true` first if unsure."
                ),
            ))
        }
        _ => bail!("Unknown prompt `{name}`; use session_start, save_progress or record_decision"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_resource_uris() {
        assert_eq!(
            ResourceUri::parse("rms://proj/overview").unwrap(),
            ResourceUri {
                project: "proj".to_string(),
                target: ResourceTarget::Overview
            }
        );
        assert_eq!(
            ResourceUri::parse("rms://proj/note/decisions/My%20Note.md")
                .unwrap()
                .target,
            ResourceTarget::Note("decisions/My Note.md".to_string())
        );
        assert_eq!(
            ResourceUri::parse("rms://proj/checkpoint/release-1.3")
                .unwrap()
                .target,
            ResourceTarget::Checkpoint("release-1.3".to_string())
        );
        for bad in [
            "file:///tmp/x",
            "rms://proj",
            "rms:///overview",
            "rms://proj/note/",
            "rms://proj/unknown",
            "rms://proj/note/%zz",
            "rms://proj/note/%+1",
        ] {
            assert!(ResourceUri::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn encoded_paths_round_trip() {
        let path = "Notes/Café plan #1.md";
        let uri = format!("rms://p/note/{}", percent_encode_path(path));
        assert_eq!(
            ResourceUri::parse(&uri).unwrap().target,
            ResourceTarget::Note(path.to_string())
        );
    }

    #[test]
    fn prompts_declare_required_arguments() {
        let listed = prompts();
        let names: Vec<_> = listed["prompts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|prompt| prompt["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["session_start", "save_progress", "record_decision"]);
        assert_eq!(listed["prompts"][1]["arguments"][0]["required"], true);
    }
}
