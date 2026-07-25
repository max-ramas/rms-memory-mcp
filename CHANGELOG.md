# Changelog

All notable changes to this project will be documented in this file.

## [1.0.8] - 2026-07-26

Multi-project MCP routing fix. Companion GUI adopts the **same `1.0.8` product version**.

**Why not 1.0.7:** `1.0.7` already shipped with sticky single-vault binding (`already bound to project 'X', not 'Y'`). Agents in other registered repos could not write their own vaults. That contract is superseded here.

### Fixed
- **MCP multi-project rebind:** explicit `project` on any tool call switches the active vault (cancels previous watchers, opens the target store). Sticky “already bound… refusing to switch” is removed — one Cursor MCP process can serve all registered projects. Ambiguity without `project` remains fail-closed (no silent pick-first / no global-vault fallback).
- **Empty `roots/list` → process cwd:** when Cursor advertises roots but returns no registered project, MCP tries `Workspace::discover(cwd)` before staying unbound. Same fallback on the first tool call without `project` while unbound.
- **Wrong-vault binding on sibling paths:** workspace discovery compared a byte-level string prefix, so a registered `/repo` also captured `/repo-old`, `/repo2`, and `/repository`. Combined with the new cwd fallback this could route reads *and writes* into a neighbouring project's vault. Discovery now uses a component-aware `Path::starts_with` on canonical paths and picks the longest match by path-component depth.
- **Failure-atomic rebind:** `bind_workspace` cancelled the active watchers *before* opening the target store, so a failed open left the previous project bound but watcher-less. The new store is opened and validated first; old watchers are cancelled only after that succeeds.
- **`inject-rules` fail-closed:** standalone `rms-memory inject-rules` on an unregistered directory fell back to the folder basename and wrote a mandatory `project: "<basename>"` that `rms_projects` cannot resolve. The single-path command now refuses unregistered paths; `--all` (registry-sourced) and `init` (basename fallback for the not-yet-registered case) are unchanged.
- **Installer 404:** `install.sh` / `install.ps1` requested `rms-memory-<target>.{tar.gz,zip}` while releases publish `rms_memory_mcp_<target>.*`. Both one-line installers now request the published asset names.
- **Stale packages in published releases:** the persistent self-hosted release target retained `.deb`/`.rpm` from earlier versions — a `1.0.6` RPM shipped inside the `1.0.7` release. Packaging now clears `$CARGO_TARGET_DIR/{debian,generate-rpm}` before regenerating, so only the current version is uploaded.
- **Manual release dispatch could attach the wrong revision:** `workflow_dispatch` accepted `release_tag` but every job checked out the branch tip, and the Cargo version was validated only in the late crates.io job. `plan` now checks out the requested ref and verifies `Cargo.toml` against the tag *before* any build; `build`, `release`, and `publish-crates` check out that same tag.

### Added
- **`rms-memory inject-rules [--all]`:** re-injects managed IDE rule blocks with the concrete registry key without re-running `init` registration. Templates now **require** `project: "<key>"` on every memory tool call (not only when unbound).
- **Full CLI config coverage:** `rms-memory config` gains `--max-backups N` (global) and per-project `--include` / `--exclude` (validated globs); `--auto-import` validated; any flag is fully non-interactive; flagless run prints global **and** matched-project settings.

### Changed
- `rms_system_instructions` describes an *active* project and states that explicit `project` rebinds.
- ADR: `decisions/mcp-explicit-project-rebind.md` supersedes the refuse-to-switch clause of `mcp-workspace-routing-roots-and-project-fallback-2026-07-16`; follow-up `decisions/mcp-cwd-fallback-and-mandatory-project-key.md`.

### Verification
- Unit: sticky refuse strings gone; rule templates require `Always pass project:`; empty-roots cwd fallback path present; inject basename fallback; config flag/glob/strategy tests; sibling-prefix routing regression (`/repo` must not match `/repo-old`, `/repo2`, `/repository`); **154 lib tests green**.
- Pre-release audit closed six findings (3 × P1, 3 × P2) covering installer asset names, stale release packages, sibling-prefix vault binding, rebind atomicity, `inject-rules` key invention, and manual-dispatch revision pinning.
- Gates re-run after the fixes: `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`; both release workflows parse as YAML; `bash -n scripts/install.sh`.
- Manual: `inject-rules --all` updated 14/14 registered projects; `rms-ds.com` rules contain `project: "rms-ds.com"`.
- Release binary installed via `build.sh` → `/usr/local/bin/rms-memory` **1.0.8** (restart IDE MCP processes to pick up).
- Companion GUI **1.0.8** (unified tag / numbering).

