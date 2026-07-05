# Changelog

All notable changes to this project will be documented in this file.

## [1.0.0] - 2026-07-05

### Fixed
- **MCP Initialization Bug:** Changed MCP workspace provisioning to fallback to the process's current working directory (`cwd`) when the IDE (like Zed) fails to pass a valid `rootUri` in the JSON-RPC `initialize` handshake. Furthermore, strict validation now rejects root paths (`/`) to entirely prevent the generation of orphaned `UnknownProject` vaults.
- **Config Directory Isolation:** Removed the `directories::ProjectDirs` abstraction which defaulted to messy system paths (e.g., `~/Library/Application Support/`). All configs, databases, and logs are now strictly bound to a cross-platform `~/.rms-memory/` directory to ensure clean disk usage.

### Added
- **MCP Stdio Server:** Full implementation of JSON-RPC protocol over standard I/O for `read`, `write`, and `search_memory` tooling.
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
