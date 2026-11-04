use super::AppContext;
use anyhow::Result;

pub async fn execute(
    ctx: &AppContext,
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Value> {
    // `noPromote` is reserved for read-without-side-effects. Vault reads are
    // already side-effect free (no access counters); the flag is accepted so
    // agents/clients can request that contract explicitly and stay forward-compatible.
    let _no_promote = args
        .get("noPromote")
        .or_else(|| args.get("no_promote"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let workspace_root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let path_str = args.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let file_path = super::validation::resolve_vault_path(workspace_root, path_str)?;

    // If this document is a link, fetch the linked content — but only when the
    // resolved target stays inside the vault. Escapes fall through to reading
    // the link file itself instead of leaking data from outside the vault.
    let full_text = if let Some(linked_content) = crate::link::get_linked_content_in_vault(
        &file_path,
        workspace_root,
        ctx.code_path.as_deref(),
    ) {
        linked_content
    } else {
        let store = ctx
            .store
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Store not initialized"))?;
        store.read_document(path_str).await?
    };
    render(path_str, &full_text, &ReadOptions::from_args(args))
}

/// Options that shrink what `rms_read` returns. With none set the whole note is returned.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReadOptions {
    pub outline: bool,
    pub heading: Option<String>,
    pub max_chars: Option<usize>,
    pub offset: usize,
    pub if_hash: Option<String>,
}

impl ReadOptions {
    pub fn from_args(args: &serde_json::Map<String, serde_json::Value>) -> Self {
        let num = |key: &str| args.get(key).and_then(|v| v.as_u64()).map(|n| n as usize);
        let text = |key: &str| {
            args.get(key)
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        Self {
            outline: args
                .get("outline")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
            heading: text("heading"),
            max_chars: num("max_chars"),
            offset: num("offset").unwrap_or(0),
            if_hash: text("if_hash"),
        }
    }
}

pub fn render(path: &str, full_text: &str, options: &ReadOptions) -> Result<serde_json::Value> {
    use super::doc_sections as sections;
    let hash = sections::content_hash(full_text);
    // The caller names the version it already holds; resend nothing if it is unchanged.
    if let Some(known) = options.if_hash.as_deref()
        && known == hash
    {
        return Ok(serde_json::json!({
            "content": [{"type": "text", "text": format!(
                "unchanged: {path} is identical to the version you already read (hash {hash}); content not resent. Omit `if_hash` to read it again."
            )}],
            "structuredContent": {"path": path, "content_hash": hash, "unchanged": true}
        }));
    }
    let total_chars = full_text.chars().count();
    let (view, next_offset) = if options.outline {
        (sections::outline(path, full_text, &hash), None)
    } else {
        let selected = match options.heading.as_deref() {
            Some(heading) => sections::section_text(full_text, heading)?,
            None => full_text,
        };
        sections::window(selected, options.offset, options.max_chars)
    };
    let mut text = view;
    if let Some(next) = next_offset {
        text.push_str(&format!(
            "\n\n[truncated: continue with offset={next}; {total_chars} chars in the whole note]"
        ));
    }
    Ok(serde_json::json!({
        "content": [{"type": "text", "text": text}],
        "structuredContent": {
            "path": path,
            "content_hash": hash,
            "chars": total_chars,
            "truncated": next_offset.is_some(),
        }
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "# A\none\n# B\ntwo\n";

    fn text(v: &serde_json::Value) -> String {
        v["content"][0]["text"].as_str().unwrap().to_string()
    }

    #[test]
    fn default_read_is_the_whole_note_plus_hash() {
        let v = render("n.md", DOC, &ReadOptions::default()).unwrap();
        assert_eq!(text(&v), DOC);
        assert_eq!(
            v["structuredContent"]["content_hash"]
                .as_str()
                .unwrap()
                .len(),
            16
        );
        assert_eq!(v["structuredContent"]["truncated"], false);
    }

    #[test]
    fn if_hash_returns_a_stub_only_for_the_identical_version() {
        let first = render("n.md", DOC, &ReadOptions::default()).unwrap();
        let hash = first["structuredContent"]["content_hash"]
            .as_str()
            .unwrap()
            .to_string();
        let same = render(
            "n.md",
            DOC,
            &ReadOptions {
                if_hash: Some(hash.clone()),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(text(&same).starts_with("unchanged:"));
        assert!(text(&same).len() < DOC.len() + 200);
        assert_eq!(same["structuredContent"]["unchanged"], true);
        let changed = render(
            "n.md",
            "# A\nnew\n",
            &ReadOptions {
                if_hash: Some(hash),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(text(&changed), "# A\nnew\n");
    }

    #[test]
    fn heading_outline_and_window_options() {
        let sec = render(
            "n.md",
            DOC,
            &ReadOptions {
                heading: Some("B".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(text(&sec), "# B\ntwo\n");
        assert!(
            render(
                "n.md",
                DOC,
                &ReadOptions {
                    heading: Some("zzz".into()),
                    ..Default::default()
                }
            )
            .is_err()
        );
        let outline = render(
            "n.md",
            DOC,
            &ReadOptions {
                outline: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(text(&outline).contains("2 section(s)"));
        let win = render(
            "n.md",
            DOC,
            &ReadOptions {
                max_chars: Some(4),
                ..Default::default()
            },
        )
        .unwrap();
        assert!(text(&win).starts_with("# A\n"));
        assert!(text(&win).contains("continue with offset=4"));
        assert_eq!(win["structuredContent"]["truncated"], true);
    }

    #[test]
    fn options_parse_from_arguments() {
        let args = serde_json::json!({"outline": true, "heading": " X ", "max_chars": 10, "offset": 3, "if_hash": "abc"});
        let o = ReadOptions::from_args(args.as_object().unwrap());
        assert_eq!(
            o,
            ReadOptions {
                outline: true,
                heading: Some("X".into()),
                max_chars: Some(10),
                offset: 3,
                if_hash: Some("abc".into())
            }
        );
        assert_eq!(
            ReadOptions::from_args(&serde_json::Map::new()),
            ReadOptions::default()
        );
    }
}