## [1.0.7] - 2026-07-25

Bounded recall, vault-native knowledge lifecycle, and session continuity (second-brain mapping onto Markdown + LanceDB, not a SQL memories table). Companion GUI adopts the **same `1.0.7` product version** (shared GitHub Releases tag / numbering).

### Added
- **`rms_search` decision envelope:** responses include `{ decision: inject|abstain, reason, injected_ids, results }` (additive; existing `results` shape preserved).
- **`max_chars`:** total injected content budget (default **2000**); weaker hits truncated/dropped to stay in budget.
- **`min_score`:** optional relevance floor (0..1). Best hit below threshold → **abstain** with empty `results` (fail-closed).
- **Fail-closed search:** index/embedding failures return an abstain envelope instead of a hard tool error for agents.
- **Supersession frontmatter:** `status` (`active`|`draft`|`superseded`), `supersedes`, `superseded_by`; Lance default recall keeps NULL/active/draft and excludes superseded.
- **Soft supersede on `rms_write`:** optional `supersedes: <relative vault path>` stamps the new note and marks the predecessor superseded with bidirectional ids.
- **Temporal validity:** optional `valid_from` / `valid_until` / `learned_at` in frontmatter; indexed and applied in `vault_recall_filter`.
- **Doctor `[7/7] Knowledge freshness`:** expired `valid_until`, broken supersession links (issues); old `learned_at` without `valid_until` (informational).
- **Query-aware FTS prefer:** short keyword/path queries (≤3 tokens, ≤48 chars) try Lance FTS-only first, then fall back to hybrid; envelope may include `retrieval_mode: fts_prefer|hybrid`.
- **Checkpoint MCP tools:** `rms_checkpoint_save` / `done` / `load` / `query`. Checkpoints are plain Markdown under `artifacts/checkpoints/` (`type: checkpoint`, `status: active|done`, `goal`, `pending`, `links`); closing writes a durable session summary under `artifacts/sessions/`. `status: done` drops out of the default recall filter automatically — no new storage, no tiers.
- **`rms_overview`:** structured orientation for exactly one project — document counts by folder/status, recent notes, active checkpoints, coverage metadata. Fail-closed scoping: requires a bound workspace or explicit `project`; never aggregates across projects.
- **`rms_system_instructions`:** returns the canonical memory-usage protocol so agents can self-bootstrap without injected rule files.
- **Structured MCP responses:** continuity tools return `structuredContent` alongside pretty-JSON text (one contract for agents and the GUI).
- **`rms-memory hook` CLI (editor-agnostic L3):** `--event session_start|pre_compact|session_stop [--project <key>] [--cwd <dir>] [--apply --name … --goal … --pending … --summary …]`. Machine-readable JSON on stdout; Cursor/VS Code/Neovim/CI wire thin adapters to this CLI instead of any IDE-specific format. Fail-closed project resolution (explicit key or unique registry match; no global fallback).
- **Installer capability matrix:** `rms-memory install` prints an honest L1 (MCP config) / L2 (rules injection) / L3 (continuity hooks) report with a ready-to-paste Cursor `hooks.json` adapter example.
- **Frontmatter:** `goal`, `pending`, `links`, `done_at` fields for checkpoint/session notes; **`pinned`** bypasses temporal/`min_confidence` recall gates (status still applies); indexed as Lance `pinned='true'`.
- **`rms_read` `noPromote`:** accepted for explicit read-without-side-effects contracts (vault reads were already side-effect free).
- **L3 installer adapters:** `rms-memory install` writes thin Cursor `hooks.json` entries + shared/Claude/Neovim scripts that call `rms-memory hook`; uninstall strips managed hooks only.
- ADR: vault `architecture/bounded-recall.md`.

