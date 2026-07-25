use serde_json::{Value, json};

pub fn json_text_response(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}]
    })
}

/// MCP result carrying the same payload twice: as `structuredContent` for
/// structured consumers (GUI, typed agents) and as pretty JSON text for
/// plain-text LLM clients.
pub fn json_structured_response<T: serde::Serialize>(payload: &T) -> anyhow::Result<Value> {
    let value = serde_json::to_value(payload)?;
    Ok(json!({
        "content": [{"type": "text", "text": serde_json::to_string_pretty(&value)?}],
        "structuredContent": value
    }))
}
