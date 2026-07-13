# GUI-Ready Core Architecture

## Decision

The future GUI is a first-class client of RMS Memory, alongside CLI and MCP. The core must expose transport-neutral commands, queries, jobs, events, and configuration operations. MCP remains the AI-agent adapter. A future local HTTP or Unix-socket server may adapt the same application API for the GUI, but network transport is not part of the indexing core and will not be introduced before the core boundary exists.

```text
                         ┌──────────────┐
                         │ RMS Core API │
                         └──────┬───────┘
                  ┌─────────────┼─────────────┐
                  │             │             │
              MCP adapter    CLI adapter   GUI adapter
                                            (future)
                  │             │             │
                  └─────────────┼─────────────┘
                                │
             Vault + code index + graph + config repository
```

The GUI must not call MCP tools internally. CLI, MCP, and GUI must also stop duplicating persistence logic; they invoke the same services.

## Graph contract

Search chunks are not graph nodes. Chunk boundaries may change, so edges never reference `code_chunks.id` or a Markdown chunk index.

Canonical node keys:

- `vault:<document_id>` for a Markdown document;
- `code:<item_key>` for a semantic code item;
- `external:<normalized-identifier>` for an unresolved import, symbol, URL, or other external target.

Materialize graph data in separate `graph_nodes`, `graph_edges`, and `graph_edge_overrides` tables. Existing `links_raw` and `links_resolved` remain a compatibility/search projection, not the authoritative editable graph.

### `graph_nodes`

| Field | Purpose |
|---|---|
| `node_key` | Stable typed identity; primary key |
| `corpus` | `vault`, `code`, or `external` |
| `source_id` | Document ID, item key, or normalized external identifier |
| `kind` | Document/code/external node kind |
| `label` | Display label for graph clients |
| `path` | Optional navigable path |
| `metadata_json` | Versioned extensible metadata |
| `generation` | Derived reconciliation generation |
| `updated_at` | Audit timestamp |

### `graph_edges`

| Field | Purpose |
|---|---|
| `edge_key` | Stable semantic identity |
| `source_key` / `target_key` | Canonical node keys |
| `relation` | `links_to`, `uses`, `implements`, `calls_symbol`, `related_to`, etc. |
| `origin` | `derived` or `user` |
| `extractor` | Versioned producer such as `rust-tree-sitter-v1` |
| `resolution` | `resolved`, `unresolved`, or `ambiguous` |
| `confidence` | Nullable extraction/resolution confidence |
| `generation` | Reconciliation generation for derived rows |
| `metadata_json` | Versioned extensible payload |
| `created_at` / `updated_at` | Audit timestamps |

A `manual BOOLEAN` is insufficient. Reindexing owns only rows where `origin=derived` and only for its own versioned extractor. User-created rows are never overwritten or deleted by reindex.

### Overrides and deletion

If a user hides or rejects a derived edge, reindex must not resurrect it. `graph_edge_overrides` stores an override keyed by the derived `edge_key`, with action `suppress` or `restore`, revision, author, and timestamps. Effective graph queries apply overrides after derived-edge reconciliation.

Manual edge edits use optimistic concurrency (`revision`/ETag). This prevents two GUI windows or a GUI and CLI operation from silently overwriting each other. The initial persistence layer now enforces this as a compare-and-swap revision: a stale edit returns `GRAPH_OVERRIDE_CONFLICT`.

## Initial relationship extraction

The first extractor should emit only relationships whose limitations are explicit:

- Markdown links: `links_to`, resolved by document ID/path when possible.
- Rust `use` declarations: `uses`, initially allowed to target unresolved external nodes.
- Trait implementations: `implements`, resolved inside the parsed item where possible.
- Function/method call syntax: `calls_symbol`, marked unresolved or ambiguous until a symbol-resolution layer exists.

Tree-sitter provides syntax, not compiler-accurate name resolution. The GUI must distinguish resolved edges from lexical hints. A full call graph is not a v1 promise.

## Application API

Before adding HTTP, extract services with transport-neutral request/response types:

- `QueryService`: documents, code items, graph neighborhoods, search, configuration snapshots;
- `CommandService`: edit Markdown, create/update/suppress graph edges, update configuration, request reindex;
- `JobService`: start/cancel long operations and query progress by `job_id`;
- `EventBus`: typed events such as `IndexProgress`, `GraphChanged`, `DocumentChanged`, and `ConfigChanged`.

Commands that edit Markdown or configuration accept an expected revision and return the new revision. Index progress is structured data, not terminal text. CLI renders it as lines; a future GUI adapter can expose it through SSE or another local event stream.

## Configuration repository

`Registry::load()` / `save()` calls scattered across commands are not a safe GUI contract. Replace them with one `ConfigManager` interface used by every adapter.

Required behavior:

1. Parse and validate a versioned configuration snapshot.
2. Serialize writes under a cross-process config lock.
3. Use compare-and-swap with an expected revision.
4. Write a temporary file in the same directory, flush it, atomically rename it, then sync the parent directory.
5. Watch the file and publish `ConfigChanged` to each running process.
6. Keep the last valid snapshot if an external edit is malformed and report the validation error.

A cache in each MCP process is acceptable only with file locking, revision checks, and change notifications. Caching alone is not a single-writer guarantee.

The first implementation now provides revisioned snapshots, a `.registry.lock`, compare-and-swap replacement, same-directory temporary-file persistence with flush/rename, a `tokio::watch` subscription stream, and a lifecycle-managed `notify` watcher. It preserves the last valid cached snapshot when an external file edit is malformed. Structured validation and a shared long-lived manager in the MCP runtime are the remaining integration work.

The initial jobs/events implementation is also transport-neutral: `JobManager` publishes typed snapshots through a bounded broadcast stream, supports structured phase/count/message progress, and uses cooperative cancellation. Its state machine permits only `queued`/`running` jobs to transition to a terminal state, so a cancelled or completed job cannot be silently revived. Adapters and indexing commands will be connected to this interface before any HTTP/SSE transport is introduced.

The persistence layer now opens or creates all three graph tables on demand. Reconciliation upserts a complete derived generation before pruning stale edges for its extractor; it never owns user nodes, user edges, or overrides. Canonical nodes have no single extractor owner, so they are retained until a future reference-aware graph GC proves them orphaned. This reduces the crash window and preserves manual GUI work. A future graph query service will apply suppression overrides when constructing the effective graph.

## Future GUI transport

The transport decision is deferred until the application API exists. Preferred deployment options are:

- embedded core inside a desktop shell for a single-process GUI; or
- a singleton local service reached through a Unix-domain socket, with an HTTP/SSE adapter if web technology requires it.

If TCP loopback is used, it must bind only to localhost, authenticate with a per-install token, enforce origin policy, and never expose arbitrary filesystem paths. The Markdown editor operates through validated Vault commands with backups and revision checks.

## Delivery impact

Insert a GUI-foundation gate after Slice 3 and before incremental indexing:

1. Freeze canonical node/edge identities and graph provenance rules.
2. Add graph schemas and migration tests.
3. Introduce `ConfigManager` and route existing CLI/MCP configuration access through it.
4. Introduce job/progress/event interfaces without an HTTP dependency.
5. Extend incremental reconciliation to derived graph nodes/edges while preserving user data and overrides.

No automatic code watcher or GUI server is enabled by this foundation work.
