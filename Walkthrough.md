# RMS Memory MCP Server — Walkthrough

Updated: 2026-07-23 · MCP `1.0.6` / GUI `1.0.0`

## Generated Wiki isolation

`src/path_policy.rs` is the single authority for deciding whether a path belongs to the generated Vault Wiki namespace. The policy is relative to the canonical Vault root, case-insensitive for the first `wiki` component, rejects path escape, and therefore covers `.generation`, archives and future Wiki subdirectories without duplicating checks across subsystems. The companion GUI reuses the same library policy for Wiki path checks so AI apply/generation cannot invent a second exclusion rule.

Write isolation matches index isolation for MCP tools: `rms_write` rejects `wiki/**` and non-`.md` paths. Canonical `DocumentService` list/read/write APIs also exclude or reject wiki; Wiki page mutations go through wiki-safe methods that preserve managed regions without injecting memory audit frontmatter. Linked-document `link:` resolution always re-checks that the canonical target stays inside the vault.

Wiki Markdown remains on disk and participates in the user's normal Git/GitHub workflow. It is excluded from Vault discovery, vector/FTS retrieval, Markdown and code watchers, the code corpus, the durable hybrid graph and Wiki context-pack inputs. Incremental/full sync and code reindex remove pre-policy derived data by normalized path, including incident graph edges and overrides, while preserving the source files. Doctor exposes the condition as a separate check so migration is observable instead of silently claiming a clean store. LLM clients, API keys and organizer/Wiki prompts remain exclusively in the GUI — this MCP binary stays AI-free.

RMS Memory is a specialized Model Context Protocol (MCP) server that acts as localized persistent memory for LLM agents. It keeps human-authored knowledge in centralized Markdown Vaults and can optionally maintain a separate derived semantic index for Rust code, solving context fragmentation across multiple IDEs (Cursor, Zed, VS Code, Claude Code, Codex).

## Core Architecture Highlights

### 1. Unified Configuration & Knowledge Isolation
- **Global Registry:** No more polluting code repositories with `.mcp` or `RMS.toml` files. The routing logic uses a central `~/.rms-memory/registry.toml`.
- **Explicit Provisioning, Read-Only Discovery:** `rms-memory init` registers and provisions a project once. Routine CLI/MCP resolution only reads the registry; it never auto-creates a project from an ambiguous path and never treats `/` or a broad home directory as a safe fallback.
- **Deterministic MCP Routing:** Connections resolve explicit `--scope` or legacy `rootUri`, then negotiated MCP `roots/list`, then an explicit short `project` argument. Process CWD is used only for clients that do not advertise Roots and only when it is not `/`. A connection binds to one project and refuses silent switching.
- **Rootless Client Support:** `rms_projects` lists keys before workspace binding. Injected agent rules carry the repository's concrete key, allowing Antigravity and similar globally launched clients to call tools with `project: "<key>"` without exposing full paths.

### 2. Linked Documents & Documentation Import
- **Intelligent Importer:** The server features a native `import` module (`rms-memory import`) that scans the target codebase for existing documentation (`README.md`, `CLAUDE.md`, `.cursorrules`, `docs/`, `ADR/`).
- **Explicit Integration:** During `rms-memory init`, users can interactively choose how to handle existing knowledge. Routine workspace discovery does not import documents or mutate a repository.
- **Linked Documents (No Duplication):** The recommended `Link Only` and `Import & Organize` flows utilize a unique Linked Document architecture. Instead of duplicating project files into the Vault, the system creates a lightweight "Link File" (a markdown file containing standard Frontmatter with a `link: <relative/path/to/source>` property).
- **Guaranteed Consistency:** 
  - **Reads:** Intercepted by the server to return the live source file content.
  - **Writes:** Intercepted and rerouted back to the source file, guaranteeing the Vault link file metadata is never overwritten by an autonomous agent.
  - **Indexing:** The LanceDB chunker traces the link, indexing the source file content but retaining the Vault's directory structure for vector metadata (`architecture/auth.md`).