### Changed
- ROADMAP: stale **pruning** (future/v2) is distinct from vault-native **supersession** + temporal recall gates.

### Fixed
- **NULL confidence round-trip:** `extract_results` no longer maps Arrow null `confidence` to `Some(0.0)`; unset confidence stays `None` after search (fail-open `min_confidence` semantics preserved for agents reading hits).
- **L3 hook script path escaping:** installer embeds the executable path as a single-quoted bash literal (neutralizes `$`, backticks, spaces, and quotes from `current_exe` / CLI install paths).
- **Invalid Cursor `hooks.json`:** parse failures now `tracing::warn!` + print the path before backing up and rewriting managed hooks (same visibility pattern as Zed JSONC skip).

### Known issue (fixed in 1.0.8)
- Sticky MCP bind refused `project` switches after the first vault — broke multi-repo Cursor/global MCP use. See **1.0.8**.

### Verification
- Unit/integration: search abstain/budget/RRF; soft supersede; vault recall filters; freshness datetime parse; `prefers_fts_query`; **real Lance FTS search excludes `status=done`/`superseded` while keeping `active`**; **NULL confidence passes `min_confidence` but weak score still abstains under `min_score`**; checkpoint save→update→load→done; hook project resolution refusals; full suite green.
- Manual: `hook --event session_start --project rms-memory-mcp` returns the live overview JSON; unregistered `--cwd` exits 1.
- `cargo check` / targeted `--lib` filters clean.
- Companion GUI **1.0.7**: Search + Continuity tabs; `serde_gui` camelCase boundary; About Pro UX (`licenseExpiresAt`, no leftover trial chrome); entry JS budget **152 KiB**; docs/CHANGELOG aligned.

## [1.0.6] - 2026-07-23

Line closed 2026-07-23. Includes everything shipped after **1.0.5** (wiki generator through path-scoped watcher, write isolation, and docs).

### Added
- **Reliable MCP project routing:** clients that omit legacy `rootUri` are resolved through MCP `roots/list`; every vault/code tool also accepts an explicit short `project` key, and the unbound `rms_projects` tool lists valid keys.
- **Safe registry removal:** `rms-memory projects remove <key>` removes an erroneous project mapping without implicitly deleting its vault files.
- **Transport-neutral project lifecycle API:** `ProjectService` keeps unregister and permanent data deletion as separate operations. Destructive deletion requires the exact project key, is restricted to a dedicated child of the configured master vault, removes only the vault and derived index, and never touches source code.
- **Companion GUI lifecycle controls:** project settings expose separate unregister and permanent-delete actions, exact-key confirmation, an unsaved-settings guard, non-blocking Tauri execution, and safe scope refresh after removal.
- **Navigable code structure graph:** every code reindex derives a deterministic `project → folder → file → symbol` hierarchy using resolved `contains` edges under the versioned `code-structure-v1` extractor.
- **Wiki Context Pack Generator (`rms-memory wiki`):** assembles verified context packs from vault documents, code index, project files, and CLI help. Produces deterministic `context-pack.md` + `agent-task.md`. Supports custom YAML manifests with budget controls, RRF dedup, and semantic truncation.
- **`rms_wiki_pack` MCP tool:** JSON-RPC wrapper over `WikiService::generate()`.
- **`RetrievalService`:** public facade over `Store` — used by MCP tools and `WikiService`.
- **`rms-memory projects`:** CLI `list` / `locate --vault/--project` for registry diagnostics.
- **Project Label Provenance:** `project: <key>` in YAML frontmatter — set on first write, preserved on updates, rejected on conflict. Custom keys preserved via `serde_yaml::Mapping`.
- **Codex / ChatGPT IDE support:** TOML-aware installer patcher (`inject_toml`) for `~/.codex/config.toml` `[mcp_servers]`.
- **Centralized Wiki path policy** (`src/path_policy.rs`): case-insensitive `<vault>/wiki/**` exclusion shared with the companion GUI; Doctor check **Wiki index isolation**.
- **DocumentService wiki-safe channel** (`read_wiki` / `write_wiki` / `create_wiki` / `delete_wiki`): containment + ETag without audit-frontmatter injection for managed Wiki pages.
- **Path-scoped code watcher reindex** (`try_index_code_paths`): dirty-path set from notify events; segment + graph patch without full generation prune; full-walk fallback when cold / empty / >200 paths / channel overflow.
- **`Store::upsert_derived_graph_patch`** for incremental graph upserts.
- **`SECURITY.md`**, **`NOTICE`**, and **`scripts/bench_large_vault.sh`** large-fixture perf smoke.

