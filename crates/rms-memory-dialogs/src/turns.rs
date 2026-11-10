//! Turn extraction: pulls what a person and an assistant actually said out of the archived
//! logical files of each tool. Tool output, reasoning, and system injections are left out,
//! so the search index stays about decisions and not about noise.

use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Turn {
    /// "user", "assistant" or "note" (a plan/notes file)
    pub role: &'static str,
    pub ts: Option<String>,
    pub text: String,
}

/// Longest text kept per turn; a pasted file or log is not a decision.
const MAX_TURN_CHARS: usize = 50_000;

fn push(out: &mut Vec<Turn>, role: &'static str, ts: Option<String>, text: &str) {
    let t = text.trim();
    if t.is_empty() {
        return;
    }
    let text = if t.chars().count() > MAX_TURN_CHARS {
        t.chars().take(MAX_TURN_CHARS).collect()
    } else {
        t.to_owned()
    };
    out.push(Turn { role, ts, text });
}

fn lines(data: &[u8]) -> impl Iterator<Item = Value> + '_ {
    data.split(|b| *b == b'\n')
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_slice::<Value>(l).ok())
}

fn s(v: &Value) -> Option<String> {
    v.as_str().map(str::to_owned)
}

/// A whole message that is one tag block (`<environment_context>…</environment_context>`) is injected context.
fn is_injected(text: &str) -> bool {
    let t = text.trim();
    let Some(rest) = t.strip_prefix('<') else {
        return false;
    };
    let Some(end) = rest.find(['>', ' ']) else {
        return false;
    };
    let name = &rest[..end];
    !name.is_empty() && !name.starts_with('/') && t.ends_with(&format!("</{name}>"))
}

