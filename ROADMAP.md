# RMS Memory Roadmap

This document outlines the strategic direction and upcoming milestones for RMS Memory.

**Current (2026-07-23):** MCP **`1.0.6`** · companion GUI **`1.0.0`**.

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
- [x] Write-Guard snapshotting with rolling `.bak` backups and fsync-backed atomic Markdown replacement for `create`/`replace`.
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
- [x] Multi-Scope Documentation: public usage guidance is maintained in `README.md`.
- [x] Symlink traversal hardening (canonicalize + containment check).
- [x] Panic-free database layer (12 `unwrap()`/`panic!()` → proper `Result`).
- [x] JSON-RPC error responses (malformed requests return `-32700`).
- [x] Request size limit (1MB) + search limit cap (`min(100)`).
- [x] Code deduplication: `VectorStore` trait removed, `CommandRunner` trait removed, shared `response.rs`/`validation.rs`/`create_vault_dirs()`.

## v1.0.5 — Stabilization + Semantic Code Memory (Released 2026-07-13)

**Goal:** Keep multi-IDE operation idle when nothing changes, then add a safe, optional semantic code corpus without weakening Markdown memory.

- [x] Thread pool reduction: ONNX `with_intra_threads(1)` + tokio `worker_threads=2` — per-process thread count from ~45 to ~6.
- [x] Fast-path skip fix: `get_file_timestamps()` returns `(doc_id, timestamp)` — no more silent vector deletion on unchanged files.
- [x] Single `Arc<Mutex<Indexer>>` shared between search and background sync.
- [x] Watcher `.bak` filter + trigger logging.
- [x] Codex IDE: auto-install into `~/.codex/mcp.json`.
- [x] Runtime verified: load avg 648 → 8.31 (-98.7%), CPU 380% → 0%, 3 IDE processes.
- [x] PID-aware per-project writer lock, fail-closed frontmatter parsing, and read-only background sync.
- [x] Tree-sitter semantic chunks for Rust, Go, JS/JSX, TS/TSX, Python, C/C++, Java, Ruby, Swift, and inline Vue scripts, with stable IDs, vector reuse, language metadata, and preamble-preserving segmentation.
- [x] Explicit `reindex --vault|--code|--all`; code indexing remains opt-in through `code_index_mode = off|manual|watch` (default `off`).
- [x] Derived graph nodes/edges plus durable manual edges and suppress/restore overrides, separated from retrieval chunks.
- [x] Federated `rms_search(corpus=vault|code|all)` and `rms_code_search`; mixed results use Reciprocal Rank Fusion.
- [x] Opt-in multilingual watcher: three-second coalescing, project language allow-lists, and shared completion markers prevent duplicate reindexing across IDE processes.
- [x] Live MCP JSON-RPC probe: `rms_search(corpus=vault|code|all)` and `rms_code_search` returned expected corpus provenance and RRF output.
- [x] Five-server watch soak: rapid Rust saves advanced one shared completion marker; all test servers returned to 0.0% idle CPU.
- [x] Legacy memory integrity repair: explicit doctor repair assigned UUIDs to 14 valid ID-less records with backups.
- [x] Real-project stress gate: concurrent GeoMail, License Server, RMS Monitoring, and GeoTax Site indexing completed; four restarted IDEs remained at 0.0% idle CPU.
- [x] Independent Rust and mixed Rust/Tauri dogfood: `rms-threads-assistant` (19 files / 101 items / 133 reused segments) and `rms-monitoring` (114 / 846 / 976 reused) completed on the final binary.
- [ ] Optional scale-up: a separate larger Rust workspace remains unavailable locally. One unrelated invalid-YAML vault record remains intentionally manual-only.

## v1.0.6 — Wiki Generator + Project Provenance + Isolation + Path-Scoped Watch (Released / line closed 2026-07-23)

**Goal:** Deterministic context pack assembly, project identity tracking, ChatGPT/Codex integration, Wiki write/index isolation, and incremental code watcher reindex.

