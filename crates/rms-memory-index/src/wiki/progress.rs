use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WikiPhase {
    Resolving,
    Retrieving {
        section: String,
        items_done: usize,
        items_total: usize,
    },
    Budgeting,
    Packaging,
    Complete,
    Failed(String),
}

impl fmt::Display for WikiPhase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WikiPhase::Resolving => write!(f, "Resolving manifest"),
            WikiPhase::Retrieving {
                section,
                items_done,
                items_total,
            } => {
                write!(f, "Retrieving [{section}]: {items_done}/{items_total}")
            }
            WikiPhase::Budgeting => write!(f, "Allocating budget"),
            WikiPhase::Packaging => write!(f, "Packaging output"),
            WikiPhase::Complete => write!(f, "Complete"),
            WikiPhase::Failed(msg) => write!(f, "Failed: {msg}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiEvent {
    pub phase: WikiPhase,
    pub message: Option<String>,
}

impl WikiEvent {
    pub fn new(phase: WikiPhase) -> Self {
        Self {
            phase,
            message: None,
        }
    }

    pub fn with_message(mut self, msg: impl Into<String>) -> Self {
        self.message = Some(msg.into());
        self
    }
}
