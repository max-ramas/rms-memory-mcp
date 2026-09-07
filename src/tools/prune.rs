use super::AppContext;
use anyhow::Result;
use rms_memory_vault::prune::{PruneOptions, prune_vault};
use serde::Deserialize;
use serde_json::{Map, Value};

#[derive(Debug, Default, Deserialize)]
struct PruneArgs {
    older_than_days: Option<u32>,
    apply: Option<bool>,
    #[allow(dead_code)]
    project: Option<String>,
}

fn options_from_args(args: &Map<String, Value>) -> Result<PruneOptions> {
    let args: PruneArgs = serde_json::from_value(Value::Object(args.clone()))?;
    Ok(PruneOptions {
        older_than_days: args.older_than_days.unwrap_or(30),
        apply: args.apply.unwrap_or(false),
    })
}

pub async fn execute(ctx: &AppContext, args: &Map<String, Value>) -> Result<Value> {
    let root = ctx
        .workspace_root
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Workspace root not initialized"))?;
    let report = prune_vault(root, options_from_args(args)?)?;
    super::response::json_structured_response(&report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_to_thirty_day_dry_run() {
        let options = options_from_args(&Map::new()).unwrap();
        assert_eq!(options, PruneOptions::default());
    }

    #[test]
    fn accepts_explicit_options_and_project() {
        let args = json!({
            "older_than_days": 7,
            "apply": true,
            "project": "rms-memory-mcp"
        })
        .as_object()
        .unwrap()
        .clone();
        let options = options_from_args(&args).unwrap();
        assert_eq!(
            options,
            PruneOptions {
                older_than_days: 7,
                apply: true,
            }
        );
    }
}