### 3. Hybrid Search Engine (LanceDB)
- **Local Embedded DB:** Uses the blazingly fast embedded LanceDB (v0.31.0) stored locally at `~/.rms-memory/dbs/`.
- **Hybrid Retrieval:** Fully implements combined Vector Search + Tantivy Full-Text Search (FTS). It avoids keyword matching failures by falling back to precise vector similarities.
- **Multilingual Semantic Parsing:** Driven by `fastembed-rs` utilizing the `multilingual-e5-small` model (384 dimensions) natively handling both English and Russian code documentation contexts.

### 4. Advanced Context Chunking
- **AST Markdown Chunker:** Raw token truncation destroys structured knowledge. This server uses `pulldown-cmark` to parse the Markdown Abstract Syntax Tree (AST) directly.
- **Heading-Preservation:** Code blocks, paragraphs, and list elements are recursively accumulated under their direct parent `Heading` to generate perfectly contextualized vector chunks.
- **Sliding-Window Fallback:** Enforces a strict 1500-character boundary to protect context windows. Monolithic code blocks are split sequentially with an overlapping ~200-character window.
- **Batched Semantic Indexing:** To prevent Out-Of-Memory (OOM) crashes and CPU starvation on large files, the indexer pipelines all text chunks into strictly controlled batches of eight. This maintains a flat memory footprint and avoids starving concurrent IDE processes.

### 5. Semantic Code Memory & Relationship Graph (v1.0.5)
- **Separate corpora:** Markdown and derived source live in independent LanceDB tables. `reindex --vault` (the default), `reindex --code`, and `reindex --all` make the refresh target explicit; code indexing never modifies repository sources.
- **Multilanguage semantic chunks:** Tree-sitter adapters cover Rust, Go, JS/JSX, TS/TSX, Python, C/C++, Java, Ruby, Swift, and inline Vue scripts. Large items repeat their preamble and declaration signature in each segment, so a retrieved method remains interpretable outside its original file.
- **Incremental vectors:** Stable segment IDs and content hashes allow reindexing to reuse embeddings for unchanged code and delete only segments no longer emitted.
- **Federated retrieval:** `rms_search` accepts `corpus: vault|code|all`; `rms_code_search` is the code-only shortcut. `all` independently ranks both corpora, then merges with Reciprocal Rank Fusion (RRF), never raw cross-table distances. Code results include file, symbol, kind, line range, and segment index.
- **Graph contract:** Nodes and edges never point to retrieval chunks, whose boundaries may change. Every reindex emits a deterministic `project → folder → file → symbol` structural projection with resolved `contains` edges. Markdown links plus language-level imports/includes and lexical calls are stored as versioned derived edges; user edges and suppress/restore overrides survive reconciliation. All call edges are intentionally syntax-level hints, not a compiler-accurate call graph.
- **Shared GUI core:** A revisioned `ConfigManager` owns validated, atomic configuration updates and change subscriptions. Transport-neutral services and bounded job events are consumed by the companion GUI through human-oriented Tauri commands without repurposing MCP.
- **Safe activation:** `code_index_mode = off|manual|watch` defaults to `off`; set it with `rms-memory config --code-index-mode watch` from the registered project root. `code_languages = ["auto"]` selects all bundled adapters, or use `rms-memory config --code-languages go,typescript,vue`. Watch mode is opt-in, coalesces enabled source events for three seconds, accumulates dirty absolute paths, and calls `try_index_code_paths` so only changed files are re-embedded and graph-patched (no full generation prune). Cold start, empty/oversized dirty sets (>200), or watcher overflow fall back to a full walk. A shared completion marker keeps concurrent IDE servers from repeating a completed generation.
- **Live validation:** An unchanged reindex on this repository processed 43 Rust files, 298 items, and 438 segments with all vectors reused. A real-project stress gate completed concurrent GeoMail, License Server, RMS Monitoring, and GeoTax Site indexing; after four IDE restarts, seven MCP servers remained at 0.0% CPU with no background reindex. Path-scoped unit tests cover dirty-file hash updates and delete-without-wiping siblings. Large-fixture timing: `./scripts/bench_large_vault.sh`.

