# Security Policy

## Supported versions

| Component | Version | Supported |
|-----------|---------|-----------|
| `rms-memory` CLI / MCP (`rms-memory-mcp`) | 1.0.x | Yes |
| RMS Memory GUI (`rms-memory-gui`) | 1.0.x | Yes |

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