### Security
- **Path traversal hardening (wiki):** `resolve_files` canonicalizes paths, validates workspace containment, and hard-excludes `.env*`, `*secret*`, `*.pem`, `*.key`.
- **Global vault fallback removed:** bad or missing `rootUri` returns an error with diagnostic logging.
- **Panic-free write tool:** `inject_audit_metadata` returns `Result` — project conflict is recoverable.
- **Vault-aware `link:` resolution:** after resolving frontmatter links, paths must canonicalize inside the vault; symlink targets outside the vault are rejected on read and write.
- **Wiki write isolation:** `rms_write` requires `.md` and rejects `wiki/**`; canonical DocumentService list/read/write/create/rename/delete exclude or reject `wiki/**` via `path_policy`.
- **`rms_wiki_pack` manifests** must resolve under the vault (no absolute escape, `..`, or symlink escape).

### Fixed
- **Release Linux self-hosted:** skip `sudo apt-get` (no passwordless sudo); verify `pkg-config`/`openssl` instead. Apt install remains only on GitHub-hosted Linux matrix rows.
- **CI cost:** persistent `$HOME/.cache/rms-ci/.../target` on Contabo; no mid-build cancel; release `workflow_dispatch` can build `linux-x64` only (skip paid macOS/Windows minutes); registry cache without uploading multi-GB targets.
- **Fastembed init under parallel tests/CI:** `Indexer::new` serializes model load, retries hf-hub blob lock / ONNX retrieve failures, skips process CWD changes, and only overrides `TMPDIR` when system temp is not writable (avoids tempfile+gitignore flakes).
- **Antigravity workspace initialization:** globally launched MCP processes no longer depend on process CWD (`/`). Injected agent rules carry the repository's concrete registry key.
- **Plain Markdown frontmatter repair:** `doctor --repair-frontmatter` backs up legacy Markdown without YAML, adds one stable UUID, preserves the body.
- **Thread pool reduction:** ONNX `with_intra_threads(1)` + tokio `worker_threads=2`.
- **Fast-path skip fix:** `get_file_timestamps()` returns `(doc_id, timestamp)`.
- **Single `Arc<Mutex<Indexer>>`:** shared between search handler and background sync.
- **`--refresh-code` works:** `WikiGenerateRequest.refresh_code` triggers `code_indexer::index_code_full()`.
- **Per-command `self_cli_help`:** uses `find_subcommand().render_help()`.
- Clippy `collapsible_if` in `project_migrate.rs` rollback path.

### Changed
- **Fail-closed resolution order:** explicit scope / legacy `rootUri` → negotiated `roots/list` → tool-level `project` key. Process CWD only for clients without Roots and never when it is `/`.
- **Project-aware agent rules:** injected Cursor, Claude, Zed, Gemini, and general rules include the concrete registry key.
- **`inject_audit_metadata` rewritten:** `serde_yaml::Mapping` preserves custom user YAML keys.
- **`WikiService` uses `RetrievalService`** instead of direct `Store` calls.
- **`ignore::WalkBuilder` in `resolve_files`:** nested `.gitignore` support.
- **`pack_id` includes Git revision** for reproducible builds.
- **CLI commands unified:** `rms-memory projects`, `rms-memory wiki`.
- **Graph Store Query API:** `Store::query_graph_nodes()` / `query_graph_edges()`; `from_string()` / `into_string()` on graph keys.
- Vault Markdown discovery, incremental/full indexing, Markdown/code watchers, code walking, federated search, graph reconciliation and Wiki context-pack input exclude `<vault>/wiki/**` consistently.
- Legacy Wiki-derived records are purged by normalized path during sync/reindex while files stay on disk.
- MCP remains AI-free: organizer/Wiki LLM orchestration lives only in the GUI.
- Embedding batch size raised 8→32 for vault and code indexing (intra-threads remain 1).
- Semantic graph embeds selected node queries in one batch instead of one ONNX call per node.
- Docs: unsigned/manual macOS GUI distribution and Gatekeeper workaround documented until Apple notarization; companion GUI secrets `RMS_LICENSE_PUBLIC_KEY` + `RMS_MEMORY_MCP_TOKEN` noted.