### 6. Dynamic MCP Auto-Installer (`rms-memory install`)
- Eradicates manual configuration. Run `rms-memory install` and a strict bounding crawler scans `~/.config/` and `~/Library/Application Support/` across your OS.
- **Cross-Format Resilience:** The patcher handles both standard JSON (Claude, Cursor, VSCode) and **JSONC** (Zed — supports `//` comments). The `inject_jsonc` engine strips comments character-by-character before parsing, then applies regex-based in-place injection to preserve the original file's formatting and comments.
- **Dependency Injection (`PayloadBuilder`):** Each IDE entry carries its own `build_payload` function via the `PayloadBuilder` type alias. This eliminates inline `if/else` branching in the installer core — adding a new IDE format is a one-line change in `registry.rs`.
- **OpenCode Native Schema:** OpenCode receives `{"type": "local", "command": ["/path/rms-memory", "serve"], "enabled": true}` — matching its `McpLocalConfig` JSON Schema exactly. All other IDEs get the standard `{"command": "/path", "args": ["serve"], "enabled": true}` format.
- **Failure Logging:** When a config file fails to parse even after JSONC stripping, the installer logs a `tracing::warn!` diagnostic instead of silently skipping, making misconfigured IDE configs debuggable.

### 7. Rules-as-Code Agent Patching
- **Cross-IDE Context:** Automatically drops IDE-specific guide files upon repository discovery.
  - `.cursorrules` (Cursor)
  - `.claude/CLAUDE.md` (Claude Code)
  - `.zed/assistant.md` (Zed)
  - `RMS_MEMORY_GUIDE.md` (Fallback)
- **Non-Destructive AST Patching:** Embedded a safe block-patching algorithm utilizing `<!-- RMS-MEMORY-START -->` and `<!-- RMS-MEMORY-END -->`. This guarantees the server seamlessly injects and updates its usage instructions without corrupting any existing developer constraints. It performs safe in-place updates during injection, completely avoiding the generation of noisy `.bak` files in user workspaces.
- **Force Generation (`--full`):** By default, the injector only patches rule files that *already exist* to prevent workspace pollution. Running `rms-memory init --full` will force the creation of all supported IDE templates (Cursor, Windsurf, Zed, Gemini, Claude, etc.) and automatically append them to the project's `.gitignore`.
- **Opt-In Control (`inject_rules`):** Integrated `--inject-rules <true|false>` into the `rms-memory config` CLI command. Auto-injection now strictly defaults to `false`. Developers must explicitly opt-in globally or per-project to protect pristine IDE configs from silent modification.
- **Dry-Run & Auditing:** Added full `--dry-run` telemetry across all injection and installation flows (`rms-memory init --dry-run`, `rms-memory install --dry-run`). Emits an exact preview of the targeted configuration files and visualizes the generated AST patch payload (`[NEW BLOCK]` vs `[Replace existing block]`) without writing to disk.

### 8. LLM-Optimized MCP Tool Schemas
- **Context-Aware Tool Descriptions:** A common failure mode for MCP servers is providing vague tool schemas (e.g., "Search the database"). RMS Memory embeds highly descriptive, action-oriented prompts directly into the JSON-RPC `tools/list` response.
- **Proactive AI Behavior:** The tool descriptions explicitly command the LLM when to act. For example, `rms_search` instructs the agent to "Use this tool FIRST to understand the repository's background", and `rms_write` commands the agent to "Use this tool PROACTIVELY at the end of a task if you learned a new user preference". This guarantees Cursor and Claude will leverage the memory vault autonomously without user prompting.

