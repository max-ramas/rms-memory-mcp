# Semantic Codebase Memory — Revised Implementation Plan

## Outcome

RMS Memory will expose two complementary corpora through one MCP server:

- **Vault memory** — human-editable Markdown containing decisions, rules, architecture, and artifacts.
- **Code memory** — a derived, read-only semantic index of the current source tree.

It will also maintain a transport-neutral **knowledge graph** over stable Vault documents and semantic code items. Search chunks are projections for retrieval; they are not stable graph identities.

Markdown remains the source of truth for intent and history. Code chunks describe the implementation that exists now. Code indexing must never write to source files or silently create metadata in them.

The complete knowledge lifecycle is:

```text
Architecture and decisions (Vault) → Current implementation (Code) → Results and evidence (Artifacts)
```

The approved multilingual extension is specified in `docs/multilanguage-code-memory-plan.md`. It preserves this contract while moving Rust behind a language-adapter boundary and adding Go, TypeScript/TSX, JavaScript/JSX, Python, C, C++, Java, Ruby, Swift, and a two-stage Vue SFC pipeline.

## Implementation status (2026-07-13)

- Slice 0 is implemented: fail-closed frontmatter parsing, read-only background sync, cross-process writer lock, watcher retry, repair tooling, and PID-aware lock diagnostics.
- Slice 1 parser spike is implemented for Rust with fixture coverage for outer and inner docs, attributes, nested and documented modules, multiple impl blocks, generics, traits, enums, and undocumented items.
- Slice 2 is implemented: semantic items emit stable, zero-based `segment_index` values; oversized chunks repeat their preamble and declaration signature, split body text at line boundaries when possible, and preserve a bounded character overlap.
- Slice 3 is implemented: manual `reindex --code` builds the separate `code_chunks` table. Dogfood on this repository indexed 38 files into 188 items and 288 segments, with zero skipped files.
- Slice 3.5 is in progress: canonical graph identities, versioned derived-edge keys, provenance/resolution types, and schemas for nodes, edges, and user overrides are implemented. The revisioned, atomically persisted ConfigManager with a change subscription now owns all internal CLI/MCP registry reads/writes. A transport-neutral job manager exposes structured progress, cooperative cancellation, and typed events. Graph tables now persist derived and user records; derived reconciliation is generation-based and override updates use CAS. Extractors and graph queries remain before Slice 4.
- Slice 4 is implemented: `code_chunks` uses stable-id upserts, reuses a previous vector when the segment content hash is unchanged, and removes only ids no longer emitted by parsing. Code reindex emits Rust `use`, trait-implementation, and lexical call relationships as versioned unresolved graph hints; vault reindex/sync emits Markdown `links_to` edges, resolving known documents to `vault:<document_id>`. Release-dogfood timing remains pending.
- Slice 5 is implemented: `rms_code_search` and `rms_search(corpus=vault|code|all)` expose both corpora; `all` uses RRF on source-local ranks. Slices 7–8 remain planned; source watching stays disabled until Slice 7.
- Slice 6 is implemented: three independent writer processes serialize through the project lock, a reader remains available while a writer lock is held, and a killed lock owner releases the OS lock so metadata can be safely cleared.
- Slice 7 is implemented: `ProjectConfig.code_index_mode` defaults to `off`; only `watch` enables Rust source watching. Paths are coalesced into a three-second quiet window, and a successful shared generation marker suppresses duplicate work in processes that lost the writer lock.

## Preconditions

The following stabilization work is required before source watching is enabled:

- malformed Markdown frontmatter fails closed and is reported by `doctor`;
- background vault indexing is read-only;
- vault and code writers use a per-project cross-process lock;
- lock contention has retry/dirty semantics rather than dropping an event;
- embedding uses one ONNX intra-op thread and batches of at most 8;
- CPU, memory, indexing duration, files scanned, chunks embedded/reused, and lock contention are observable.

The first three items are implemented by the CPU-storm fix that accompanies this plan.

## Public configuration

Use one explicit state instead of interacting booleans:

```toml
code_index_mode = "off"       # off | manual | watch; default off
code_index_max_file_kb = 512
```

Semantics:

- `off`: no automatic source scan; an explicit `reindex --code` still runs.
- `manual`: same background behavior as `off`, but records that the project intentionally uses code search.
- `watch`: initial code sync plus debounced source watching.
- `reindex --vault`, `reindex --code`, and `reindex --all` are explicit user actions and override the automatic mode.

The selector for search corpora is named `corpus`, not `scope`; `scope` already identifies a project/vault.

## Storage schema

Create a separate `code_chunks` table in the existing per-project database directory.

