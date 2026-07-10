# RMS Memory MCP Server — Walkthrough

RMS Memory is a specialized Model Context Protocol (MCP) server that acts as a localized persistent memory for LLM agents. By isolating project knowledge into centralized standard Markdown Vaults, it solves context fragmentation across multiple IDEs (Cursor, Zed, VS Code, Claude Code).

## Core Architecture Highlights

### 1. Unified Configuration & Knowledge Isolation
- **Global Registry:** No more polluting code repositories with `.mcp` or `RMS.toml` files. The routing logic uses a central `~/.rms-memory/registry.toml`.
- **Auto-Discovery & Provisioning:** The server reads the `rootUri` dynamically from the MCP `initialize` request sent by the IDE (falling back to the current working directory if missing). It then calculates a unique hash and seamlessly routes agents to an isolated external Markdown vault (`/user/defined/path/ProjectName`). If it doesn't exist, it is cleanly provisioned with structured directories (`rules/`, `decisions/`, `architecture/`, `artifacts/`, `docs/`, `api/`). This lazy initialization enables global MCP servers (like Zed's `settings.json`) to accurately target specific workspaces.

### 2. Linked Documents & Documentation Import
- **Intelligent Importer:** The server features a native `import` module (`rms-memory import`) that scans the target codebase for existing documentation (`README.md`, `CLAUDE.md`, `.cursorrules`, `docs/`, `ADR/`).
- **Interactive & Auto Integration:** Users can interactively choose how to handle existing knowledge during `rms-memory init` or let the server auto-import during `auto_add` based on the `--auto-import` config strategy.
- **Linked Documents (No Duplication):** The recommended `Link Only` and `Import & Organize` flows utilize a unique Linked Document architecture. Instead of duplicating project files into the Vault, the system creates a lightweight "Link File" (a markdown file containing standard Frontmatter with a `link: <relative/path/to/source>` property).
- **Guaranteed Consistency:** 
  - **Reads:** Intercepted by the server to return the live source file content.
  - **Writes:** Intercepted and rerouted back to the source file, guaranteeing the Vault link file metadata is never overwritten by an autonomous agent.
  - **Indexing:** The LanceDB chunker traces the link, indexing the source file content but retaining the Vault's directory structure for vector metadata (`architecture/auth.md`).

### 3. Hybrid Search Engine (LanceDB)
- **Local Embedded DB:** Uses the blazingly fast embedded LanceDB (v0.31.0) stored locally at `~/.rms-memory/dbs/`.
- **Hybrid Retrieval:** Fully implements combined Vector Search + Tantivy Full-Text Search (FTS). It avoids keyword matching failures by falling back to precise vector similarities.
- **Multilingual Semantic Parsing:** Driven by `fastembed-rs` utilizing the `multilingual-e5-small` model (384 dimensions) natively handling both English and Russian code documentation contexts.

### 3. Advanced Context Chunking
- **AST Markdown Chunker:** Raw token truncation destroys structured knowledge. This server uses `pulldown-cmark` to parse the Markdown Abstract Syntax Tree (AST) directly.
- **Heading-Preservation:** Code blocks, paragraphs, and list elements are recursively accumulated under their direct parent `Heading` to generate perfectly contextualized vector chunks.
- **Sliding-Window Fallback:** Enforces a strict 1500-character boundary to protect context windows. Monolithic code blocks are split sequentially with an overlapping ~200-character window.
- **Batched Semantic Indexing:** To prevent Out-Of-Memory (OOM) crashes and CPU starvation on large files, the indexer pipelines all text chunks into the embedding model in strictly controlled batches (`batch_size = 32`). This maintains a flat memory footprint and guarantees stable parallel throughput.