### 9. Production-Grade System Resiliency
To transition from a "toy server" to an instrumental platform, 10 resilience protocols are enforced:
1. **Path Traversal Protection:** All MCP tool handlers (`rms_read`, `rms_write`) reject paths containing `..` and enforce vault containment.
2. **Filter Injection Prevention:** LanceDB query filters escape single quotes in document IDs and paths, preventing malformed filter strings from corrupting the data layer.
3. **Zombie Process Prevention:** When the IDE closes stdin (EOF on disconnect), the `run()` loop signals the background file-watcher task to stop via a `tokio::sync::watch` channel. The watcher breaks its `loop` and the task terminates. `std::process::exit(0)` in `main()` guarantees the process exits even if tokio runtime has lingering tasks.
4. **macOS Sandbox Bypassing:** Claude Desktop and other IDEs operate in strict macOS Read-Only sandboxes. The server detects sandbox constraints and dynamically intercepts `fastembed` model downloads, rerouting `TMPDIR` and caching layers exclusively to the user's guaranteed-writable `~/.rms-memory/cache/` directory. The `unsafe` block is documented with a full `// SAFETY:` comment explaining bounded scope and restoration.
5. **Garbage Collection (`rms-memory gc`):** Detects and purges orphaned LanceDB vector stores belonging to deprecated project vaults.
6. **Cross-IDE Writer Coordination:** Per-project filesystem locks store owner PID metadata. Background sync is read-only; competing writers serialize, and `doctor` can diagnose stale lock owners safely.
7. **Watchers That Stay Idle:** Markdown watching is debounced. Rust watching is off by default and, when enabled, coalesces events for three seconds and suppresses duplicate completed generations across IDE processes.
8. **Write-Guard Snapshotting and Atomic Replacement:** JSON-RPC `write` events triggered by autonomous agents are intercepted. The server automatically issues an `fs::copy` backup to `.bak` before permitting the agent's modification. `create` and `replace` write/fsync a same-directory temporary file before atomically replacing the target and syncing its directory, so an interrupted write cannot expose a frontmatter-only or truncated Markdown file. The rolling backup system (`max_backups`, default 5) prevents unbounded disk pollution; `create` rejects overwriting an existing target.
9. **Graceful Shutdown:** `SIGINT`/`Ctrl+C` handler ensures clean log flush on exit.
10. **LLMs.txt Export (`export-llms`):** Compiles the entire isolated Vault structure into a standardized `llms.txt` digest with clickable links, titles, and summaries — compatible with LLM ingestion tools.

### 10. System Diagnostics & Maintenance
- **Doctor (`rms-memory doctor`):** A 5-point health check system that validates:
  1. Vault directory structure (rules, decisions, architecture, artifacts, docs, api)
  2. Missing document IDs in frontmatter
  3. Broken cross-document markdown links (checks file existence)
  4. LanceDB store connectivity
  5. Registry coherence (project-to-vault path mapping)
- **Explicit frontmatter recovery:** `doctor --repair-frontmatter` creates a backup and can remove duplicate IDs, add UUIDs to valid legacy records missing IDs, and recover the known attached-closing-delimiter form. It intentionally refuses arbitrary invalid YAML.
- **Uninstall (`rms-memory uninstall`):** Removes `rms-memory` entries from all discovered IDE configuration files. Uses the same JSONC-aware patcher as the installer with automatic `.bak` backups, making uninstallation as safe and transparent as installation.
- **Hybrid Retrieval Activation:** The `VectorStore::search()` implementation now truly combines vector similarity AND Tantivy full-text search (FTS). Previously, the FTS index was built on table creation but never queried — searches were vector-only. The fix adds a two-tier approach: hybrid search with graceful fallback to vector-only if the FTS index is unavailable.

