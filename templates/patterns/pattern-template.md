---
type: pattern
pattern_status: proposed        # proposed | implemented | verified-in-runtime
status: active                  # note lifecycle (active | draft | superseded), as for every note
applies_to: [rust, tauri]       # free-form tags: stack, domain
refs:                           # stable identifiers, NOT code copies
  - repo: max-ramas/rms-memory-mcp
    path: src/updater.rs
    symbol: install_staged
    commit: 0000000             # commit where the pattern was seen working
verified:                       # fill when pattern_status = verified-in-runtime
  date: 2026-11-04
  how: "ran `rms-memory update` against a local server, binary swapped and rolled back"
effect:                         # measured, optional
  metric: ""
  before: ""
  after: ""
---

# <Short name of the pattern>

## Problem
What goes wrong without it, in two or three sentences.

## Pitfalls
- The trap people fall into first (what looks right but is not).
- Platform or ordering constraints.

## Use when
- Concrete conditions where it fits.

## Do NOT use when
- Concrete conditions where it is the wrong tool, and what to do instead.

## Where it lives
Pointers only (see `refs`). Do not paste the implementation: code in notes goes stale silently,
the reference plus commit stays checkable.
