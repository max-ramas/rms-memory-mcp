use serde_json::{Value, json};

pub fn json_text_response(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}]
    })
}
