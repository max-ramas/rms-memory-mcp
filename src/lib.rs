/// RMS Memory MCP Server
///
/// This is the umbrella library for RMS Memory. Its re-exports preserve the
/// original `rms_memory_mcp::<module>` API while implementation lives in
/// focused workspace crates.
/// Workspace crates (core / index / vault / cli) live under `crates/`; this
/// package remains the crates.io umbrella with stable `rms_memory_mcp::<mod>` paths.
// Public API
pub use rms_memory_core::{audit, config_manager, document, link, path_policy, workspace};
pub use rms_memory_index::{
    code_indexer, code_parser, graph, graph_store, index_lock, indexer, jobs, retrieval,
    semantic_graph, store, vault_graph, wiki,
};
pub use rms_memory_vault::{document_service, import, project_migrate, project_service};

pub mod tools;

/// In-process `rms-memory --help` text for Wiki `self_cli_help` sources.
/// Prefer this over shelling out so GUI / library hosts work without a PATH binary.
pub fn render_cli_help(subcommand: &str) -> String {
    use clap::CommandFactory;
    let mut app = cli::Cli::command();
    if subcommand.is_empty() {
        return app.render_help().to_string();
    }
    let parts: Vec<&str> = subcommand.split_whitespace().collect();
    for part in &parts[..parts.len().saturating_sub(1)] {
        if let Some(cmd) = app.find_subcommand(part) {
            app = cmd.clone();
        }
    }
    if let Some(cmd) = app.find_subcommand(parts.last().unwrap_or(&"")) {
        cmd.clone().render_help().to_string()
    } else {
        app.render_help().to_string()
    }
}

// Internal modules (hidden from docs.rs but available to the binary)
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod commands;
#[doc(hidden)]
pub mod installer;
#[doc(hidden)]
pub mod mcp_server;
#[doc(hidden)]
pub mod rules_injector;