| Field | Type | Purpose |
|---|---|---|
| `id` | string | `blake3(item_key + segment_index)` |
| `item_key` | string | Stable semantic identity independent of line numbers |
| `file_path` | string | Path relative to project root |
| `module_path` | string | Rust module nesting inside the file |
| `symbol_name` | string | Human-readable symbol |
| `qualified_symbol` | string | Module-qualified symbol or canonical impl target |
| `kind` | string | function, struct, enum, trait, impl, module_doc |
| `language` | string | `rust` in v1 |
| `start_line` / `end_line` | u32 | Navigation metadata only; never identity |
| `segment_index` | u32 | Stable position of an oversized item segment |
| `item_hash` | string | Hash of the complete parsed item |
| `content_hash` | string | Hash of this searchable segment |
| `content` | string | Preamble plus body segment |
| `embedding` | vector[384] | Multilingual E5 embedding |
| `timestamp` | nullable string | Indexing audit timestamp |

`item_key` is derived from `file_path + module_path + kind + qualified_symbol`. For an impl, the qualified symbol includes the trait, target type, and a deterministic occurrence discriminator when necessary. Duplicate symbol names in nested modules must not collide.

## GUI-ready graph and core boundary

The authoritative graph uses separate `graph_nodes`, `graph_edges`, and `graph_edge_overrides` tables. Edges reference `vault:<document_id>` or `code:<item_key>`, never chunk IDs. Derived rows carry a versioned extractor and generation; user rows are outside reindex ownership. User suppression of a derived edge is persisted as an override so the edge does not reappear on the next reindex.

MCP, CLI, and the future GUI are adapters over one application API. The core exposes structured commands, queries, cancellable jobs, progress, and typed events. HTTP/SSE or Unix-socket transport is deferred until that boundary exists. Configuration access moves behind a revisioned `ConfigManager` with validation, an OS lock, atomic replace, change notifications, and compare-and-swap updates.

The detailed contract is in `docs/gui-ready-core-architecture.md`.

## Chunking contract

- Parse Rust with `tree-sitter` and `tree-sitter-rust`.
- Index functions, structs, enums, traits, impl blocks, and module documentation.
- Attach contiguous `///` comments and intervening attributes to the following item.
- Join the leading contiguous `//!` block into one `module_doc` item.
- Methods remain inside their impl block in v1.
- For an item up to 1500 characters, emit one segment.
- For an oversized item, preserve doc comments, attributes, and signature as a preamble in every segment; split only the body with approximately 200 characters of overlap.
- Segment sizes count Unicode characters. If the repeated preamble itself exceeds the target size, retain it intact and reduce the body budget rather than truncating semantic context.
- Use a new `split_with_preamble` helper. The Markdown `split_large_node` helper cannot satisfy this contract unchanged.

## Incremental update algorithm

For each changed file, while holding the project index lock:

1. Validate extension, ignore rules, and maximum size.
2. Read a stable file snapshot and parse it.
3. Produce items and searchable segments.
4. Load existing rows for `file_path` and key them by `(item_key, segment_index)`.
5. Reuse an embedding only when `content_hash` is identical.
6. Embed changed/new segments in batches of at most 8.
7. Replace the file's rows in one logical transaction: write the new set, then remove superseded rows, or use a staging/generation column if LanceDB cannot make replacement atomic.
8. If the source changed during parsing/embedding, discard the result and schedule one retry.

Deleted, renamed, newly ignored, unsupported, or oversized files remove their old rows. The walker maintains a current-file set so orphan cleanup does not depend on receiving every filesystem event.

## Cross-process coordination

All IDE processes may read. Only one process may mutate project index tables at a time.

- Use `dbs/<project-hash>/.index.lock` for vault and code writers.
- Write owner PID and acquisition timestamp into the lock file for diagnostics.
- Watcher-triggered work uses `try_lock`; on contention it retains a dirty generation and retries with bounded jitter.
- Manual reindex waits asynchronously and reports how long it waited.
- The lock covers scan decisions, parsing, embedding, and commit so two processes cannot embed the same generation concurrently.
- Reads never acquire the writer lock.
- OS lock release after crash is verified with a killed subprocess test.
- The OS lock is authoritative; PID existence alone never authorizes unlinking the file. `doctor` may clear stale owner metadata only after successfully acquiring the lock itself, avoiding PID-reuse and split-inode races.

A standalone daemon is not required for v1. A leader/lease process can be considered later only if measurements show that loading one embedding model per IDE remains too expensive at idle.

## MCP surface

Extend `rms_search` with:

```json
{
  "query": "where is vault sync coordinated",
  "corpus": "vault",
  "limit": 10,
  "include_content": true,
  "min_confidence": 0.5
}
```

`corpus` values:

- `vault` — default; fully backward compatible;
- `code` — code table only;
- `all` — query both and merge.

Also expose `rms_code_search(query, limit, include_content)` for tool discoverability and the common code-only path.

Every result includes `source: vault | code`. Code results expose file path, qualified symbol, kind, line range, and segment index. `min_confidence` applies only to vault rows; code rows have no confidence field and are not filtered out.

Current `_distance` values use lower-is-better semantics, but `corpus=all` never merges raw distances. It uses **Reciprocal Rank Fusion (RRF)** over independently ranked vault and code results. RRF is robust to different score distributions and future retrieval changes. Apply `limit` only after fusion; document the rank constant and deterministic tie-breaker.

## Delivery slices

### Slice 0 — Stabilization gate