### Verification
- Installed-binary routing gate from `cwd=/` with short project key.
- `cargo test --lib`: 122 passed (incl. `path_scoped_*`, wiki isolation, containment).
- `cargo clippy --all-targets -- -D warnings`: clean.

## [1.0.5] - 2026-07-13

### Added
- **Semantic code parser spike:** Tree-sitter Rust extraction now produces stable semantic items for functions, structs, enums, traits, impl blocks, and module docs, with preamble-aware fixtures covering nested modules, attributes, generics, and multiple inherent impls.
- **Go semantic code adapter:** the 1.0.5 code corpus now dispatches Rust and Go through a language registry. Go indexing covers package docs, functions and receiver methods, structs, interfaces, aliases, constants, variables, import/call graph hints, stable IDs, preamble-aware chunks, and code-search language metadata.
- **Multilanguage code adapters:** the registry now dispatches JavaScript/JSX, TypeScript/TSX, Python, C, C++, Java, Ruby, Swift, and Vue SFC inline scripts in addition to Rust and Go. C/C++/Java/Ruby/Swift emit conservative unresolved import/include and call hints; Vue maps embedded JS/TS symbols back to the `.vue` host file.
- **Per-project language policy:** `code_languages = ["auto"]` is the backwards-compatible default. `rms-memory config --code-languages <comma-list>` validates and persists an allow-list used consistently by manual reindex and the watcher.
- **Preamble-aware code segmentation:** oversized semantic items receive stable segment indexes; each segment repeats its documentation, attributes, and declaration signature while retaining bounded body overlap.
- **Manual semantic code indexing:** `rms-memory reindex --code` now builds an isolated LanceDB `code_chunks` table from supported source files while respecting nested `.gitignore`, hard exclusions, a 512 KiB file limit, and embedding batches of eight.
- **GUI-ready graph foundation:** canonical Vault/code/external node keys, versioned derived-edge identities, provenance and resolution contracts, plus separate graph node, edge, and user-override schemas prepare the core for editable visual relationships without tying it to MCP or HTTP.
- **Revisioned configuration core:** CLI and MCP configuration access now go through `ConfigManager`, which uses a cross-process lock, compare-and-swap revisions, atomic persistence, subscriptions, and a file watcher while preserving the last valid snapshot after malformed external edits.
- **Transport-neutral jobs and events:** `JobManager` provides structured progress, cooperative cancellation, terminal-state protection, and bounded typed event subscriptions for future CLI, MCP, and GUI adapters.
- **Durable editable graph core:** graph nodes, derived/user edges, and overrides now persist in separate LanceDB tables. Reconciliation upserts complete generations before pruning stale derived rows, while manual rows and suppress/restore overrides remain intact; override writes use compare-and-swap revisions.
- **Incremental code embeddings:** code reindex now upserts stable segment ids, reuses vectors for unchanged content hashes, and deletes only no-longer-emitted segments instead of recreating the whole code table.
- **Rust relationship hints:** code reindex materializes module `use` declarations, trait implementations, and function/method call syntax as extractor-versioned graph edges. They are explicitly marked unresolved lexical hints, not a compiler-accurate call graph.
- **Markdown relationship graph:** vault full indexing and changed-file sync now materialize `links_to` edges. Known document paths resolve to stable Vault nodes; missing targets are retained as unresolved external nodes.
- **Federated retrieval:** `rms_search` now accepts `corpus=vault|code|all`, and `rms_code_search` exposes a code-only path with file/symbol/line metadata. Mixed-corpus retrieval uses deterministic Reciprocal Rank Fusion rather than incompatible raw vector distances.
- **Multi-process verification:** regression tests now exercise three independent writer processes, concurrent reader availability, and lock-owner crash recovery for the per-project index lock.
- **Opt-in code watcher:** project configuration now supports `code_index_mode = "off" | "manual" | "watch"` (default `off`). Watch mode debounces enabled supported source paths for three seconds and shares a completed-generation marker to prevent duplicate cross-IDE reindexes.
- **Codex IDE support:** `rms-memory install` now auto-injects into `~/.codex/mcp.json` alongside existing IDEs.

