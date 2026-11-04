# Patterns (`patterns/` notes)

A *pattern* is a reusable solution recorded as a vault note: the problem, the traps, when to use it
and when **not** to, and a **pointer** to where it lives in code. It deliberately holds no copy of the code.

This is a convention on top of ordinary notes. `rms_write` stores them and `rms_search` finds them;
`patterns/**/*.md` is covered by the default `**/*.md` include. No server code is required.

## Frontmatter

| Key | Values | Meaning |
| --- | --- | --- |
| `type` | `pattern` | marks the note for filtering |
| `pattern_status` | `proposed` → `implemented` → `verified-in-runtime` | how far the pattern has been proven. Separate from the note-lifecycle `status` (`active`/`draft`/`superseded`) |
| `applies_to` | list of tags | stack or domain |
| `refs` | list of `{repo, path, symbol, commit}` | stable identifiers. `symbol` + `commit` let a later check detect that the referenced code moved or vanished |
| `verified` | `{date, how}` | required evidence when `pattern_status: verified-in-runtime` |
| `effect` | `{metric, before, after}` | measured benefit, optional but preferred |

Body sections: Problem, Pitfalls, Use when, **Do NOT use when**, Where it lives.
The template is `templates/patterns/pattern-template.md`.

## Serving rule

Agents should treat only `verified-in-runtime` patterns as recommendations. `implemented` is "it exists,
not yet proven" and `proposed` is an idea; mention them only if asked or when no verified pattern fits,
and say which status they have. (Server-side filtering is a later step; today it is an instruction to the agent.)

## Writing one

```text
rms_write(path="patterns/<topic>/<name>.md", mode="create", content=<template filled in>, project=<key>)
```

Promote by editing `pattern_status` (and `verified`) with `mode="replace"` after the evidence exists.
Never promote on the strength of "tests pass" alone; `verified-in-runtime` means it ran for real.

## Later (deliberately not built yet)

1. Staleness check: `rms-memory doctor` resolves each `refs` entry and warns when the path or symbol is gone or the file changed since `commit`.
2. Default retrieval filter by `pattern_status` in `rms_search`.
3. A separate vault for patterns shared across projects (`cross_project_vault`), once the format has settled in practice.
