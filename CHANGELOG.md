# Changelog

All notable changes to this project will be documented in this file.

## [1.0.2] - 2026-07-08

### Security
- **Path Traversal Prevention:** `rms_read` and `rms_write` now reject paths containing `..` components, preventing escape from the vault directory.
- **LanceDB Filter Injection:** All filter strings in `store.rs` (`delete_document`, `read_document`) now escape single quotes via `escape_filter()`, preventing potential data corruption.

### Fixed
- **OpenCode MCP Payload Format:** Replaced hardcoded `if ide.name == "OpenCode"` branching with a `PayloadBuilder` dependency-injection architecture. The `opencode_payload` function now produces OpenCode's native `McpLocalConfig` schema (`{"type": "local", "command": [array], "enabled": true}`), which was previously missing the `type` field and using a string `command` instead of an array.
- **Zed JSONC Silent Skip:** The installer now falls back to `strip_json_comments()` when `serde_json::from_str` fails on config files containing `//` comments (e.g. Zed's `settings.json`). Previously, Zed configs were silently skipped. On parse failure, `tracing::warn!` now logs a clear diagnostic message.
- **Missing `enabled: true` for Standard Payload:** The `standard_payload` function now injects `"enabled": true` for all non-OpenCode IDEs. This is required by Zed's `context_servers` schema and is a harmless no-op for Claude, Cursor, and VSCode.
- **Hybrid Search Activated:** The LanceDB `search()` method now uses combined vector search + Tantivy full-text search (`FullTextSearchQuery`). Previously, only vector search was performed despite the FTS index being created. Falls back to vector-only gracefully if FTS index is unavailable.
- **Write `create` Mode:** Now correctly rejects overwriting existing files. Previously, `create` fell through to the catch-all `_` branch which behaved as a full overwrite.
- **`links_resolved` No Longer Stub:** Both `sync_vault` and `index_vault_full` now store actual normalized link paths in `links_resolved` instead of the placeholder `"[]"`.
- **Safe Unsafe:** Added `// SAFETY:` documentation comment for the `TMPDIR` environment variable override in `Indexer::new()`.

### Added
- **`rms-memory doctor` (Full Diagnostics):** Implements 5-point vault health check: directory structure, missing document IDs, broken cross-document links, LanceDB store accessibility, and registry coherence.
- **`rms-memory uninstall`:** New command to remove `rms-memory` entries from all discovered IDE configuration files. Uses `patcher::remove_key()` for safe JSONC-aware key removal with automatic `.bak` backups.
- **Graceful Shutdown:** Added `SIGINT`/`Ctrl+C` handler in `main.rs` via `tokio::signal::ctrl_c()`, ensuring clean log flush on exit instead of immediate process termination.
- **Zombie Process Prevention:** MCP server now signals background file-watcher tasks to stop when stdin closes (EOF on disconnect). `std::process::exit(0)` in `main.rs` guarantees the process terminates even if tokio runtime has lingering tasks.
- **llms.txt Export Compliance:** `export-llms` now generates a proper `llms.txt` spec format with clickable links, frontmatter-derived titles, and content summaries, in addition to the full vault contents section.

### Changed
- **Installer DI Architecture:** Introduced `PayloadBuilder` type alias (`fn(exe: &str) -> serde_json::Value`) and attached it to `IdeConfig` as a `build_payload` field. Adding a new IDE format no longer requires modifying `run_installer()` — just one line in `get_ide_registry()`.
- **ExportLlms Optimization:** Fixed double `find_markdown_files()` call — now computed once.

## [1.0.1] - 2026-07-07

### Added
- **crates.io Release Prep:** Configured `Cargo.toml` with `readme`, `documentation`, `homepage`, `keywords`, and `categories` metadata for official publication.
- **Library Target (`lib.rs`):** Exposed public API (`store`, `indexer`, `tools`, `workspace`, `document`) as a library crate, separating binary and library logic to support ecosystem consumption (v1.1+ Roadmap).
- **ROADMAP.md:** Added strategic roadmap tracking the transition to a Cargo Workspace ecosystem and multi-vault architecture.

### Fixed
- **CI/CD Build Arguments:** Removed outdated `--target` arguments from the GitHub Actions `release.yml` pipeline that were causing `cargo generate-rpm` to fail during cross-platform deb/rpm packaging.

## [1.0.0] - 2026-07-07 

### Fixed
- **MCP Initialization Bug:** Changed MCP workspace provisioning to fallback to the process's current working directory (`cwd`) when the IDE (like Zed) fails to pass a valid `rootUri` in the JSON-RPC `initialize` handshake. Furthermore, strict validation now rejects root paths (`/`) to entirely prevent the generation of orphaned `UnknownProject` vaults.
- **Config Directory Isolation:** Removed the `directories::ProjectDirs` abstraction which defaulted to messy system paths (e.g., `~/Library/Application Support/`). All configs, databases, and logs are now strictly bound to a cross-platform `~/.rms-memory/` directory to ensure clean disk usage.
- **Import Routing Categorization:** Fixed an issue where unmatched markdown files were erroneously dumped into `guides/`. They now default to `docs/`. Added strict mapping for `task.md`, `walkthrough.md`, `changelog`, `history` and `implementation_plan` to route directly to `artifacts/`.
- **Infinite Sync Loop Fix (CPU Hog):** Resolved a critical issue where the `notify` file watcher would recursively trigger itself indefinitely by ignoring changes to `.lancedb`, `store.json`, and `.log` files, ensuring 0% CPU consumption during idle times.
- **macOS Sandboxing Crashes:** `rms-memory install` now automatically copies a safe binary, builds entitlements (`disable-library-validation`), and runs `codesign` atomically to bypass macOS terminating the running process with `SIGKILL`.
- **Agent Rule Templates:** Updated all MCP injected templates (`general_mcp_guide.md`, `cursor_rules.md`, `claude_code_rules.md`, `zed_assistant_rules.md`) to explicitly use IDE-prefixed tools (`rms-memory_rms_search`) and document the `artifacts/`, `docs/`, and `api/` directories.
- **Repository Pollution:** Removed the generation of `.bak` files during agent rule injection (`rules_injector.rs`) to prevent flooding user workspaces with backup files.
- **Batched Vector Indexing (OOM Fix):** Replaced monolithic embedding calls with a batched chunking architecture (`batch_size = 32`) in `sync_vault` and `index_vault_full`. This entirely resolves extreme CPU/RAM spikes and process deadlocks (OOM) when indexing exceptionally large files, allowing the `fastembed` ONNX Runtime to efficiently ingest 100% of the file content without truncation.

### Changed
- **Default Vault Structure:** Vault initialization (`cli.rs`, `workspace.rs`) now explicitly creates `docs/` and `api/` directories alongside `rules/`, `decisions/`, `architecture/`, and `artifacts/`.

### Added
- **Monolith Refactoring & Dependency Injection:** Completely dismantled `cli.rs` and `mcp_server.rs` monoliths into modular components (`src/commands/` and `src/tools/`). Introduced `AppContext` for dependency injection of databases and models across the system.
- **MCP Stdio Server:** Full implementation of JSON-RPC protocol over standard I/O for `rms-memory_rms_read`, `rms-memory_rms_write`, and `rms-memory_rms_search` tooling.
- **Global Vault Registry:** Added `registry.toml` routing logic allowing the server to automatically detect the current code directory and isolate contextual documentation into a unified, secure system-level vault (`~/.rms-memory/vaults/ProjectName`).
- **Hybrid LanceDB Retrieval:** Deployed local embedded `LanceDB` (v0.31.0) configured with multi-threaded Vector Search and Tantivy FTS indices to guarantee zero-fail context hits.
- **Multilingual-E5-Small FastEmbed Pipeline:** Native ONNX AST embedding for dual-language (Russian & English) documentation support directly integrated into the ingest flow.
- **Semantic AST Markdown Chunking:** Precision boundary preservation (1500 chars limit) via `pulldown-cmark`. It attaches hierarchical headings to paragraphs and enforces smart sliding-window truncation for large code blocks.
- **Dynamic IDE Auto-Installer (`rms-memory install`):** Interactive recursive scanner spanning `~/.config/` and `~/Library/Application Support/` to auto-inject the `mcpServers` JSON object directly into Cursor, Zed, OpenCode, VS Code, and Claude Code configurations.
- **Rules-as-Code IDE Patching:** Non-destructive AST block-patching (`<!-- RMS-MEMORY-START -->`) to automatically inject contextual guide instructions into `.cursorrules`, `.claude/CLAUDE.md`, `.zed/assistant.md`, and `RMS_MEMORY_GUIDE.md`.
- **Dry-Run & System Auditing (`--dry-run`):** Execution previews for `install` and `init` commands to visualize JSON modifications and Markdown AST patches without corrupting host files. Generates automated `.bak` backups before any writes.
- **Background Incremental Sync (`rms-memory sync`):** Zero-latency startup sync task performing `Delete-then-Insert` vector replacement based on `mtime` bound metadata.
- **Garbage Collection (`rms-memory gc`):** Prunes orphaned LanceDB data caches matching deleted Vault boundaries.
- **LLMs.txt Export Hook:** Enables one-shot compilation of a project Vault into standard flat structures.
- **Linked Documents Architecture:** Introduced native support for "Link Files"—standard Markdown Vault entries containing a `link: <source_path>` frontmatter property. The LanceDB indexer, `read` and `write` endpoints seamlessly trace and operate on the original repository file while enforcing strict path constraints within the Vault directory structure.
- **Interactive Documentation Importer (`rms-memory import`):** Added a powerful CLI importer to scan project repositories for existing legacy documentation (`README.md`, `CLAUDE.md`, `.cursorrules`, `docs/`, `ADR/`). Features 5 user-selectable strategies including `Link Only` (creates zero-copy Link Files) and `Import & Organize` (deterministically categorizes files into the Vault).
- **Auto-Import Integration:** Added `--auto-import` flag to `rms-memory config` (`auto_import_strategy` in `registry.toml`). When `auto_add_projects` triggers in the background, the server can now silently resolve legacy documentation into the Vault using the pre-configured strategy without blocking the MCP connection.
- **Write-Guard Snapshotting:** Captures local `fs::copy` `.bak` state preservation automatically before an AI is permitted to execute JSON-RPC `.md` replace/append mutations.
- **Dedicated Telemetry Logging:** Integrated `tracing` streams routed directly to `~/.rms-memory/rms.log` shielding MCP stdio channels from standard output noise.

### Fixed
- **Safe Auto-Inject Default:** `auto-inject` configuration now defaults to `false`, preventing implicit modification of `.cursorrules`, `CLAUDE.md`, etc., upon first repository discovery without explicit user consent.
- **Write-Guard Rolling Backups:** Added `max_backups` parameter to prevent disk pollution. Ensures backup files are gracefully rotated and limited (default 5).
- **Embedding Dimension Safety:** Hardened `Store::init` to properly validate existing table schema dimension arrays against the current embedding model dimension. Returns a loud `INDEX_REBUILD_REQUIRED` error instead of a runtime vector search panic.
- **Dependency Inversion (DIP) & Testing:** Extracted `VectorStore` and `Embedder` traits from `McpServer`, establishing a robust Mock architecture for the JSON-RPC layer and cleanly eliminating LanceDB `RecordBatch` leaky abstractions.
- **Bug Fix**: Fixed server crash/early exit during `initialize` in strict sandboxes by routing `hf-hub` atomic download temp files to the `.rms-memory` cache directory instead of the system temp directory, preventing `Read-only file system` (os error 30) panics.
- **Bug Fix**: Fixed JSON-RPC stream corruption where background auto-initialization was writing `[INFO]` logs to `stdout` instead of using `tracing` or `stderr`, causing the MCP client to instantly disconnect the transport.