### Fixed
- **Memory frontmatter integrity:** `rms_write` now places the closing YAML delimiter on its own line and assigns an ID to newly created or legacy ID-less records. `doctor --repair-frontmatter` recovers the known attached-delimiter form and adds UUIDs to valid legacy records without IDs, always after creating backups.
- **Code-index merge collisions:** conflicting parser segments that share a nominal stable ID now receive a deterministic content-hash suffix instead of aborting the entire LanceDB merge. CLI command failures now return a nonzero exit status.
- **Self-sustaining CPU storm:** malformed YAML frontmatter is now reported as an error instead of being treated as missing metadata. Background indexing no longer calls `ensure_id()` or writes to Markdown files.
- **Cross-IDE indexing amplification:** vault sync and full reindex use a per-project filesystem lock. Watcher sync retries after lock contention while manual commands wait asynchronously.
- **Lock diagnostics:** `.index.lock` records owner PID and acquisition time. `doctor` reports active owners and clears stale metadata only after acquiring the OS lock; it never unlinks based only on PID state.
- **Frontmatter recovery:** `rms-memory doctor --repair-frontmatter` removes duplicate top-level `id:` keys after creating a timestamped backup. `--repair-path` can target one file inside a registered vault. Other YAML errors are never rewritten automatically.
- **Watcher noise:** automatic sync now reacts only to Markdown files and continues to ignore backups.
- **Generated code exclusions:** semantic code indexing excludes `.next`, `.nuxt`, `node_modules`, `target`, `vendor`, `coverage`, `.git`, and `.rms-memory`; generic `build` and `dist` remain available unless ignored by the project itself.
- **C/C++ header ambiguity:** `.h` is deterministically indexed as C once; `.hpp`, `.hh`, and `.hxx` select C++.
- **Watcher generation marker flake:** duplicate-generation suppression now compares the precise completion timestamp stored in `.code-index.updated`, with filesystem `mtime` retained only for legacy markers. This avoids timestamp-resolution failures on GitHub Actions and networked filesystems.
- **Atomic Markdown writes:** `rms_write` `create` and `replace` now write and fsync a same-directory temporary file before atomically replacing the target and syncing its directory. This prevents a frontmatter-only or truncated vault file from becoming visible if a write is interrupted.

### Performance
- **Thread Pool Reduction:** ONNX `with_intra_threads(1)` (was 2) and tokio `worker_threads=2` (was 12). Per-process thread count cut from ~45 to ~6. Runtime verified: load avg 648 → 8.31 (-98.7%), CPU 380% → 0% idle across 3 IDE processes.
- **Real-project stress gate:** concurrent GeoMail, License Server, RMS Monitoring, and GeoTax Site reindexes completed; after restarting four IDEs, seven `serve` processes remained at `0.0%` CPU with no background reindex.

## [1.0.4] - 2026-07-12

### Fixed
- **Single Indexer instance:** Each watcher-triggered sync previously created a new `Indexer::new()` (loading ONNX model: 100-200MB RAM, 100-300ms). Now one `Arc<Mutex<Indexer>>` is created in `McpServer::run()` and shared between search handler and background sync. Eliminates N model reloads — idle CPU drops from ~380% to near-zero with 4 IDE processes.
- **Path-based mtime check:** `sync_vault` now skips parsing unchanged files using `get_file_timestamps()` (path-keyed timestamps from LanceDB `path` column). Previously every file was parsed on every sync — chicken-and-egg: `doc_id` for mtime check required frontmatter parsing.
- **Watcher `.bak` filter:** File watcher ignores Write-Guard snapshot files (`*.bak`), preventing self-triggering sync cycles.
- **Watcher trigger logging:** `tracing::info!` with triggering file path on each watcher event (previously silent).

