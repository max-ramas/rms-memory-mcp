# RMS Memory Roadmap

This document outlines the strategic direction and upcoming milestones for RMS Memory.

## v1.0 — Foundation & Open Source ✅ (Released)

**Goal:** Deliver a stable, easy-to-install product with maximum reach.

- [x] Stable CLI and MCP Server functionality.
- [x] `rms-memory config` interactive wizard.
- [x] Robust Cross-Platform CI (macOS Intel/ARM, Linux x64/ARM64, Windows).
- [x] Publish on `crates.io` (`cargo install rms-memory`).
- [x] Hybrid search algorithms (LanceDB vector + Tantivy FTS).
- [x] Dynamic IDE Auto-Installer (12 IDEs: Claude, Cursor, Zed, OpenCode, VSCode, etc).
- [x] Rules-as-Code IDE Patching (non-destructive AST block injection).
- [x] Linked Documents architecture (zero-copy import with transparent read/write routing).
- [x] Write-Guard snapshotting with rolling `.bak` backups.
- [x] macOS sandbox bypass (`codesign` entitlements).

## v1.0.2 — Security & Features (Released 2026-07-08)

- [x] Path traversal prevention (`..` rejection in read/write).
- [x] LanceDB filter injection protection (escaping).
- [x] `rms-memory doctor` — 5-point vault health diagnostics.
- [x] `rms-memory uninstall` — JSONC-aware IDE config removal with `.bak` backups.
- [x] Graceful shutdown (`SIGINT` handler + `std::process::exit(0)`).
- [x] Zombie process prevention (watcher shutdown signal + explicit exit).
- [x] `llms.txt` export in standard spec format.
- [x] Installer `PayloadBuilder` DI architecture (OpenCode native schema).
- [x] JSONC config parsing with fallback and `tracing::warn!` diagnostics.

## v1.0.3 — Generalization & Audit (Released 2026-07-12)

- [x] Generalized Scope Resolver (`--scope` flag): arbitrary string identifiers beyond filesystem paths.
- [x] Caller Identity Tracking: `last_modified_by` from MCP `clientInfo.name`.
- [x] Audit Metadata: `timestamp`, `created_at`, `confidence`, `source` in YAML frontmatter.
- [x] Confidence-Aware Search: `min_confidence` with NULL-aware filter.
- [x] Zero-Downtime Schema Migration: LanceDB `add_columns(AllNulls)` auto-migration.
- [x] Multi-Scope Documentation: `docs/multi-scope-usage.md`.
- [x] Symlink traversal hardening (canonicalize + containment check).
- [x] Panic-free database layer (12 `unwrap()`/`panic!()` → proper `Result`).
- [x] JSON-RPC error responses (malformed requests return `-32700`).
- [x] Request size limit (1MB) + search limit cap (`min(100)`).
- [x] Code deduplication: `VectorStore` trait removed, `CommandRunner` trait removed, shared `response.rs`/`validation.rs`/`create_vault_dirs()`.

## v1.0.5 — Thread Reduction + Codex (Released 2026-07-13)

**Goal:** Resolve CPU storms and add Codex IDE support.

- [x] Thread pool reduction: ONNX `with_intra_threads(1)` + tokio `worker_threads=2` — per-process thread count from ~45 to ~6.
- [x] Fast-path skip fix: `get_file_timestamps()` returns `(doc_id, timestamp)` — no more silent vector deletion on unchanged files.
- [x] Single `Arc<Mutex<Indexer>>` shared between search and background sync.
- [x] Watcher `.bak` filter + trigger logging.
- [x] Codex IDE: auto-install into `~/.codex/mcp.json`.
- [x] Runtime verified: load avg 648 → 8.31 (-98.7%), CPU 380% → 0%, 3 IDE processes.

## v1.1 — The Workspace Split & Ecosystem (Next)

**Goal:** Transition from a monolithic architecture into a modular ecosystem of crates.

- `rms-memory-core` — Core abstractions, API schemas, and types.
- `rms-memory-vault` — File-system interactions and Vault management.
- `rms-memory-index` — The RAG engine (LanceDB vector + Tantivy FTS).
- `rms-memory-mcp` — Model Context Protocol server implementation.
- `rms-memory-cli` — The end-user CLI application.

This enables downstream consumers to use just `rms-memory-index` without the MCP layer.

## v2.0 — Multi-Vault & Advanced Context (Future)

- Multi-vault routing: one MCP server serving multiple workspaces simultaneously.
- Agentic memory graphs: autonomous summarization, knowledge consolidation, stale context pruning.
- Remote backends: cloud-hosted Vector DBs (managed LanceDB, Qdrant) beyond local `.lancedb`.
