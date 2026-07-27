# Cargo workspace layout

The root `rms-memory-mcp` package remains the **only crates.io package**. It owns
the `rms-memory` binary, MCP server, tools, rules injector, templates, CLI
commands, and installers, and re-exports modules so existing
`rms_memory_mcp::<module>` imports remain valid.

Local development uses path-only workspace members under `crates/`:

- `rms-memory-core`: documents, configuration, workspace discovery, path policy,
  links, and audit metadata.
- `rms-memory-index`: storage, indexing, retrieval, graphs, jobs, and Wiki.
- `rms-memory-vault`: document and project services, migration, and import.
- `rms-memory-cli`: reserved boundary (`publish = false`); CLI stays in the
  umbrella because `serve` calls the local MCP server.

All members have `publish = false`. They are **never** uploaded to crates.io.

## crates.io publish

`scripts/publish-workspace-crates.sh` flattens core/index/vault into a temporary
single-package tree (`scripts/flatten-for-crates-io.py`) and runs
`cargo publish` for **`rms-memory-mcp` only**. Tag push and the manual
`Publish crates.io` workflow both use that script.

Do not reintroduce publishing of `rms-memory-{core,index,vault}`.
