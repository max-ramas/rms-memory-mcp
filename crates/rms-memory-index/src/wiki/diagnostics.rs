use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Diagnostics {
    pub skipped_sources: Vec<String>,
    pub stale_index: bool,
    pub budget_exceeded: Vec<String>,
    pub read_errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl Diagnostics {
    pub fn add_skip(&mut self, source: &str) {
        self.skipped_sources.push(source.to_string());
    }

    pub fn add_error(&mut self, source: &str, error: &str) {
        self.read_errors.push(format!("{source}: {error}"));
    }

    pub fn add_warning(&mut self, msg: &str) {
        self.warnings.push(msg.to_string());
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn to_map(&self) -> HashMap<String, serde_json::Value> {
        serde_json::to_value(self)
            .and_then(serde_json::from_value)
            .unwrap_or_default()
    }
}
