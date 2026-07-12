use serde_json::{Value, json};

pub fn json_text_response(text: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": text}]
    })
}

pub fn json_data_response(data: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": data}]
    })
}
