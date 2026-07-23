# Dual-repo audit — RMS Memory MCP + GUI — 2026-07-22

Mode of original pass: **audit-only**. Same-day release push closed the actionable backlog below.

## Empirical (after remediation)

| Check | Result |
|-------|--------|
| MCP `cargo test --lib` | **122 passed** |
| MCP `cargo clippy -D warnings` | **PASS** |
| GUI `cargo test --lib` | **63 passed** |
| GUI `pnpm typecheck` / `pnpm build` | **PASS** |
| GUI initial JS budget | **122.8 / 128 KiB gzip** (raised after Spend i18n; Monaco/Milkdown split out of EditorView) |

## Closed in the release push

1. Clippy `collapsible_if` in `project_migrate.rs`
2. Path-scoped code watcher reindex (`try_index_code_paths` + graph patch without full generation prune)
3. Graph force-layout Web Worker (`src/workers/graphLayout.worker.ts`)
4. Editor Monaco vs Milkdown `React.lazy` split inside `EditorView`
5. `SECURITY.md` / `NOTICE` / GUI `EULA.md`
6. `scripts/bench_large_vault.sh` perf smoke
7. GUI `require_entitlement` on AI analyze/apply/wiki, sync, spend sync (debug builds open)

## Still deferred (not 1.0.x blockers)

- ROADMAP **v1.1** multi-crate split
- Live rms-license product registration + bake `RMS_LICENSE_PUBLIC_KEY` in release CI
- Apple notarization / Windows Authenticode playbooks for public installers
