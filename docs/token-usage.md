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

## Measured on a real repository (107 files, 1870 code segments)

Ten code queries against the MCP repo itself (debug build, local embeddings):

| Call | Hits returned | Chars per call | Notes |
| --- | --- | --- | --- |
| `rms_code_search` default | 4 | ~3.3k | the 2000-char content budget cuts the list |
| `include_content: false` | 10 | ~2.7k | path, symbol, kind and lines only: a map, then `rms_read`/file read for the ones that matter |
| `snippet_chars: 200` | 10 | ~4.8k | more hits for a similar cost, each with a short body |

So a "light" code search mode already exists: `include_content: false` is a third of the cost per hit.
No new mode was added; the protocol text now points agents at it.

Structured tools (`rms_graph`, `rms_doctor`, `rms_overview`, checkpoints) used to send pretty-printed
JSON text; compact JSON carries the same information in 25-35% fewer characters (graph neighbors,
16 edges: 14.8k -> 11.3k). Responses are now compact; `structuredContent` is unchanged.

`rms_graph` reads all nodes and edges from the store on every call (about 90 ms for 9.3k nodes and
14.3k edges; code search itself takes about 30 ms). That is small next to a model round-trip, so
there is no graph cache; revisit it if graph calls get slow on very large repositories.
