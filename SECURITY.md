# Security Policy

## Supported versions

| Component | Version | Supported |
|-----------|---------|-----------|
| `rms-memory` CLI / MCP (`rms-memory-mcp`) | **1.3.1** (2026-10-15) | Yes |
| RMS Memory GUI (`rms-memory-gui`) | **1.3.1** (2026-10-15) | Yes |

Older pre-1.0 builds are unsupported.

## Reporting a vulnerability

Email **security@rms-ds.com** (or open a private advisory on the relevant GitHub repository if available).

Please include:

- affected component and version / commit
- reproduction steps or PoC (non-destructive preferred)
- impact (data disclosure, vault escape, RCE, privilege escalation, etc.)

We aim to acknowledge within **3 business days** and to provide a status update within **10 business days**.

Do **not** open a public issue for exploitable flaws until a fix or coordinated disclosure date is agreed.

## Scope highlights

In-scope examples:

- vault / path jail breaks (symlink escape, `../` traversal)
- MCP tool input that writes outside the registered vault
- authentication / license bypass that unlocks paid GUI features in release builds
- supply-chain issues in release artifacts we publish

Out of scope (unless chained into a higher impact):

- local malware with the same OS user privileges as the app
- denial of service from intentionally huge vaults without a resource budget
- issues only reproducible with `debug_assertions` entitlement bypass

## Hardening expectations

- Production GUI builds verify license payloads with the baked `RMS_LICENSE_PUBLIC_KEY`.
- MCP remains local-first; cloud sync (Spend) sends metadata only when the user opts in.
- See `docs/AI_PRIVACY.md` in the GUI repo for AI provider data boundaries.
- L3 continuity adapters embed the installer executable path as a single-quoted bash literal (no `$`/backtick expansion from `current_exe` or CLI install paths). Invalid Cursor `hooks.json` is warned before backup/rewrite; foreign unmanaged hook entries are preserved.
- Workspace discovery matches registered projects by whole path components on canonical paths, so a registered `/repo` cannot capture a sibling `/repo-old` or `/repo2` and route reads/writes into the wrong vault (fixed in 1.0.8, where cwd fallback made this reachable automatically).
- `rms-memory inject-rules` is fail-closed for unregistered directories: it never writes a `project` key that the registry cannot resolve.
- Release packaging clears the persistent build target's package directories before regenerating, so a published release cannot carry `.deb`/`.rpm` artifacts left over from an older version. Manual release dispatch builds the requested tag and validates the Cargo version before producing artifacts.
- Portable MCP archives and GUI installers are **always versioned** (`rms_memory_mcp_<version>_<target>.*`, `rms_memory_gui_<version>_*`). Unversioned names are not published; CI runs `scripts/check-release-asset-names.sh`.
- Cross-project vault federation (`projects` with `len > 1` + `corpus=vault|all`) is fail-closed: every listed project must opt in via `cross_project_vault` or the tool call errors (no silent degrade to code-only).
- **Accepted residual (1.0.9):** multi-project `corpus=code` (and the code half of `corpus=all`) does **not** consult `cross_project_vault`. Same-host, same-user IDE use is the intended audience; treat indexed code as less sensitive than vault Markdown, or keep those projects unregistered / code-index off. A future `cross_project_code` flag is out of scope for 1.0.9.
- Federated `projects` lists are capped (8 keys after dedupe) so fan-out cannot unbounded-open LanceDB stores outside the bind cache.
- Bind-cache eviction cancels watchers and aborts tasks that miss the join timeout, so detached notify/sync loops do not accumulate after Store drop.
- CI runs `cargo deny --locked check advisories bans sources` (`deny.toml`) so known RUSTSEC advisories and yanked crates fail the build (with a documented ignore list for unfixable transitive deps).
- **`rms_file_history` (1.1.2):** runs `git log` only against the registered project `code_path` and stores a derived Lance cache (not vault Markdown). MCP `reindex` requires explicit `project`. Catch-up/reindex enforce commit budgets; hex SHA allowlist on revision args. Agents should use the tool instead of arbitrary shell `git`; v1 does not follow renames across paths. Search `include_file_history` is refused with federated `projects`.
- **`rms_graph` (1.2.0):** durable vault/code graph is MCP-readable/mutable; edge mutations require explicit `project`. Search `include_graph_neighbors` is refused with federated `projects`. Visual GraphView remains GUI-only.
- **Update check (1.3.0):** only `doctor` (CLI, MCP `rms_doctor`) sends one unauthenticated request to the GitHub releases API, at most once per 24 hours, with no vault data; the result is cached in `~/.rms-memory/update-check.json`. Server start-up and MCP `initialize` never touch the network. `RMS_MEMORY_NO_UPDATE_CHECK=1` (or `CI`) disables the check. There is no self-update command: upgrades go through the installer that owns the binary.
- **Read-only vaults (1.3.0):** a vault attached with `projects attach-obsidian` is never written: `rms_write`, checkpoints, prune, hooks and frontmatter ID repair refuse it, and removing the project only drops the registration. `import --obsidian` only reads the source, skips hidden folders and never overwrites files in the destination vault.
- **Git context (1.3.0):** `rms_overview` and checkpoints run read-only `git` commands against the registered `code_path` and report branch, HEAD, ahead/behind and at most 50 changed file names. Diff contents are never read.
- **Embedding models (1.3.0):** bundled models are downloaded once by fastembed into `~/.rms-memory/cache`. An `ollama:<model>` profile sends note text for embedding only to a loopback Ollama URL; non-loopback URLs are refused.

## Local HTTP transport

`rms-memory serve --http` (1.3.0) is meant for local MCP clients that cannot spawn a stdio process. It is not a remote or multi-user server:

- It binds `127.0.0.1` only (default port 7337, endpoint `/mcp`).
- Every request needs `Authorization: Bearer <token>`. The token is random, created on first use in `~/.rms-memory/http-token` with mode 0600, and compared in constant time. Anyone who can read that file as your OS user can use the server.
- `Host` must name the loopback listener and a present `Origin` must be a loopback origin, so a web page cannot reach the server through DNS rebinding.
- Each `Mcp-Session-Id` has its own server state; at most 8 sessions are kept and idle ones expire after an hour. Request bodies are capped at 1 MiB.
- Tool behaviour is identical to stdio, including project routing and vault path checks. Do not expose the port through a tunnel or reverse proxy.

## Credential refusal limits

Since 1.3.0, `rms_write` (`content`, `source`), `rms_checkpoint_save` (`goal`, `pending`) and `rms_checkpoint_done` (`summary`) refuse text that matches a known credential format: AWS access key ids, GitHub / GitLab / Slack / npm tokens, OpenAI and Anthropic keys, Stripe live keys, Google API keys and PEM private keys. The error names the kind and line so the agent can remove it; the value is never echoed and nothing is masked silently. Doctor check 8 reports matching notes already in the vault and changes nothing.

This lowers the chance that an agent copies a key into memory. It does **not** make the vault secret-free:

- Only prefix-based formats are detected. Passwords, generic `secret = …` assignments, bot tokens without a known prefix and high-entropy strings are not.
- Text that does not pass through these MCP tools is not checked: edits in the GUI editor, direct edits on disk, `rms-memory import` and vault sync.
- Secrets already committed to a vault's git history stay there. Revoke the key and rewrite history with your git tooling if needed.