### 11. Modular Architecture & crates.io Ready
- **Library API (`lib.rs`):** Prepared for ecosystem integration by exposing core components (`store`, `indexer`, `tools`) as a public Rust library. Internal CLI logic remains safely encapsulated.
- **Dependency Injection (`AppContext`):** The system uses a centralized `AppContext` that securely holds the initialized LanceDB connection, embedding models (`fastembed`), and runtime configuration. This eliminates redundant initializations and allows dependency injection across all commands and tools.
- **CLI Commands (`src/commands/`):** The massive `cli.rs` monolith was completely dismantled into individual domain-specific modules (`install`, `init`, `sync`, `gc`, etc.), dramatically improving readability and minimizing Git merge conflicts.
- **MCP Tools (`src/tools/`):** JSON-RPC tool executions are now routed to specialized handler files (`search.rs`, `read.rs`, `write.rs`) under the `src/tools/` module, ensuring clear separation of concerns between standard CLI interactions and autonomous Agent requests.

### 12. CI/CD and Cross-Platform Distribution
- **Strict User-Scoped Isolation:** The core configuration logic was overhauled to enforce a rigid `~/.rms-memory/` standard across all platforms. By utilizing the `directories` crate exclusively to locate the user's home folder, the program correctly targets `C:\Users\username\.rms-memory\` on Windows, `/Users/username/.rms-memory/` on macOS, and `/home/username/.rms-memory/` on Linux without polluting generic OS domains like `AppData/Roaming` or `Library/Application Support/`.
- **GitHub Actions Matrix:** Engineered a seamless `release.yml` pipeline that triggers on repository version tags.
  - Compiles optimized native binaries for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, and `aarch64-apple-darwin`.
  - Packages and uploads them instantly as GitHub Release assets.
- **Single-Line Installers:** `install.sh` (cURL/Bash) and `install.ps1` (PowerShell) automatically detect target system architecture, fetch the optimal GitHub release binary, map it to the exact correct path (`~/.cargo/bin` or user-defined), and add it to standard OS `PATH` vars.

### 13. Generalized Scope Resolver (`--scope`)

The vault system is no longer tied to filesystem paths. Any non-empty string can serve as a scope identifier:

```bash
rms-memory --scope "/home/user/project" serve   # path-based (regression-safe)
rms-memory --scope "thread:abc-123" serve        # arbitrary identifier
rms-memory --scope "lead:acme-corp" serve        # CRM entity
```

- **Blake3 Hashing:** `Workspace::project_hash_for(identifier)` hashes any string deterministically. Path-based scopes produce identical hashes to pre-1.0.3 behavior — no migration needed.
- **Unified Storage:** All vaults live under `base_dir()/dbs/<blake3_hash>/` regardless of scope type. Path-based and opaque scopes never collide.
- **Validation:** Empty strings rejected; max 512 characters; path-like prefixes (`/`, `./`, `../`) are canonicalized; everything else treated as opaque identifier.
- **Auto-Discovery:** For opaque scopes, `Workspace::discover_with_scope()` creates the standard vault directory structure at `base_dir()/vaults/<hash>/` without requiring a registry entry.

### 14. Audit Metadata

Every write operation now injects provenance metadata into the document's YAML frontmatter:

```yaml
---
last_modified_by: OpenCode       # from MCP clientInfo.name
timestamp: 2026-07-11T10:00:00Z  # ISO 8601, updated on every write
created_at: 2026-07-10T08:00:00Z # set once, never overwritten
confidence: 0.85                  # optional 0.0–1.0
source: "SEC filing 10-K, p.42"   # optional citation
---
```