### 4. Dynamic MCP Auto-Installer (`rms-memory install`)
- Eradicates manual configuration. Run `rms-memory install` and a strict bounding crawler scans `~/.config/` and `~/Library/Application Support/` across your OS.
- **Cross-Format Resilience:** The patcher handles both standard JSON (Claude, Cursor, VSCode) and **JSONC** (Zed — supports `//` comments). The `inject_jsonc` engine strips comments character-by-character before parsing, then applies regex-based in-place injection to preserve the original file's formatting and comments.
- **Dependency Injection (`PayloadBuilder`):** Each IDE entry carries its own `build_payload` function via the `PayloadBuilder` type alias. This eliminates inline `if/else` branching in the installer core — adding a new IDE format is a one-line change in `registry.rs`.
- **OpenCode Native Schema:** OpenCode receives `{"type": "local", "command": ["/path/rms-memory", "serve"], "enabled": true}` — matching its `McpLocalConfig` JSON Schema exactly. All other IDEs get the standard `{"command": "/path", "args": ["serve"], "enabled": true}` format.
- **Failure Logging:** When a config file fails to parse even after JSONC stripping, the installer logs a `tracing::warn!` diagnostic instead of silently skipping, making misconfigured IDE configs debuggable.

### 5. Rules-as-Code Agent Patching
- **Cross-IDE Context:** Automatically drops IDE-specific guide files upon repository discovery.
  - `.cursorrules` (Cursor)
  - `.claude/CLAUDE.md` (Claude Code)
  - `.zed/assistant.md` (Zed)
  - `RMS_MEMORY_GUIDE.md` (Fallback)
- **Non-Destructive AST Patching:** Embedded a safe block-patching algorithm utilizing `<!-- RMS-MEMORY-START -->` and `<!-- RMS-MEMORY-END -->`. This guarantees the server seamlessly injects and updates its usage instructions without corrupting any existing developer constraints. It performs safe in-place updates during injection, completely avoiding the generation of noisy `.bak` files in user workspaces.
- **Force Generation (`--full`):** By default, the injector only patches rule files that *already exist* to prevent workspace pollution. Running `rms-memory init --full` will force the creation of all supported IDE templates (Cursor, Windsurf, Zed, Gemini, Claude, etc.) and automatically append them to the project's `.gitignore`.
- **Opt-In Control (`inject_rules`):** Integrated `--inject-rules <true|false>` into the `rms-memory config` CLI command. Auto-injection now strictly defaults to `false`. Developers must explicitly opt-in globally or per-project to protect pristine IDE configs from silent modification.
- **Dry-Run & Auditing:** Added full `--dry-run` telemetry across all injection and installation flows (`rms-memory init --dry-run`, `rms-memory install --dry-run`). Emits an exact preview of the targeted configuration files and visualizes the generated AST patch payload (`[NEW BLOCK]` vs `[Replace existing block]`) without writing to disk.

### 6. LLM-Optimized MCP Tool Schemas
- **Context-Aware Tool Descriptions:** A common failure mode for MCP servers is providing vague tool schemas (e.g., "Search the database"). RMS Memory embeds highly descriptive, action-oriented prompts directly into the JSON-RPC `tools/list` response.
- **Proactive AI Behavior:** The tool descriptions explicitly command the LLM when to act. For example, `rms_search` instructs the agent to "Use this tool FIRST to understand the repository's background", and `rms_write` commands the agent to "Use this tool PROACTIVELY at the end of a task if you learned a new user preference". This guarantees Cursor and Claude will leverage the memory vault autonomously without user prompting.

