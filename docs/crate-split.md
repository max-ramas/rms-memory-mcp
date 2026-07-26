# Cargo workspace layout

The root `rms-memory-mcp` package remains the public umbrella crate and owns the
`rms-memory` binary, MCP server, tools, rules injector, templates, CLI commands,
and installers. It re-exports modules from these path-only workspace crates so
existing `rms_memory_mcp::<module>` imports remain valid:

- `rms-memory-core`: documents, configuration, workspace discovery, path policy,
  links, and audit metadata.
- `rms-memory-index`: storage, indexing, retrieval, graphs, jobs, and Wiki.
- `rms-memory-vault`: document and project services, migration, and import.
- `rms-memory-cli`: reserved boundary; CLI implementation stays in the umbrella
  during Phase 1 because `serve` calls the local MCP server.

Internal members use `publish = false` and are resolved through local path
dependencies, so source builds and binary releases still have one public product:
the root `rms-memory-mcp` umbrella package.

Cargo removes `path` from dependency specifications when publishing to crates.io.
Consequently, a future `cargo publish -p rms-memory-mcp` requires either publishing
the exact internal crate versions first or changing the packaging strategy so the
umbrella contains their implementation directly. Phase 1 intentionally leaves the
members path-only; do not claim the umbrella is crates.io-publishable until one of
those follow-ups is completed.