## [1.0.3] - 2026-07-12

### Added
- **Generalized Scope Resolver (`--scope`):** String-based identifier system replacing path-only vault isolation. `blake3(identifier)` supports arbitrary scopes (`"thread:12345"`, `"lead:acme-corp"`) alongside filesystem paths. Unified `base_dir()/dbs/<hash>/` for all scopes — no regression for existing projects.
- **Caller Identity Tracking:** MCP `initialize` extracts `clientInfo.name` → `caller_id` in `AppContext`. `last_modified_by` reflects the actual editing client (Cursor, Claude Code, OpenCode).
- **Audit Metadata:** Five YAML frontmatter fields: `last_modified_by` (auto), `timestamp` (ISO 8601, updated per write), `created_at` (set once), `confidence` (0.0–1.0, LanceDB-indexed), `source` (free-text citation). All `Option` with `#[serde(skip_serializing_if)]` — no clutter for unset fields.
- **Confidence-Aware Search:** `rms_search` accepts `min_confidence`. Filter: `confidence IS NULL OR confidence >= X` — pre-migration records without confidence are always included. Agent guidance in tool description warns against starting with high thresholds.
- **Zero-Downtime Schema Migration:** `Store::open_table()` auto-adds `confidence` column via `NewColumnTransform::AllNulls` if missing. FTS index recreated. Race-condition safe. No manual `reindex`.
- **Project-Level Vault Pattern:** `docs/multi-scope-usage.md` — two-level architecture (product canon vs. thread episodes). Caller merges results client-side.
- **3-Agent Audit Completed:** Tester (44) + Reviewer (34) + Optimizer (16) findings. 94 total, cross-referenced into 11 fix groups. All critical/high bugs closed.

### Security
- **Path Traversal (3 vectors):** `is_safe_link()` in `link.rs` rejects absolute/`..` frontmatter paths. `resolve_vault_path()` in `tools/validation.rs` canonicalizes + validates symlink containment. Shared `validate_path()` uses `Path::components()` across read/write.
- **Panic-Free Database Layer:** 12 `unwrap()`/`panic!()` in `store.rs` replaced with `Context`-based `Result`. Server returns errors instead of crashing on schema mismatch.
- **Request Size Limit:** 1MB cap on stdin reads + search `limit` clamped to `min(100)`.
- **JSON-RPC Compliance:** Malformed requests return `-32700 Parse error`. Oversized requests return `-32700 Request too large`.

### Fixed
- **sync_vault timestamps:** `unwrap_or_default()` → explicit error log — no more mass-reindex on transient DB errors.
- **Sync error logging:** `let _ = sync_vault(...)` → `tracing::error!` — silent vault staleness eliminated.
- **Store init errors:** `Err(_e) => {}` → `tracing::error!` — LanceDB connection failures now visible.
- **File watcher channel:** `blocking_send` → `try_send` — prevents thread blocking under event flood.
- **Installer macOS unwrap:** `.to_str().unwrap()` → `.unwrap_or("")`.
- **Rules injector gitignore:** `unwrap_or_default()` → error-logged fallback — no silent overwrite.
- **Scope ≠ rootUri mismatch:** `scope: Option<String>` — `None` auto-detects from rootUri at connect time, restoring v1.0.2 behavior.
- **`rms_search` guidance:** Tool description warns against starting with high `min_confidence`.
- **SQL migration:** `CAST(NULL AS FLOAT)` → `AllNulls(ArrowSchema)` for guaranteed NULL.

### Changed
- **`CommandRunner` trait removed** — direct method calls on `Args` structs instead of trait dispatch. Depends on `--scope` via parameter.
- **`VectorStore` trait removed** — `search()` and `read_document()` made inherent `pub` methods on `Store`.
- **Shared `tools/response.rs`** — single `json_text_response()` replaces duplicate `json!({...})` across search/read/write.
- **Shared `tools/validation.rs`** — single `resolve_vault_path()` with symlink safety.
- **DRY vault dirs:** `create_vault_dirs()` — 3 copy-pasted sites → 1 function.
- **`Frontmatter`** extended, `SearchResult` carries `confidence`, codebase audit cleanups.

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
