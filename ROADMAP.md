# RMS Memory Roadmap

This document outlines the strategic direction and upcoming milestones for RMS Memory.

## v1.0 - Foundation & Open Source (Current Focus)
**Goal:** Deliver a stable, easy-to-install product with maximum reach on crates.io and GitHub.

- [x] Stable CLI and MCP Server functionality.
- [x] `rms-memory config` interactive wizard.
- [x] Robust Cross-Platform CI (macOS Intel/ARM, Linux x64/ARM64, Windows).
- [x] Publish on `crates.io` (`cargo install rms-memory`).
- [ ] Auto-generated docs on `docs.rs`.
- [ ] Stabilize core hybrid search algorithms (LanceDB + FastEmbed + Tantivy).

## v1.1 - The Workspace Split & Ecosystem
**Goal:** Transition from a monolithic architecture into a modular ecosystem of crates.

We plan to divide the project into a Cargo Workspace to allow developers to use individual components:

* `rms-memory-core` — Core abstractions, API schemas, and types.
* `rms-memory-vault` — File-system interactions and Vault management.
* `rms-memory-index` — The RAG engine (LanceDB vector search + Tantivy full-text + Graph relations).
* `rms-memory-mcp` — Model Context Protocol server implementation.
* `rms-memory-cli` — The end-user CLI application.

This will allow users to, for example, build their own AI applications utilizing just the `rms-memory-index` crate without needing the MCP layer.

## v2.0 - Multi-Vault & Advanced Graph Context
**Goal:** Support complex workflows and massive codebases.

- Multi-vault routing: allow the MCP server to serve multiple disjoint workspaces simultaneously.
- Advanced Agentic Memory graphs: autonomous memory summarization, knowledge consolidation, and automated forgetting of stale context.
- Remote backends: support for cloud-hosted Vector DBs (e.g. managed LanceDB, Qdrant) instead of purely local `.lancedb` files.
