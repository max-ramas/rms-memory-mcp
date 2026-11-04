# Keeping memory cheap for agents

What costs tokens, and what to use:

| Cost | Where | What helps |
| --- | --- | --- |
| Tool descriptions (`tools/list`, ~4.7k tokens, paid once per session) | every connection | Clients that load tools lazily; `rms-memory stats` shows the exact size |
| Search results | each `rms_search` | `snippet_chars` (first pass), `limit`, `max_chars`; heading-only chunks are dropped automatically |
| Reading a note | `rms_read` | `outline`, then `heading`; `max_chars`/`offset` for windows; `if_hash` to skip unchanged notes |
| Notes that are long | the vault | a short `summary:` in the frontmatter is shown by `outline` |

Why there is no server-side response cache: the agent pays for tokens that enter its context, whatever the server
cached. Savings come from sending less, or a short stub when nothing changed (`if_hash`). Caching also has to follow
vault edits, so only an exact, content-hash-based check is used.

Measure before tuning: `rms-memory stats --days 7`. Numbers are characters; tokens are estimated as characters / 4.
