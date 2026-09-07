//! Cycle-free CLI command implementations.
//!
//! The `rms-memory` executable, clap command tree (`src/cli.rs` /
//! `src/commands/`), MCP stdio server, `serve`, and installer remain in the
//! umbrella package `rms-memory-mcp`. Commands that depend only on lower-level
//! workspace crates can live here without creating a Cargo cycle.
//!
//! ## Current contract
//!
//! - `gc` and `prune` are implemented here and re-exported by the umbrella.
//! - `publish = false` — never upload to crates.io (umbrella-only publish).
//! - Do **not** add a second binary named `rms-memory` here.

pub mod gc;
pub mod prune;