### 7. Production-Grade System Resiliency
To transition from a "toy server" to an instrumental platform, 10 resilience protocols are enforced:
1. **Path Traversal Protection:** All MCP tool handlers (`rms_read`, `rms_write`) reject paths containing `..` and enforce vault containment.
2. **Filter Injection Prevention:** LanceDB query filters escape single quotes in document IDs and paths, preventing malformed filter strings from corrupting the data layer.
3. **Zombie Process Prevention:** When the IDE closes stdin (EOF on disconnect), the `run()` loop signals the background file-watcher task to stop via a `tokio::sync::watch` channel. The watcher breaks its `loop` and the task terminates. `std::process::exit(0)` in `main()` guarantees the process exits even if tokio runtime has lingering tasks.
4. **macOS Sandbox Bypassing:** Claude Desktop and other IDEs operate in strict macOS Read-Only sandboxes. The server detects sandbox constraints and dynamically intercepts `fastembed` model downloads, rerouting `TMPDIR` and caching layers exclusively to the user's guaranteed-writable `~/.rms-memory/cache/` directory. The `unsafe` block is documented with a full `// SAFETY:` comment explaining bounded scope and restoration.
5. **Garbage Collection (`rms-memory gc`):** Detects and purges orphaned LanceDB vector stores belonging to deprecated project vaults.
6. **Incremental Sync (`rms-memory sync`):** Background `tokio` indexing on MCP launch. Uses a strict LanceDB `Delete-then-Insert` pipeline against file `mtime` bounds to cleanly sync vectors without RAG pollution.
6. **Real-time File Watcher:** Background `notify` service instantly detects IDE saves (`Modify` filesystem events). Triggers a trailing-edge debounced (3s) incremental `sync_vault` to guarantee persistent memory seamlessly stays aligned with local workspace changes without requiring manual explicit syncs or server restarts.
7. **Write-Guard Snapshotting:** JSON-RPC `write` events triggered by autonomous agents are intercepted. The server automatically issues an `fs::copy` artifact backup to `.bak` before permitting the agent's modification. Includes a rolling backup system (`max_backups` config, default 5) to prevent unbounded disk pollution from continuous AI revisions. The `create` mode rejects overwriting existing files, requiring explicit `replace` mode for modifications.
8. **Graceful Shutdown:** `SIGINT`/`Ctrl+C` handler ensures clean log flush on exit.
9. **LLMs.txt Export (`export-llms`):** Compiles the entire isolated Vault structure into a standardized `llms.txt` digest with clickable links, titles, and summaries — compatible with LLM ingestion tools.

### 8. System Diagnostics & Maintenance
- **Doctor (`rms-memory doctor`):** A 5-point health check system that validates:
  1. Vault directory structure (rules, decisions, architecture, artifacts, docs, api)
  2. Missing document IDs in frontmatter
  3. Broken cross-document markdown links (checks file existence)
  4. LanceDB store connectivity
  5. Registry coherence (project-to-vault path mapping)
- **Uninstall (`rms-memory uninstall`):** Removes `rms-memory` entries from all discovered IDE configuration files. Uses the same JSONC-aware patcher as the installer with automatic `.bak` backups, making uninstallation as safe and transparent as installation.
- **Hybrid Retrieval Activation:** The `VectorStore::search()` implementation now truly combines vector similarity AND Tantivy full-text search (FTS). Previously, the FTS index was built on table creation but never queried — searches were vector-only. The fix adds a two-tier approach: hybrid search with graceful fallback to vector-only if the FTS index is unavailable.

### 8. Modular Architecture & crates.io Ready
- **Library API (`lib.rs`):** Prepared for ecosystem integration by exposing core components (`store`, `indexer`, `tools`) as a public Rust library. Internal CLI logic remains safely encapsulated.
- **Dependency Injection (`AppContext`):** The system uses a centralized `AppContext` that securely holds the initialized LanceDB connection, embedding models (`fastembed`), and runtime configuration. This eliminates redundant initializations and allows dependency injection across all commands and tools.
- **CLI Commands (`src/commands/`):** The massive `cli.rs` monolith was completely dismantled into individual domain-specific modules (`install`, `init`, `sync`, `gc`, etc.), dramatically improving readability and minimizing Git merge conflicts.
- **MCP Tools (`src/tools/`):** JSON-RPC tool executions are now routed to specialized handler files (`search.rs`, `read.rs`, `write.rs`) under the `src/tools/` module, ensuring clear separation of concerns between standard CLI interactions and autonomous Agent requests.

### 9. CI/CD and Cross-Platform Distribution
- **Strict User-Scoped Isolation:** The core configuration logic was overhauled to enforce a rigid `~/.rms-memory/` standard across all platforms. By utilizing the `directories` crate exclusively to locate the user's home folder, the program correctly targets `C:\Users\username\.rms-memory\` on Windows, `/Users/username/.rms-memory/` on macOS, and `/home/username/.rms-memory/` on Linux without polluting generic OS domains like `AppData/Roaming` or `Library/Application Support/`.
- **GitHub Actions Matrix:** Engineered a seamless `release.yml` pipeline that triggers on repository version tags.
  - Compiles optimized native binaries for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`.
  - Packages and uploads them instantly as GitHub Release assets.
- **Single-Line Installers:** `install.sh` (cURL/Bash) and `install.ps1` (PowerShell) automatically detect target system architecture, fetch the optimal GitHub release binary, map it to the exact correct path (`~/.cargo/bin` or user-defined), and add it to standard OS `PATH` vars.
