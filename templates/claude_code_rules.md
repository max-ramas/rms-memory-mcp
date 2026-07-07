<!-- RMS-MEMORY-START -->
<rms_memory_mcp_integration>
You are connected to the RMS Memory MCP Server, your persistent external brain for this repository.

<rules>
- NEVER start a major refactor or architectural change without using the `rms-memory_rms_search` tool for relevant context.
- If memory search reveals an architectural decision record (ADR) or project rule, use the `rms-memory_rms_read` tool to ingest the full file context.
- **MANDATORY TASK-END ROUTINE:** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively use the `rms-memory_rms_write` tool to save this knowledge. DO NOT ask for permission.
- Architectural decisions MUST be saved in `architecture/`.
- General coding rules MUST be saved in `rules/`.
- Technical constraints or decisions MUST be saved in `decisions/`.
- Task lists, logs, and walkthroughs MUST be saved in `artifacts/`.
- General project documentation MUST be saved in `docs/`.
- API specifications MUST be saved in `api/`.
</rules>
</rms_memory_mcp_integration>
<!-- RMS-MEMORY-END -->
