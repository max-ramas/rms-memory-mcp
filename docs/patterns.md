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

## Serving rule (implemented)

`rms_search` takes `pattern_status`, a minimum level applied to hits from `patterns/*.md`:

| Value | Returns |
| --- | --- |
| *(default)* `verified-in-runtime` | only proven patterns |
| `implemented` | implemented and verified |
| `proposed` or `all` | everything; hits carry their `pattern_status` |

Notes outside `patterns/` are never filtered. A pattern with a missing or unknown `pattern_status`, or an unreadable
note, counts as `proposed`, so a half-written note is not served as a recommendation. When the vault has a `patterns/`
folder the search asks the index for 3x the hits (max 100) so filtering does not leave the result short.

## Staleness check (implemented)

`rms-memory doctor` / `rms_doctor` adds check 9 "Patterns" when the vault has `patterns/`. For each `refs` entry:

- the file at `path` must exist in the referenced repo (`repo` absent = this project's code path; `owner/name` is matched
  against the `origin` remote of registered projects, repos without a local checkout are skipped);
- `symbol` (last `::`/`.` segment, whole word) must still occur in the file;
- `commit` must exist and `git rev-list <commit>..HEAD -- <path>` must be empty, otherwise "changed in N commit(s) since";
- `implemented`/`verified-in-runtime` patterns need `refs`, and `verified-in-runtime` needs `verified.date` and `verified.how`.

It only reports; it never edits notes. The symbol check is textual (no code index), so it can miss a symbol that moved
to another file but still matches by name.

## Writing one

```text
rms_write(path="patterns/<topic>/<name>.md", mode="create", content=<template filled in>, project=<key>)
```

Promote by editing `pattern_status` (and `verified`) with `mode="replace"` after the evidence exists.
Never promote on the strength of "tests pass" alone; `verified-in-runtime` means it ran for real.

## Later (deliberately not built yet)

1. A separate vault for patterns shared across projects (`cross_project_vault`), once the format has settled in practice.
2. Symbol resolution through the code index instead of a text match.