- **Caller Identity:** The MCP `initialize` handler extracts `clientInfo.name` (e.g., "Cursor", "Claude Code", "OpenCode") and stores it as `caller_id` in `AppContext`. Falls back to `"unknown"` if not provided.
- **Automatic Injection:** `tools/write.rs` applies audit fields automatically — agents don't need to pass `last_modified_by` or `timestamp` explicitly. `confidence` and `source` are written only if the agent provides them.
- **Backward Compatibility:** Documents without audit fields parse normally — all fields are `Option`, missing values are `None`.
- **LanceDB Schema Migration:** `Store::open_table()` auto-adds the `confidence` column via `NewColumnTransform::SqlExpressions("CAST(NULL AS FLOAT)")` if missing. FTS index is recreated afterwards. Race-condition safe. Zero-downtime upgrade.
- **Confidence-Aware Search:** `rms_search` accepts `min_confidence` (float 0.0–1.0). Filter is `confidence IS NULL OR confidence >= X` — pre-migration records without confidence are always included, never silently excluded.
- **Two-Level Vaults:** Combine scope + audit for multi-context agents: project-level vault stores high-confidence canon; thread-level vault stores session episodes.

### 15. Security & Robustness Hardening (v1.0.3 audit)

The codebase underwent a 3-agent (Tester + Reviewer + Optimizer) comprehensive audit. Key fixes delivered:

- **Panic-Free Database Layer:** All `unwrap()` / `panic!()` calls in `store.rs` replaced with `Context`-based `Result` propagation. Schema mismatches now return errors instead of crashing the server process.
- **Path Traversal Prevention (3 vectors closed):**
  1. `link:` frontmatter field — `is_safe_link()` rejects absolute paths and `..` components
  2. File system symlinks — `resolve_vault_path()` canonicalizes and validates containment within `workspace_root`
  3. Direct `../` in request paths — `validate_path()` uses `Path::components()` for robust rejection
- **Error Observability:** All previously swallowed errors (`let _ = sync_vault`, `Err(_e) => {}`) now emit `tracing::error!` / `tracing::warn!` diagnostics. Server operators can now see when vault syncs fail, LanceDB connections drop, or requests are malformed.
- **JSON-RPC Compliance:** Malformed requests now return proper `-32700 Parse error` RPC responses. Requests exceeding 1MB are rejected with an explicit error code. Previously both cases resulted in silent client timeouts.
- **Resource Limits:** `rms_search` `limit` parameter capped at 100. Request size limit of 1MB enforced on stdin. File watcher uses `try_send` (non-blocking) instead of `blocking_send` to prevent event flood deadlocks.
- **Code De-duplication:** `VectorStore` trait removed (single implementation = unnecessary abstraction). `CommandRunner` trait removed (enum dispatch existed alongside). Vault directory creation extracted to `create_vault_dirs()`. JSON response wrapper extracted to `tools/response.rs`. Shared path validation extracted to `tools/validation.rs`.

### 16. Performance Hardening (v1.0.5)

Multi-IDE scenarios exposed a CPU storm: 4 processes consuming ~380% CPU, load avg 648, at idle.

- **Thread Pool Reduction:** ONNX `with_intra_threads(1)` (was 2) and tokio `worker_threads=2` (was 12). Per-process thread count cut from ~45 to ~6. Cross-process cascade from `ensure_id()` writes resolved via fast-path fix.
- **Single Model Instance:** `Arc<Mutex<Indexer>>` created once in `McpServer::run()` and shared between search and background synchronization.
- **Path-Based Mtime Cache:** `sync_vault` now uses `get_file_timestamps()` returning `(doc_id, timestamp)` tuples — skipped files correctly tracked in `current_doc_ids`, preventing silent vector deletion.
- **PID-aware coordination:** Background indexing does not mutate Markdown; writer work is serialized by a per-project filesystem lock that records owner PID metadata for diagnostics and stale-lock recovery.
- **`.bak` Filter:** Write-Guard snapshot files are filtered from the Markdown watcher; Rust source watching remains opt-in.
- **Codex IDE:** Auto-installer supports `~/.codex/mcp.json` alongside 11 existing IDEs.
- **Runtime Verified:** load avg 648 → 8.31 (-98.7%), CPU 380% → 0%, memory 2.5GB → ~1.3GB across 3 IDE processes.

