# Multi-Scope Usage

RMS Memory supports two levels of vault isolation through the `--scope` flag.

## How It Works

Every vault is identified by a scope string. The LanceDB database path is derived from
`blake3(identifier)`, so different scopes produce completely isolated databases:

```
~/.rms-memory/dbs/<blake3_hash>/
```

## Path-Based Scope (Default)

No `--scope` flag → current working directory is used as the identifier:

```bash
rms-memory serve                    # scope = canonicalize(cwd)
rms-memory --scope "/home/user/my-project" serve  # explicit absolute path
```

Path-based scopes produce the same vault as the original behavior — existing projects
continue to work unchanged.

## Arbitrary Scope

Pass any non-path string as a scope identifier for virtual projects:

```bash
rms-memory --scope "thread:abc-123" serve
rms-memory --scope "lead:acme-corp" serve
rms-memory --scope "project-analysis" serve
```

This creates a new vault at `~/.rms-memory/vaults/<blake3_hash>/` with the standard
directory structure (`rules/`, `decisions/`, `architecture/`, `artifacts/`, `docs/`, `api/`).

## Two-Level Architecture (Project / Thread)

For bots and agents that need both product knowledge (canon) and session history
(episodes), use two separate scope identifiers:

```
Project-level vault (canon):
  scope = "product-myapp"
  Contains: rules, artifacts, FAQ — facts about the product

Thread-level vault (episodes):
  scope = "thread:conversation-456"
  Contains: episodes of a specific conversation
```

### Usage Pattern

The caller (MCP client, bot, or agent) makes **two separate search calls**:

```json
// 1. Read product canon
{
  "name": "rms_search",
  "arguments": {
    "query": "what are the API rate limits",
    "scope": "product-myapp"
  }
}

// 2. Read thread history
{
  "name": "rms_search",
  "arguments": {
    "query": "what did we discuss about rate limits",
    "scope": "thread:conversation-456"
  }
}
```

The caller merges results on its side. RMS Memory does not have a built-in multi-scope
merge — this is intentional to keep the server stateless and the API minimal.

### Writing

- **Project-level writes** should be explicit, rare actions (updating product FAQ, rules)
- **Thread-level writes** are the standard write path (episodes, conversation notes)

Each write automatically receives audit metadata: `last_modified_by` (from the MCP client name),
`timestamp`, and optionally `confidence` (0.0–1.0) and `source` (citation text).

## Scope Validation Rules

| Scope value | Behavior |
|-------------|----------|
| (empty) | Error: scope must be non-empty |
| > 512 chars | Error: scope too long |
| Starts with `/` | Treated as absolute path → canonicalized |
| Starts with `./` or `../` | Treated as relative path → resolved against cwd |
| Anything else | Treated as opaque identifier → used as-is |

## Confidence Filtering — Agent Guidance

The `min_confidence` parameter (0.0–1.0) allows filtering search results by reliability.
However, **aggressive filtering can silently discard useful knowledge**:

- **Start without `min_confidence`** — see all available results first.
- **Use low thresholds (0.3–0.5) for broad discovery** — filters out only the
  most speculative content.
- **Use high thresholds (0.7+) sparingly** — only when you need verified, canonical
  facts. If the search returns zero results with a high threshold, **retry with a
  lower threshold or omit `min_confidence` entirely.**
- **NULL confidence records are always included** — documents created before v1.0.3
  have no confidence value and will appear in all searches regardless of threshold.
  This is intentional: absence of confidence ≠ low confidence.

### Recommended pattern for agents

```json
// First search: broad, no filter
{
  "name": "rms_search",
  "arguments": { "query": "API authentication flow" }
}

// If too many results, add a moderate filter
{
  "name": "rms_search",
  "arguments": { "query": "API authentication flow", "min_confidence": 0.5 }
}

// Only for verified canon — and ONLY after broad search succeeds
{
  "name": "rms_search",
  "arguments": { "query": "API authentication flow", "min_confidence": 0.85 }
}
```

The agent should **never** start a search with `min_confidence` above 0.7 —
this would preemptively hide potentially useful information that simply hasn't
been formally verified yet.
