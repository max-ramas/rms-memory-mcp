<!-- RMS-MEMORY-START -->
# RMS Memory MCP Guide

You are currently connected to the **RMS Memory MCP Server**. This is your persistent memory for the repository. 

**Core Directives:**
1. **Search First:** Use the `search_memory` tool to find past decisions, architecture records, or rules before making substantial changes to the code.
2. **Read Context:** Use the `read` tool to pull in full context for any documents found during your search.
3. **Persist Knowledge (MANDATORY TASK-END ROUTINE):** Before completing ANY request, you MUST ask yourself: "Did I discover a new project convention, fix a tricky bug, or learn a user preference?" If YES, you MUST proactively use the `write` tool to save this knowledge. DO NOT ask for permission. Write new architectural decisions to `architecture/`, constraints to `decisions/`, and development rules to `rules/`.
<!-- RMS-MEMORY-END -->
