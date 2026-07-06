# RMS Memory MCP

**RMS Memory MCP** is a localized, production-grade Model Context Protocol (MCP) server designed to solve AI-agent context fragmentation. 

If you are developing a single project but switching between different agents (Cursor, Zed, Claude Code, OpenCode), they frequently lose context of architectural decisions, system requirements, and user preferences. RMS Memory bridges this gap by maintaining an isolated, centralized markdown Vault perfectly configured for LLM consumption, effectively serving as an external memory bank for all your IDEs.

## Key Features

- **Global Centralized Vaults:** Codebases remain clean. Project context is automatically routed to an external, user-defined Vault directory without polluting repositories with `.mcp` files.
- **LanceDB Hybrid Retrieval:** Powered by an embedded LanceDB engine enabling zero-fail retrieval through parallel Vector Search + Tantivy Full-Text Search.
- **Multilingual Semantic Parsing:** The `fastembed-rs` pipeline uses `multilingual-e5-small` to understand both Russian and English context perfectly.
- **AST Markdown Chunker:** Context is king. `pulldown-cmark` is used to split documents along their logical Abstract Syntax Tree bounds, keeping code blocks and lists tightly bound to their parent Headings.
- **Dynamic Auto-Installer:** `rms-memory install` scans your entire system to natively inject MCP configurations directly into your preferred IDEs without manual JSON hacking.
- **Rules-as-Code Patching (Opt-In):** Safely injects agent context prompts (`.cursorrules`, `.zed/assistant.md`, etc.) using a non-destructive AST block-patching algorithm. Auto-injection defaults to `false` to protect pristine environments. Force generation of all rule templates with `rms-memory init --full`.
- **Dry-Run & Auditing:** Verify exactly what the installer or rules injector will do before modifying configuration files by passing `--dry-run`. All modified files generate `.bak` backups before write.
- **Six-Point Resiliency:** Features an automated Garbage Collector (`gc`), Background Incremental Sync via File Watcher, AI Write-Guard snapshot backups, macOS Sandbox Bypassing (for embedded model caching), LLMs.txt export endpoints, and isolated File Telemetry Logging (`log`).

## Installation

```bash
# 1. Clone the repository
git clone https://github.com/max-ramas/rms-memory-mcp.git
cd rms-memory-mcp

# 2. Install dependencies (Requires protoc for LanceDB)
# macOS: brew install protobuf
# Ubuntu: sudo apt-get install protobuf-compiler
# Windows: choco install protoc

# 3. Build the optimized release binary
cargo build --release

# 3. Add the binary to your global PATH
# (e.g., cp target/release/rms-memory-mcp ~/.cargo/bin/)
```

## Quick Start

The fastest way to get your IDEs connected to the memory vault is using the auto-installer.

```bash
rms-memory install
```
This interactive command will scan `~/.config/` and `~/Library/Application Support/` and seamlessly hook `rms-memory` into the configurations of **Cursor**, **Zed**, **Claude Code**, **OpenCode**, and others.

### Initialization & Configuration

First, configure your global master settings (where all your knowledge will live).
```bash
rms-memory config --vault-path ~/MyVaults/ --auto-add true
```

When you navigate to any code project and start your IDE, the server reads the `rootUri` sent during the MCP `initialize` request. Because `--auto-add true` is enabled, the server will dynamically provision a perfectly structured folder ready to accept memory:
```text
~/MyVaults/
  └── <ProjectHash>/
      ├── rules/
      ├── decisions/
      ├── architecture/
      └── artifacts/
```

### CLI Commands

- `rms-memory serve` - Initialize the JSON-RPC Stdio server (Automatically triggered by your IDE). It connects to the project sent in the `initialize` message.
- `rms-memory init` - Manually register a project into the global registry. Supports `--dry-run`. Use `--full` to forcefully create all supported IDE rule templates (`.cursorrules`, `GEMINI.md`, etc.) and append them to `.gitignore`.
- `rms-memory import` - Scan the project for existing documentation (`README.md`, `docs/`, `ADR/`, etc.) and import them into the Vault interactively (Import, Import & Organize, Link Only).
- `rms-memory install` - Hook the server into supported IDEs interactively (Supports `--dry-run`).
- `rms-memory config` - Set global settings (`--vault-path`, `--auto-add`, `--inject-rules`, `--auto-import`).
- `rms-memory reindex` - Force a monolithic re-indexing of the current project vault.
- `rms-memory sync` - Perform an incremental LanceDB "Delete-then-Insert" sync (runs automatically in background during `serve`).
- `rms-memory gc` - Prune orphaned LanceDB indices that belong to deleted vaults.
- `rms-memory log` - Tail the isolated telemetry logs (`tail -f ~/.rms-memory/rms.log`).
- `rms-memory export-llms` - Compile the current Vault down to a single `llms.txt` payload.

## MCP Tools Exposed

This server provides highly descriptive MCP tool schemas specifically engineered so agents like Claude, Gemini, and Cursor understand *exactly* when to use them.

1. `rms_search`
   - **Purpose:** Search the local RMS Memory vector database for project documentation, architectural decisions, and context rules using semantic similarity. Agents are instructed to use this tool *first* to understand the repository's background before making changes.
   - **Input:** `{ "query": "string", "limit": 10, "include_content": true }`
2. `rms_read`
   - **Purpose:** Read the full contents of a markdown document from the RMS Memory vault. Used to retrieve the full context of a document found via `rms_search`.
   - **Input:** `{ "path": "string" }`
3. `rms_write`
   - **Purpose:** Save new architectural decisions, constraints, development rules, or project context. Agents are prompted to use this tool *proactively* at the end of a task if they learned a new user preference or solved a tricky bug.
   - **Input:** `{ "path": "string", "content": "string", "mode": "replace|append|create" }`

## License
MIT License
