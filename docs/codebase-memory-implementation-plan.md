# Semantic Codebase Memory — Revised Implementation Plan

## Outcome

RMS Memory will expose two complementary corpora through one MCP server:

- **Vault memory** — human-editable Markdown containing decisions, rules, architecture, and artifacts.
- **Code memory** — a derived, read-only semantic index of the current source tree.

Markdown remains the source of truth for intent and history. Code chunks describe the implementation that exists now. Code indexing must never write to source files or silently create metadata in them.

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

## Chunking contract

- Parse Rust with `tree-sitter` and `tree-sitter-rust`.
- Index functions, structs, enums, traits, impl blocks, and module documentation.
- Attach contiguous `///` comments and intervening attributes to the following item.
- Join the leading contiguous `//!` block into one `module_doc` item.
- Methods remain inside their impl block in v1.
- For an item up to 1500 characters, emit one segment.
- For an oversized item, preserve doc comments, attributes, and signature as a preamble in every segment; split only the body with approximately 200 characters of overlap.
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
- Watcher-triggered work uses `try_lock`; on contention it retains a dirty generation and retries with bounded jitter.
- Manual reindex waits asynchronously and reports how long it waited.
- The lock covers scan decisions, parsing, embedding, and commit so two processes cannot embed the same generation concurrently.
- Reads never acquire the writer lock.
- OS lock release after crash is verified with a killed subprocess test.

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

Current `_distance` values use lower-is-better semantics. Before `corpus=all` ships, verify that both tables use the same metric and retrieval mode. Convert distances to a documented normalized relevance value or merge by reciprocal-rank fusion if hybrid FTS/vector scores are not directly comparable. Apply `limit` after merging.

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
- Acceptance: every oversized segment contains the full signature/docs; body overlap is measured; IDs are unique and stable.

### Slice 3 — Code table and manual full index

- Add schema, migrations, Rust walker, hard excludes, nested `.gitignore`, and file-size limit.
- Implement `reindex --code` and `reindex --all` only; no automatic watching.
- Acceptance: dogfood on this repository, record scan time, parse time, embed time, peak RSS, file count, item count, and segment count.

### Slice 4 — Incremental replacement and embedding reuse

- Implement `(item_key, segment_index)` matching, content-hash reuse, and orphan cleanup.
- Acceptance: adding lines above an item changes line metadata but neither ID nor embedding; editing one function embeds only its changed segments; deletion removes all rows.

### Slice 5 — Search APIs

- Add `rms_code_search` and `rms_search(corpus=...)`.
- Add code result metadata and merged ranking.
- Acceptance: JSON-RPC tests cover all corpora, absent code table, empty index, `min_confidence`, lower-is-better distance handling, and limit-after-merge.

### Slice 6 — Multi-process verification

- Exercise three concurrent manual reindexes and simultaneous reads.
- Test lock contention, dirty retry, process crash, and source changes during embedding.
- Acceptance: one writer embeds each generation, readers remain available, no lost update, and no stale lock after crash.

### Slice 7 — Opt-in source watcher

- Enable `code_index_mode=watch` with event coalescing by file path and a three-second quiet window.
- Maintain a dirty generation rather than an unbounded event queue.
- Acceptance: five IDE processes plus rapid saves produce one logical reindex generation and return to idle CPU.

### Slice 8 — Release gate

- Dogfood on `rms-memory-mcp` and at least one larger Rust workspace.
- Run correctness, resource, migration, restart, and corruption-recovery tests.
- Update README, CLI help, MCP tool descriptions, roadmap, and changelog.

## Explicit non-goals for v1

- Go, TypeScript, and JavaScript parsers.
- A doc-to-code backlink graph.
- Method-level impl chunking.
- Editing source files through MCP.
- A permanent background daemon.
- Dynamic resource throttling beyond fixed thread/batch/file-size limits.