- Land malformed-frontmatter failure, read-only background indexing, cross-process lock, watcher retry, and doctor repair.
- Repair the known corrupted vault files and rebuild affected indexes.
- Acceptance: five simultaneous IDE processes remain at approximately 0% CPU with no file changes; no Markdown mtime changes during sync.

### Slice 1 — Parser spike and fixtures

- Add tree-sitter dependencies and Rust fixtures.
- Cover `///`, `//!`, attributes, nested modules, multiple impls, generics, and items without docs.
- Acceptance: every fixture has an asserted `item_key`, line range, kind, and preamble.

### Slice 2 — Preamble-aware segmentation

- Implement `split_with_preamble` and `segment_index`.
- Acceptance: every oversized segment contains the full signature/docs; body overlap is measured; IDs are unique and stable. **Implemented and covered by parser unit tests.**

### Slice 3 — Code table and manual full index

- Add schema, migrations, Rust walker, hard excludes, nested `.gitignore`, and file-size limit.
- Implement `reindex --code` and `reindex --all` only; no automatic watching.
- Acceptance: dogfood on this repository, record scan time, parse time, embed time, peak RSS, file count, item count, and segment count. **Implemented; initial dogfood: 38 files, 188 items, 288 segments, 0 skipped. Fine-grained timing/RSS telemetry remains part of the release gate.**

### Slice 3.5 — GUI foundation

- freeze canonical graph node/edge keys, provenance, resolution state, and override semantics;
- add graph table schemas and migration/round-trip tests; **implemented: on-demand table creation, generation reconciliation, user-row preservation, and CAS overrides**;
- route registry reads/writes through a revisioned, atomically persisted `ConfigManager`; **implemented, with a cross-process lock, CAS, atomic replace, and `notify` watcher**;
- introduce transport-neutral job progress and event interfaces; do not add HTTP yet; **implemented with bounded typed events and cooperative cancellation**;
- acceptance: derived reconciliation cannot mutate user edges; suppressed derived edges stay suppressed; concurrent config updates produce a conflict rather than lost data.

### Slice 4 — Incremental replacement and embedding reuse

- Implement `(item_key, segment_index)` matching, content-hash reuse, and orphan cleanup. **Implemented: stable id upsert preserves line metadata updates while exact content hashes reuse their existing vector; only absent ids are deleted.**
- Extract Markdown links, Rust `use`, trait impl, and explicitly unresolved call-symbol relationships into the graph during reconciliation. **Implemented: Rust extraction through `rust-tree-sitter-v1` makes module-level `use` declarations, trait impls, and call syntax derived edges with `resolution=unresolved`; `markdown-links-v1` maps known links to `vault:<document_id>` and preserves unknown links as unresolved external nodes.**
- Acceptance: adding lines above an item changes line metadata but neither ID nor embedding; editing one function embeds only its changed segments; deletion removes all rows.

### Slice 5 — Search APIs

- Add `rms_code_search` and `rms_search(corpus=...)`.
- Add code result metadata and merged ranking.
- Acceptance: JSON-RPC tests cover all corpora, absent code table, empty index, `min_confidence`, lower-is-better distance handling, and limit-after-merge. **Implemented at the Store/MCP-handler layer: absent code table returns an empty corpus; `min_confidence` remains Vault-only; `corpus=all` runs independent searches then applies deterministic RRF with rank constant 60 and truncates only after fusion.**

### Slice 6 — Multi-process verification

- Exercise three concurrent manual reindexes and simultaneous reads.
- Test lock contention, dirty retry, process crash, and source changes during embedding.
- Acceptance: one writer embeds each generation, readers remain available, no lost update, and no stale lock after crash. **Implemented: a three-child-process test observes non-overlapping writer sections; a live reader test succeeds while the lock is held; crash recovery verifies OS lock release and safe stale metadata clearing.**

### Slice 7 — Opt-in source watcher

- Enable `code_index_mode=watch` with event coalescing by file path and a three-second quiet window.
- Maintain a dirty generation rather than an unbounded event queue.
- Acceptance: five IDE processes plus rapid saves produce one logical reindex generation and return to idle CPU. **Implemented: `.rs` paths outside generated/excluded directories are coalesced for three seconds; the first writer records a completion marker while holding the writer flow, and contending processes drop the same dirty generation once the marker is newer than their observed save.**

### Slice 8 — Release gate

- Dogfood on `rms-memory-mcp` and at least one larger Rust workspace.
- Run correctness, resource, migration, restart, and corruption-recovery tests.
- Update README, CLI help, MCP tool descriptions, roadmap, and changelog.

## Explicit non-goals for v1

- Adding a grammar without production fixtures, stable identity, incremental reuse, watcher coverage, and real-project dogfood. Multilingual support is now planned, but unqualified adapters remain experimental.
- Compiler-accurate Rust name resolution or a complete call graph.
- Method-level impl chunking.
- Editing source files through MCP.
- A permanent background daemon.
- Shipping the GUI or binding a local HTTP server before the transport-neutral core API exists.
- Dynamic resource throttling beyond fixed thread/batch/file-size limits.
