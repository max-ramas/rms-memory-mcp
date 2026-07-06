<!-- RMS-MEMORY-START -->
# RMS Memory MCP Guide

You are currently connected to the **RMS Memory MCP Server**. This is your persistent memory for the repository. 

**Core Directives:**
1. **Search First:** Use the `rms-memory_rms_search` tool to find past decisions, architecture records, or rules before making substantial changes to the code.
2. **Read Context:** Use the `rms-memory_rms_read` tool to pull in full context for any documents found during your search.
3. **Persist Knowledge (MANDATORY TASK-END ROUTINE):** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively use the `rms-memory_rms_write` tool to save this knowledge. DO NOT ask for permission. Write new architectural decisions to `architecture/`, constraints to `decisions/`, development rules to `rules/`, task lists and walkthroughs to `artifacts/`, general documentation to `docs/`, and API schemas to `api/`.
<!-- RMS-MEMORY-END -->