fn join_text_blocks(content: &Value, key: &str) -> String {
    match content {
        Value::String(t) => t.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|b| match b {
                Value::String(t) => Some(t.as_str()),
                _ => b.get(key).and_then(Value::as_str),
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn claude(data: &[u8]) -> Vec<Turn> {
    let mut out = Vec::new();
    for l in lines(data) {
        let role = match l["type"].as_str() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        if l["isMeta"].as_bool() == Some(true) {
            continue;
        }
        let m = &l["message"];
        let text = match &m["content"] {
            Value::Array(items) => items
                .iter()
                .filter(|b| b["type"].as_str() == Some("text"))
                .filter_map(|b| b["text"].as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            other => join_text_blocks(other, "text"),
        };
        if role == "user" && is_injected(&text) {
            continue;
        }
        push(&mut out, role, s(&l["timestamp"]), &text);
    }
    out
}

fn codex(data: &[u8]) -> Vec<Turn> {
    let mut out = Vec::new();
    for l in lines(data) {
        if l["type"].as_str() != Some("response_item")
            || l["payload"]["type"].as_str() != Some("message")
        {
            continue;
        }
        let role = match l["payload"]["role"].as_str() {
            Some("user") => "user",
            Some("assistant") => "assistant",
            _ => continue,
        };
        let text = join_text_blocks(&l["payload"]["content"], "text");
        if role == "user" && (is_injected(&text) || text.trim_start().starts_with("# AGENTS.md")) {
            continue;
        }
        push(&mut out, role, s(&l["timestamp"]), &text);
    }
    out
}

fn antigravity(name: &str, data: &[u8]) -> Vec<Turn> {
    let mut out = Vec::new();
    if name.ends_with(".md") {
        push(&mut out, "note", None, &String::from_utf8_lossy(data));
        return out;
    }
    if !name.ends_with(".jsonl") {
        return out; // trajectory databases are binary snapshots
    }
    for l in lines(data) {
        let (role, text) = match (l["source"].as_str(), l["type"].as_str()) {
            (Some("USER_EXPLICIT"), Some("USER_INPUT")) => {
                let t = l["content"].as_str().unwrap_or_default();
                let t = t.split_once("<USER_REQUEST>").map_or(t, |(_, r)| r);
                (
                    "user",
                    t.split("</USER_REQUEST>")
                        .next()
                        .unwrap_or_default()
                        .to_owned(),
                )
            }
            (Some("MODEL"), Some("PLANNER_RESPONSE")) => (
                "assistant",
                l["content"].as_str().unwrap_or_default().to_owned(),
            ),
            _ => continue,
        };
        push(&mut out, role, s(&l["created_at"]), &text);
    }
    out
}

fn parse_embedded(v: &Value) -> Value {
    match v {
        Value::String(t) => serde_json::from_str(t).unwrap_or(Value::Null),
        other => other.clone(),
    }
}

fn opencode(data: &[u8]) -> Vec<Turn> {
    let mut order: Vec<(String, &'static str, Option<String>)> = Vec::new();
    let mut parts: HashMap<String, Vec<String>> = HashMap::new();
    for l in lines(data) {
        let row = &l["row"];
        match l["table"].as_str() {
            Some("message") => {
                let d = parse_embedded(&row["data"]);
                let role = match d["role"].as_str() {
                    Some("user") => "user",
                    Some("assistant") => "assistant",
                    _ => continue,
                };
                let ts = row["time_created"].as_i64().map(|t| t.to_string());
                if let Some(id) = row["id"].as_str() {
                    order.push((id.to_owned(), role, ts));
                }
            }
            Some("part") => {
                let d = parse_embedded(&row["data"]);
                if d["type"].as_str() == Some("text")
                    && let (Some(mid), Some(t)) = (row["message_id"].as_str(), d["text"].as_str())
                {
                    parts.entry(mid.to_owned()).or_default().push(t.to_owned());
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    for (id, role, ts) in order {
        if let Some(p) = parts.get(&id) {
            push(&mut out, role, ts, &p.join("\n"));
        }
    }
    out
}

fn cursor(data: &[u8]) -> Vec<Turn> {
    let mut order: Vec<String> = Vec::new();
    let mut bubbles: Vec<(String, &'static str, Option<String>, String)> = Vec::new();
    for l in lines(data) {
        let key = l["key"].as_str().unwrap_or_default();
        let v = parse_embedded(&l["value"]);
        if key.starts_with("composerData:") {
            if let Some(h) = v["fullConversationHeadersOnly"].as_array() {
                order = h
                    .iter()
                    .filter_map(|b| b["bubbleId"].as_str().map(str::to_owned))
                    .collect();
            }
        } else if let Some(id) = key
            .rsplit(':')
            .next()
            .filter(|_| key.starts_with("bubbleId:"))
        {
            let role = match v["type"].as_i64() {
                Some(1) => "user",
                Some(2) => "assistant",
                _ => continue,
            };
            bubbles.push((
                id.to_owned(),
                role,
                s(&v["createdAt"]),
                v["text"].as_str().unwrap_or_default().to_owned(),
            ));
        }
    }
    let pos: HashMap<&str, usize> = order
        .iter()
        .enumerate()
        .map(|(i, b)| (b.as_str(), i))
        .collect();
    bubbles.sort_by(
        |a, b| match (pos.get(a.0.as_str()), pos.get(b.0.as_str())) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.2.cmp(&b.2),
        },
    );
    let mut out = Vec::new();
    for (_, role, ts, text) in bubbles {
        push(&mut out, role, ts, &text);
    }
    out
}

fn zed(data: &[u8]) -> Vec<Turn> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_slice::<Value>(data) else {
        return out;
    };
    let ts = s(&v["updated_at"]);
    for m in v["thread"]["messages"].as_array().into_iter().flatten() {
        for (key, role) in [("User", "user"), ("Agent", "assistant")] {
            if let Some(msg) = m.get(key) {
                let text = msg["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["Text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                push(&mut out, role, ts.clone(), &text);
            }
        }
    }
    out
}

/// Epoch milliseconds as an RFC 3339 UTC timestamp (no date crate needed).
fn rfc3339_from_ms(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// DeepSeek Harness: ~/.dsh/sessions/**/session.v4.jsonl.zstd, already decompressed to JSONL events.
/// Only what a person typed (`user/message` with source kind `user`) and the assistant's `text` blocks are kept;
/// runtime context, skill catalogs, reasoning and tool calls are left out.
fn deepseek(data: &[u8]) -> Vec<Turn> {
    let mut out = Vec::new();
    for v in lines(data) {
        let ts = v["time"].as_i64().map(rfc3339_from_ms);
        let d = &v["data"];
        match v["type"].as_str() {
            Some("user/message") if d["source"]["kind"].as_str() == Some("user") => {
                push(
                    &mut out,
                    "user",
                    ts,
                    &join_text_blocks(&d["content"], "text"),
                );
            }
            Some("assistant/message") => {
                let text = d["message"]["content"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|b| b["type"].as_str() == Some("text"))
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                push(&mut out, "assistant", ts, &text);
            }
            _ => {}
        }
    }
    out
}

/// Turns of one archived logical file. `tool` is the archive folder name, `name` the logical file name.
pub fn extract(tool: &str, name: &str, data: &[u8]) -> Vec<Turn> {
    match tool {
        "claude-code" => claude(data),
        "codex" => codex(data),
        "antigravity" => antigravity(name, data),
        "opencode" => opencode(data),
        "cursor" => cursor(data),
        "zed" => zed(data),
        "deepseek-harness" => deepseek(data),
        _ => Vec::new(),
    }
}

/// A readable Markdown view of a session: small front matter, then one heading per turn.
pub fn to_markdown(tool: &str, project: &str, name: &str, turns: &[Turn]) -> String {
    let mut md = format!(
        "---\ntool: {tool}\nproject: {project}\nsession: {name}\nturns: {}\n---\n\n",
        turns.len()
    );
    for t in turns {
        md.push_str(&format!(
            "## {}{}\n\n{}\n\n",
            t.role,
            t.ts.as_deref()
                .map(|x| format!(" · {x}"))
                .unwrap_or_default(),
            t.text
        ));
    }
    md
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_keeps_text_only() {
        let d = br#"{"type":"user","message":{"role":"user","content":"hello there"},"timestamp":"t1"}
{"type":"assistant","message":{"content":[{"type":"thinking","thinking":"x"},{"type":"text","text":"answer"},{"type":"tool_use","name":"Bash"}]}}
{"type":"user","message":{"content":[{"type":"tool_result","content":"out"}]}}
{"type":"attachment"}
"#;
        let t = claude(d);
        assert_eq!(t.len(), 2);
        assert_eq!((t[0].role, t[0].text.as_str()), ("user", "hello there"));
        assert_eq!((t[1].role, t[1].text.as_str()), ("assistant", "answer"));
    }

    #[test]
    fn codex_skips_injected_and_developer() {
        let d = br#"{"type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"rules"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>x</environment_context>"}]}}
{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"real question"}]}}
{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"reply"}]}}
"#;
        let t = codex(d);
        assert_eq!(
            t.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
            ["real question", "reply"]
        );
    }

    #[test]
    fn opencode_groups_parts_by_message() {
        let d =
            br#"{"table":"message","row":{"id":"m1","time_created":1,"data":"{\"role\":\"user\"}"}}
{"table":"message","row":{"id":"m2","time_created":2,"data":"{\"role\":\"assistant\"}"}}
{"table":"part","row":{"message_id":"m1","data":"{\"type\":\"text\",\"text\":\"why?\"}"}}
{"table":"part","row":{"message_id":"m2","data":"{\"type\":\"tool\",\"text\":\"skip\"}"}}
{"table":"part","row":{"message_id":"m2","data":"{\"type\":\"text\",\"text\":\"because\"}"}}
"#;
        let t = opencode(d);
        assert_eq!(
            t.iter()
                .map(|x| (x.role, x.text.as_str()))
                .collect::<Vec<_>>(),
            [("user", "why?"), ("assistant", "because")]
        );
    }

    #[test]
    fn cursor_orders_by_conversation_headers() {
        let d = br#"{"key":"composerData:c","value":"{\"fullConversationHeadersOnly\":[{\"bubbleId\":\"b\"},{\"bubbleId\":\"a\"}]}"}
{"key":"bubbleId:c:a","value":"{\"type\":2,\"text\":\"second\"}"}
{"key":"bubbleId:c:b","value":"{\"type\":1,\"text\":\"first\"}"}
"#;
        let t = cursor(d);
        assert_eq!(
            t.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
            ["first", "second"]
        );
    }

    #[test]
    fn zed_and_antigravity() {
        let z = br#"{"updated_at":"u","thread":{"messages":[{"User":{"content":[{"Text":"q"}]}},{"Agent":{"content":[{"Text":"a"},{"ToolUse":{}}]}},"Resume"]}}"#;
        assert_eq!(zed(z).len(), 2);
        let a = br#"{"source":"USER_EXPLICIT","type":"USER_INPUT","content":"<USER_REQUEST>\ndo it\n</USER_REQUEST>\n<ADDITIONAL_METADATA>m</ADDITIONAL_METADATA>"}
{"source":"MODEL","type":"PLANNER_RESPONSE","content":"done"}
"#;
        let t = antigravity("t.jsonl", a);
        assert_eq!(
            t.iter().map(|x| x.text.as_str()).collect::<Vec<_>>(),
            ["do it", "done"]
        );
        assert!(antigravity("x.db", b"bin").is_empty());
    }

    #[test]
    fn deepseek_keeps_people_and_answers_only() {
        let d = br#"{"type":"session","id":"s"}
{"type":"user/message","time":1794276539388,"data":{"source":{"kind":"user"},"content":[{"type":"text","text":"hello"}]}}
{"type":"user/message","time":1794276539389,"data":{"source":{"kind":"runtime-context"},"content":[{"type":"text","text":"Current runtime context"}]}}
{"type":"assistant/message","time":1794276540000,"data":{"message":{"content":[{"type":"reasoning","text":"thinking"},{"type":"text","text":"answer"},{"type":"tool_use","name":"bash"}]}}}
{"type":"tool/result","data":{}}
"#;
        let t = deepseek(d);
        assert_eq!(
            t.iter()
                .map(|x| (x.role, x.text.as_str()))
                .collect::<Vec<_>>(),
            [("user", "hello"), ("assistant", "answer")]
        );
        assert_eq!(t[0].ts.as_deref(), Some("2026-11-10T02:08:59Z"));
        assert!(extract("deepseek-harness", "s.jsonl", d).len() == 2);
    }

    #[test]
    fn rfc3339_handles_epoch_and_leap_days() {
        assert_eq!(rfc3339_from_ms(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339_from_ms(951_782_400_000), "2000-02-29T00:00:00Z");
        assert_eq!(rfc3339_from_ms(1_709_164_800_000), "2024-02-29T00:00:00Z");
    }
}
