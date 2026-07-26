//! Reserved CLI workspace boundary.
//!
//! The executable modules remain in the umbrella crate during Phase 1 because
//! `serve` dispatches directly into the local MCP server. Moving them here
//! would introduce a Cargo dependency cycle.
