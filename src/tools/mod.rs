pub mod read;
pub mod response;
pub mod search;
pub mod validation;
pub mod write;

use crate::indexer::Indexer;
use std::sync::Arc;
use tokio::sync::Mutex;

pub struct AppContext {
    pub store: Option<crate::store::Store>,
    pub indexer: Option<Arc<Mutex<Indexer>>>,
    pub workspace_root: Option<std::path::PathBuf>,
    pub max_backups: usize,
    /// None = auto-detect scope from rootUri at connect time.  
    /// Some(s) = explicit scope from --scope CLI flag.
    pub scope: Option<String>,
    pub caller_id: String,
}
