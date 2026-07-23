# RMS Memory GUI

RMS Memory GUI is the optional commercial desktop application for
[RMS Memory MCP](https://github.com/max-ramas/rms-memory-mcp). It is a Tauri
application for macOS, Windows and Linux that provides a local control plane for
the same vaults and projects served by MCP.

## What remains independent

The public `rms-memory-mcp` repository and its CLI/MCP binary work without the
GUI. They do not depend on a GUI license, a cloud LLM, an API key or the GUI
process to initialize projects, index content, search, write memory, serve MCP
clients or synchronize a Vault Git repository.

AI provider configuration, prompts, credentials and proposal workflows are GUI
only. The GUI stores provider secrets in the operating-system credential store;
they are never added to MCP configuration or this repository.

## Obtain the desktop application

The GUI source repository is private. Release installers are intentionally
distributed as binary assets from this public repository's
[Releases](https://github.com/max-ramas/rms-memory-mcp/releases) page. Choose
the asset whose platform and architecture match your machine:

| Platform | Installer formats |
| --- | --- |
| macOS Apple Silicon | `.dmg` |
| Windows x64 | `.msi` or NSIS `.exe` |
| Linux x64 | `.AppImage`, `.deb`, or `.rpm` |

The public release uses the same `v<version>` tag as the GUI build. The GUI
pipeline runs both for a pushed `v*` tag and for a manual dispatch that supplies
the same version tag as `src-tauri/tauri.conf.json`.

Each GUI release includes `SHA256SUMS.txt`. Verify the downloaded installer
against that file before installation (for example, `shasum -a 256 <file>` on
macOS or `Get-FileHash <file> -Algorithm SHA256` in PowerShell).

## Publication and trust boundary

The private GUI Actions workflow builds installers, uploads a short-lived
internal artifact containing only allowed installer extensions, then uses a
repository-scoped deploy credential to create or update the matching public MCP
release. It explicitly rejects all other files.

The workflow never transfers GUI source code, build logs, updater metadata,
license credentials, provider credentials or a private GUI release archive to
this public repository. On the GUI repository, the required
`RMS_MEMORY_MCP_TOKEN` must be a fine-grained GitHub PAT that has only
**Contents: Read and write** access to `max-ramas/rms-memory-mcp`. It is stored
as a GitHub Actions secret and is never embedded in the application or checked
into either repository.

Desktop installer signing will eventually run in the private GUI release
workflow. Until an Apple Developer account and platform certificates exist,
macOS builds may be distributed **unsigned** (local `pnpm tauri build`). On
first launch Gatekeeper may warn that the app is damaged or from an unidentified
developer — clear quarantine or use **Open Anyway** as documented in the GUI
`README.md`. Before installing any published asset, verify checksums against
`SHA256SUMS.txt` when present. Do not install executables from an unverified
fork.

## Compatibility

The GUI can manage a registered MCP project and its Vault, but it does not
replace the server. For a new or upgraded GUI/MCP pair, start the MCP server and
run **Doctor & Setup → Index code** in the GUI if you want its structural code
projection in the hybrid graph. Existing Markdown memory remains editable and
portable without the GUI.
