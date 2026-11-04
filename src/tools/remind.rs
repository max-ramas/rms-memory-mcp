//! `rms_remind`: global reminders delivered through the agent's replies (see `crate::reminders`).

use crate::reminders::{Status, Store, resolve_due};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde_json::{Map, Value, json};

fn view(r: &crate::reminders::Reminder) -> Value {
    json!({
        "id": r.id,
        "text": r.text,
        "due_at_utc": r.due_at.to_rfc3339(),
        "status": match r.status { Status::Pending => "pending", Status::Done => "done", Status::Cancelled => "cancelled" },
    })
}

pub fn execute(args: &Map<String, Value>) -> Result<Value> {
    execute_with(&Store::default_location(), args, Utc::now())
}

pub fn execute_with(
    store: &Store,
    args: &Map<String, Value>,
    now: chrono::DateTime<Utc>,
) -> Result<Value> {
    let action = args.get("action").and_then(Value::as_str).unwrap_or("list");
    let text = args.get("text").and_then(Value::as_str);
    let id = args.get("id").and_then(Value::as_str);
    let due_at = args.get("due_at").and_then(Value::as_str);
    let in_minutes = args.get("in_minutes").and_then(Value::as_i64);
    let need_id = || id.context("`id` is required for this action (see action=list)");
    let payload = match action {
        "add" => {
            let text = text.context("`text` is required for add")?;
            let reminder = store.add(text, resolve_due(due_at, in_minutes, now)?, now)?;
            json!({"added": view(&reminder),
                   "note": "You will be told about it ~30/20/10/5 minutes before it is due, in tool responses."})
        }
        "list" => {
            json!({"now_utc": now.to_rfc3339(),
                   "pending": store.pending().iter().map(view).collect::<Vec<_>>()})
        }
        "done" => json!({"done": view(&store.done(need_id()?)?)}),
        "cancel" => json!({"cancelled": view(&store.cancel(need_id()?)?)}),
        "snooze" => {
            json!({"rescheduled": view(&store.reschedule(need_id()?, resolve_due(due_at, in_minutes, now)?, now)?)})
        }
        other => bail!("unknown action `{other}` (add | list | done | cancel | snooze)"),
    };
    super::response::json_structured_response(&payload)
}

/// Append a reminder notice to any tool result when one is due.
pub fn attach_notices(mut result: Value) -> Value {
    attach_with(&Store::default_location(), Utc::now(), &mut result);
    result
}

fn attach_with(store: &Store, now: chrono::DateTime<Utc>, result: &mut Value) {
    if let Some(block) = crate::reminders::notice_block(store, now)
        && let Some(content) = result.get_mut("content").and_then(Value::as_array_mut)
    {
        content.push(json!({"type": "text", "text": block}));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn add_list_done_flow_and_attached_notice() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::at(dir.path());
        let now = chrono::DateTime::parse_from_rfc3339("2026-11-05T08:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let added = execute_with(
            &store,
            &args(json!({"action": "add", "text": "call Viktor Petrovich", "due_at": "2026-11-05T11:25:00+03:00"})),
            now,
        )
        .unwrap();
        let id = added["structuredContent"]["added"]["id"]
            .as_str()
            .unwrap()
            .to_string();

        let mut tool_result = json!({"content": [{"type": "text", "text": "ok"}]});
        attach_with(&store, now, &mut tool_result);
        let blocks = tool_result["content"].as_array().unwrap();
        assert_eq!(blocks.len(), 2);
        assert!(
            blocks[1]["text"]
                .as_str()
                .unwrap()
                .contains("Viktor Petrovich")
        );

        let mut again = json!({"content": [{"type": "text", "text": "ok"}]});
        attach_with(&store, now, &mut again);
        assert_eq!(again["content"].as_array().unwrap().len(), 1, "fires once");

        execute_with(&store, &args(json!({"action": "done", "id": id})), now).unwrap();
        let listed = execute_with(&store, &args(json!({})), now).unwrap();
        assert!(
            listed["structuredContent"]["pending"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        assert!(execute_with(&store, &args(json!({"action": "nope"})), now).is_err());
        assert!(execute_with(&store, &args(json!({"action": "done"})), now).is_err());
    }
}
