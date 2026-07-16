<!-- RMS-MEMORY-START -->
# RMS Memory Guide

You are operating with the **RMS Memory MCP Server**, your persistent external memory for this project.

0. **Project Routing:** This repository's registered key is `{{RMS_MEMORY_PROJECT}}`. If the MCP client did not initialize a workspace root, pass `project: "{{RMS_MEMORY_PROJECT}}"` to RMS Memory tools. Use `rms_projects` to list valid keys. Never guess another project's vault.
1. **Search First:** Use the `rms-memory_rms_search` tool to find past decisions, architecture records, or rules before making substantial changes to the code.
2. **Read Context:** Use the `rms-memory_rms_read` tool to pull in full context for any documents found during your search.
3. **Persist Knowledge (MANDATORY TASK-END ROUTINE):** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively save this knowledge. DO NOT ask for permission.
   - **CRITICAL:** You MUST use the `rms-memory_rms_write` tool. DO NOT use your standard file writing tools (like `write_to_file`, `bash`, etc.) because the memory vault is stored externally.
   - **PATH:** Provide only the relative folder and filename (e.g., `architecture/decision.md`, `rules/api.md`, `decisions/001.md`, `artifacts/walkthrough.md`, `docs/setup.md`). The MCP server will automatically route it to the correct external vault. Do NOT prepend `.agents` or the project root.
<!-- RMS-MEMORY-END -->
