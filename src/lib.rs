pub mod code_indexer;
pub mod code_parser;
pub mod config_manager;
/// RMS Memory MCP Server
///
/// This is the core library for RMS Memory, exposing its internal logic
/// for use in other applications. Currently, it acts as a monolith containing
/// the MCP server, CLI, Vault logic, and the Vector/Graph Indexer.
/// In future versions (v1.1+), these will be split into a proper Cargo Workspace.
// Public API
pub mod document;
pub mod document_service;
pub mod graph;
pub mod graph_store;
pub mod index_lock;
pub mod indexer;
pub mod jobs;
pub mod retrieval;
pub mod semantic_graph;
pub mod store;
pub mod tools;
pub mod vault_graph;
pub mod wiki;
pub mod workspace;

// Internal modules (hidden from docs.rs but available to the binary)
#[doc(hidden)]
pub mod cli;
#[doc(hidden)]
pub mod commands;
#[doc(hidden)]
pub mod import;
#[doc(hidden)]
pub mod installer;
#[doc(hidden)]
pub mod link;
#[doc(hidden)]
pub mod mcp_server;
#[doc(hidden)]
pub mod rules_injector;
