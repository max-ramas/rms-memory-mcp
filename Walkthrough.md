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

### 4. Dynamic MCP Auto-Installer (`rms-memory install`)
- Eradicates manual configuration. Run `rms-memory install` and a strict bounding crawler scans `~/.config/` and `~/Library/Application Support/` across your OS.
- Targets native IDE configurations like `mcp.json` or `settings.json` across ecosystems (Cursor, Zed, Antigravity, OpenCode, Claude Code) and injects the absolute JSON-RPC binary path using non-destructive `serde_json` merging.

### 5. Rules-as-Code Agent Patching
- **Cross-IDE Context:** Automatically drops IDE-specific guide files upon repository discovery.
  - `.cursorrules` (Cursor)
  - `.claude/CLAUDE.md` (Claude Code)
  - `.zed/assistant.md` (Zed)
  - `RMS_MEMORY_GUIDE.md` (Fallback)
- **Non-Destructive AST Patching:** Embedded a safe block-patching algorithm utilizing `<!-- RMS-MEMORY-START -->` and `<!-- RMS-MEMORY-END -->`. This guarantees the server seamlessly injects and updates its usage instructions without corrupting any existing developer constraints. It performs safe in-place updates during injection, completely avoiding the generation of noisy `.bak` files in user workspaces.
- **Opt-In Control (`inject_rules`):** Integrated `--inject-rules <true|false>` into the `rms-memory config` CLI command. Auto-injection now strictly defaults to `false`. Developers must explicitly opt-in globally or per-project to protect pristine IDE configs from silent modification.
- **Dry-Run & Auditing:** Added full `--dry-run` telemetry across all injection and installation flows (`rms-memory init --dry-run`, `rms-memory install --dry-run`). Emits an exact preview of the targeted configuration files and visualizes the generated AST patch payload (`[NEW BLOCK]` vs `[Replace existing block]`) without writing to disk.

### 6. Production-Grade System Resiliency
To transition from a "toy server" to an instrumental platform, 5 resilience protocols are enforced:
1. **Garbage Collection (`rms-memory gc`):** Detects and purges orphaned LanceDB vector stores belonging to deprecated project vaults.
2. **Incremental Sync (`rms-memory sync`):** Background `tokio` indexing on MCP launch. Uses a strict LanceDB `Delete-then-Insert` pipeline against file `mtime` bounds to cleanly sync vectors without RAG pollution.
3. **Write-Guard Snapshotting:** JSON-RPC `write` events triggered by autonomous agents are intercepted. The server automatically issues an `fs::copy` artifact backup to `.bak` before permitting the agent's modification. Includes a rolling backup system (`max_backups` config, default 5) to prevent unbounded disk pollution from continuous AI revisions.
4. **LLMs.txt Export (`export-llms`):** Compiles the entire isolated Vault structure into a standardized `llms.txt` digest for decoupled LLM ingestion or raw curl queries.
5. **Dedicated Telemetry Logging:** MCP stdio pipelines are preserved strictly for JSON-RPC. All diagnostics, sync logs, and internal errors are securely routed to `~/.rms-memory/rms.log` using standard `tracing-appender` streams. Tail it using `rms-memory log`.

### 7. CI/CD and Cross-Platform Distribution
- **Strict User-Scoped Isolation:** The core configuration logic was overhauled to enforce a rigid `~/.rms-memory/` standard across all platforms. By utilizing the `directories` crate exclusively to locate the user's home folder, the program correctly targets `C:\Users\username\.rms-memory\` on Windows, `/Users/username/.rms-memory/` on macOS, and `/home/username/.rms-memory/` on Linux without polluting generic OS domains like `AppData/Roaming` or `Library/Application Support/`.
- **GitHub Actions Matrix:** Engineered a seamless `release.yml` pipeline that triggers on repository version tags.
  - Compiles optimized native binaries for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`.
  - Packages and uploads them instantly as GitHub Release assets.
- **Single-Line Installers:** `install.sh` (cURL/Bash) and `install.ps1` (PowerShell) automatically detect target system architecture, fetch the optimal GitHub release binary, map it to the exact correct path (`~/.cargo/bin` or user-defined), and add it to standard OS `PATH` vars.