- [x] Wiki Context Pack Generator: `rms-memory wiki generate` with YAML manifests, budget controls.
- [x] `rms_wiki_pack` MCP tool — agents trigger wiki generation from any IDE.
- [x] `RetrievalService` facade — decoupled retrieval shared by MCP tools and WikiService.
- [x] Project label provenance: `project: <key>` auto-set, preserved, conflict-detected via `serde_yaml::Mapping`.
- [x] `rms-memory projects list/locate` for registry diagnostics.
- [x] `rms-memory projects remove <key>` for safe unregister without implicit vault deletion.
- [x] Transport-neutral `ProjectService`: separate unregister and confirmed vault/index deletion, with canonical containment checks and an explicit source-code exclusion.
- [x] Companion GUI lifecycle controls: separate actions, exact-key confirmation, unsaved-settings guard, and valid-scope refresh after removal.
- [x] ChatGPT / Codex TOML installer: `inject_toml()` for `~/.codex/config.toml` `[mcp_servers]`.
- [x] Global vault fallback removed — bad rootUri → error + diagnostic log.
- [x] Rootless MCP routing: negotiated `roots/list`, explicit tool-level `project`, unbound `rms_projects`, and repository-specific injected keys.
- [x] Deterministic code hierarchy: resolved `project → folder → file → symbol` `contains` edges under `code-structure-v1`.
- [x] Plain Markdown repair: backup + stable UUID frontmatter without body loss.
- [x] Installed-binary gate: clean `build.sh`, valid ad-hoc signature, and successful `rms_write` from `cwd=/` into `rms-threads-assistant`.
- [x] `ignore::WalkBuilder` for wiki file walking (nested `.gitignore`).
- [x] `pack_id` with Git revision for reproducible builds.
- [x] Security: wiki path containment, symlink validation, secrets exclusion.
- [x] Final lifecycle verification: core tests, strict core Clippy, companion GUI build gates.
- [x] Wiki path policy + write isolation (`path_policy`, wiki-safe DocumentService, Doctor Wiki isolation).
- [x] Path-scoped code watcher reindex (`try_index_code_paths`) and `upsert_derived_graph_patch`.
- [x] Vault-aware `link:` containment; `rms_wiki_pack` manifest jail.

## v1.1 — Workspace Split & Ecosystem (Next)

**Goal:** Transition the already shared core/GUI architecture from a monolithic crate into a modular ecosystem of crates.

- `rms-memory-core` — Core abstractions, API schemas, and types.
- `rms-memory-vault` — File-system interactions and Vault management.
- `rms-memory-index` — The RAG engine (LanceDB vector + Tantivy FTS).
- `rms-memory-mcp` — Model Context Protocol server implementation.
- `rms-memory-cli` — The end-user CLI application.

This enables downstream consumers to use just `rms-memory-index` without the MCP layer.

The companion GUI already consumes the core library through human-oriented Tauri commands for graph, configuration, jobs, and project lifecycle operations. MCP remains the IDE/agent protocol; v1.1 focuses on extracting stable crate boundaries rather than inventing a second application core.

**Not a v1.0.x release blocker.** Path-scoped code watch, graph layout worker (GUI), and GUI entitlement guards are already landed in the 1.0.6 / GUI 1.0.0 line.

## Multilanguage Code Memory — 1.0.5 extension (complete)

- [x] Language-neutral registry/dispatcher with project-scoped `code_languages`, preserving Rust item IDs and extractor identity.
- [x] Go adapter and real-project dogfood on the user's heavy Go/Next projects.
- [x] TypeScript/TSX, JavaScript/JSX, Python, C, C++, Java, Ruby, and Swift adapters with parser regression coverage and conservative syntax-level graph hints.
- [x] Vue 3 SFC shell parsing with inline nested JS/TS parsing and original-source mapping.
- [x] Mixed-language resource gate and four-IDE idle verification; packaging remains covered by the existing release matrix.

The detailed production contract and delivery slices are maintained privately in RMS Memory.

## v2.0 — Multi-Vault & Advanced Context (Future)

- Multi-vault routing: one MCP server serving multiple workspaces simultaneously.
- Agentic memory graphs: autonomous summarization, knowledge consolidation, stale context pruning.
- Remote backends: cloud-hosted Vector DBs (managed LanceDB, Qdrant) beyond local `.lancedb`.
