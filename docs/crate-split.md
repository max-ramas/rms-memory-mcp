# Cargo workspace layout

The root `rms-memory-mcp` package remains the public umbrella crate and owns the
`rms-memory` binary, MCP server, tools, rules injector, templates, CLI commands,
and installers. It re-exports modules from these workspace crates so existing
`rms_memory_mcp::<module>` imports remain valid:

- `rms-memory-core`: documents, configuration, workspace discovery, path policy,
  links, and audit metadata.
- `rms-memory-index`: storage, indexing, retrieval, graphs, jobs, and Wiki.
- `rms-memory-vault`: document and project services, migration, and import.
- `rms-memory-cli`: reserved boundary; CLI implementation stays in the umbrella
  during Phase 1 because `serve` calls the local MCP server (`publish = false`).

Workspace members `rms-memory-{core,index,vault}` are published to crates.io in
dependency order before the umbrella (`scripts/publish-workspace-crates.sh`).
Local development still uses path dependencies; Cargo strips `path` when publishing.

The release workflow publishes automatically on tag push (`v*`) after binary
assets and Homebrew dispatch succeed. Manual release dispatch defaults
`publish_crates=true`; uncheck it to skip crates.io when re-running assets only.

The standalone `Publish crates.io` workflow remains for retrying publication
without rebuilding the full platform matrix.
