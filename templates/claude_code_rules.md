<!-- RMS-MEMORY-START -->
<rms_memory_mcp_integration>
You are connected to the RMS Memory MCP Server, your persistent external brain for this repository.

<rules>
- This repository's registered key is `{{RMS_MEMORY_PROJECT}}`. If the MCP client did not initialize a workspace root, pass `project: "{{RMS_MEMORY_PROJECT}}"` to RMS Memory tools. Use `rms_projects` to list valid keys. Never guess another project's vault.
- NEVER start a major refactor or architectural change without using the `rms-memory_rms_search` tool for relevant context.
- If memory search reveals an architectural decision record (ADR) or project rule, use the `rms-memory_rms_read` tool to ingest the full context.
- **Persist Knowledge (MANDATORY TASK-END ROUTINE):** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively save this knowledge. DO NOT ask for permission.
   - **CRITICAL:** You MUST use the `rms-memory_rms_write` tool. DO NOT use your standard file writing tools (like `write_to_file`, `bash`, etc.) because the memory vault is stored externally.
   - **PATH:** Provide only the relative folder and filename (e.g., `architecture/decision.md`, `rules/api.md`, `decisions/001.md`, `artifacts/walkthrough.md`, `docs/setup.md`). The MCP server will automatically route it to the correct external vault. Do NOT prepend `.agents` or the project root.
- Architectural decisions MUST be saved in `architecture/`.
- General coding rules MUST be saved in `rules/`.
- Technical constraints or decisions MUST be saved in `decisions/`.
- Task lists, logs, and walkthroughs MUST be saved in `artifacts/`.
- General project documentation MUST be saved in `docs/`.
- API specifications MUST be saved in `api/`.
</rules>
</rms_memory_mcp_integration>
<!-- RMS-MEMORY-END -->
