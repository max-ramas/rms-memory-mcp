<!-- RMS-MEMORY-START -->
## RMS Memory MCP Integration
You are connected to the **RMS Memory MCP Server**, your external brain for this project.

1. **Check Memory Before Refactoring:** Never start a major refactor or architectural change without using `search_memory` for relevant keywords.
2. **Read Before Writing:** If memory search reveals an ADR or rule, use `read` to ingest the full file context.
3. **Persist Knowledge:** After solving a complex bug, making an architectural decision, or defining a new rule, ALWAYS use the `write` tool to save it into the Vault.
   - Architectural decisions go to `architecture/`.
   - General rules go to `rules/`.
   - Technical constraints or decisions go to `decisions/`.
<!-- RMS-MEMORY-END -->