### 17. Wiki Context Pack Generator (v1.0.6)

The Wiki Generator assembles deterministic context packs from multiple sources for LLM agents to create human-readable documentation.

- **YAML Manifest:** Defines sections, source types (`vault_search`, `code_search`, `files`, `self_cli_help`), and budget controls (max_chars, max_section_chars, max_item_chars).
- **Retrieval Pipeline:** `RetrievalService` queries vault documents and code chunks through a shared facade — used by both MCP tools and WikiService.
- **RRF Dedup + Semantic Truncation:** Reciprocal Rank Fusion merges multiple queries per section. Stable ID-based dedup removes duplicates. UTF-8 boundary-safe semantic truncation prevents mid-word cuts.
- **Atomic Output:** Context pack, agent task, sources JSON, diagnostics JSON, and manifest YAML written to `wiki/.generation/` via temp-file + rename.
- **Reproducible:** `pack_id` computed from schema version, scope, Git revision, and ordered source hashes.
- **MCP Tool:** `rms_wiki_pack` — agents trigger generation directly from any IDE.
- **CLI:** `rms-memory wiki generate/init/clean` + `rms-memory wiki generate --stdout`.

### 18. Project Label Provenance (v1.0.6)

Every document now carries its originating project identity.

- **`project: <key>` in frontmatter:** Set automatically on first write from registry key. Preserved on updates. Rejected on conflict.
- **Custom YAML Preservation:** `inject_audit_metadata` uses `serde_yaml::Mapping` — user-defined keys survive `replace` operations.
- **Registry Diagnostics:** `rms-memory projects list` and `rms-memory projects locate --vault/--project`.
- **Safe Registry Cleanup:** `rms-memory projects remove <key>` unregisters an accidental mapping while preserving its vault for explicit inspection or deletion.
- **Explicit Destructive Cleanup:** The companion GUI and future transports call the shared `ProjectService`. Permanent cleanup requires an exact project-key confirmation and accepts only a dedicated vault below the configured master vault; filesystem root, master vault, user home, source repository, symlink roots, and external paths are rejected.
- **Responsive GUI Workflow:** Tauri runs filesystem cleanup outside the UI thread. Unsaved configuration blocks both removal actions; after success the GUI reloads the registry, clears stale file selection, and selects a remaining valid scope. Partial filesystem failures are surfaced as structured warnings.
- **Global Vault Fallback Removed:** Bad `rootUri` returns error with client/project/vault logging — no more orphaned files in global vault root.
- **ChatGPT / Codex TOML:** `inject_toml()` patcher for `~/.codex/config.toml` `[mcp_servers]` section.

### 19. Rootless MCP Routing & Deployment Verification (v1.0.6)

Antigravity exposed a protocol edge case: a globally configured MCP process may start with `cwd=/` and omit legacy `rootUri`, even though the project already exists in the RMS registry.

- **MCP Roots negotiation:** clients advertising the Roots capability receive a server-initiated `roots/list` request after `notifications/initialized`. File URIs are decoded safely and deduplicated by resolved vault.
- **Explicit project fallback:** all vault/code tools accept `project: "<registry-key>"`; the unbound `rms_projects` tool exposes valid keys. Zero or multiple matches remain actionable errors instead of cross-project writes.
- **Repository-specific rules:** rule templates substitute the registered key during injection, so an agent knows exactly what to pass when its host omits workspace context.
- **Legacy repair:** `doctor --repair-frontmatter` now handles plain Markdown with no YAML block by creating a backup, inserting one UUID, and leaving the body intact.
- **Production gate:** `build.sh` completed a clean release build, installed and signed `/usr/local/bin/rms-memory`, and a live session from `cwd=/` successfully wrote `architecture/llm-providers-and-global-key-management.md` into the `rms-threads-assistant` vault using only its short project key.
