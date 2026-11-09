# RMS Memory GUI

**Current version:** GUI `1.4.1` (works with any MCP `1.4.x`; MCP `1.4.2` is a Linux memory fix only).
This release adds Continuity panels for index freshness, git branch, memory hygiene and closed checkpoints, an embedding-model picker in Settings, HTML/CSS/C#/PHP/Kotlin/SQL code languages, respect for read-only Obsidian vaults and doctor credential findings.

> ### 🔒 Privacy promise
> RMS does **not** collect your data or usage statistics. RMS Memory GUI has no analytics of any kind,
> and your notes, code, vaults and settings stay on your computer. What reaches RMS is only the fact
> of an installation (a random ID created on your computer, the app version and the update channel)
> and whether a license exists: about once a day the app sends that to `license.rms-ds.com` (with a
> license, also a hash and signature of your key, never the key itself; no hardware data, names or
> file paths).
> Features you switch on yourself (your own AI provider key, GitHub sign-in) are
> off by default and send only what you choose, to the service you chose. Checking for updates asks
> GitHub\*'s public releases and downloads signed files from GitHub\* and `update.rms-ds.com`.
>
> Want full privacy for the AI features? Point them at a local Ollama (or any loopback endpoint) and
> your notes never leave your computer for an AI provider; only the license check above remains.
>
> If an online service is ever introduced, we will tell you clearly beforehand.

RMS Memory GUI is the optional commercial desktop application for
[RMS Memory MCP](https://github.com/max-ramas/rms-memory-mcp). It is a Tauri
application for macOS, Windows and Linux that provides a local control plane for
the same vaults and projects served by MCP.

## What it does

| Area | Capability |
| --- | --- |
| Dashboard | Project health, document/store status, grouped Doctor findings, one-click repair for missing IDs, Continuity teaser (active checkpoints → Continuity tab) |
| Continuity | Create/continue/complete checkpoints, recent notes, overview counts, index freshness and git branch / changed files; memory hygiene check (near duplicates, stale checkpoints, old sessions), archive of superseded notes, closed-checkpoint history (camelCase via GUI `serde_gui`; core remains snake_case) |
| Search | Bounded memory search (same inject/abstain envelope as `rms_search`), corpus selector, optional multi-project federation (max 8; busy cleared on project switch), optional `min_score`, retrieval-mode badge, pinned hit badge |
| Editor | Visual (Milkdown) and Raw (Monaco) Markdown editing, create/rename/delete files and folders, dirty-state protection |
| Hybrid graph | Full WebGL Markdown + code + semantic graph; create user edges; suppress/restore durable overrides; independent filters, search and deep zoom |
| Source control | Per-project Git status/diff/history plus a separate global Vault Git/GitHub sync (safe commit → pull → push); Project Git automation card in Settings |
| Doctor & Setup | Init, MCP client installation, indexing, sync, GC, doctor and Wiki generation from one screen; Settings choose the embedding model (bundled or local Ollama) and the code languages |
| AI & Wiki (optional) | Bring-your-own-key organizer across 12 providers; proposal-only workflow — every change is reviewed with sources and a before/after diff before it touches the vault |
| Spend | Local token/cost ledger across your AI coding tools. Full Spend Settings live in the separate `rms-spend` app |
| About / license | Pro vs trial panel; optional in-app **Install** when signed `latest.json` is on the matching MCP release |
| Other | Backup history with restore, `llms.txt`/Graphviz export, live log viewer, Appearance status-bar toggle, light/dark themes, 7 UI languages |

AI and Vault GitHub sync are the only features gated behind
the GUI's own license after the trial period — everything above still starts
from the same Markdown vault and index the MCP server produces, so nothing in
this table implies a change to the MCP server itself.

## What remains independent

The public `rms-memory-mcp` repository and its CLI/MCP binary work without the
GUI. They do not depend on a GUI license, a cloud LLM, an API key or the GUI
process to initialize projects, index content, search, write memory, serve MCP
clients or synchronize a Vault Git repository.

AI provider configuration, prompts, credentials and proposal workflows are GUI
only. The GUI stores provider secrets in the operating-system credential store;
they are never added to MCP configuration or this repository.

## Obtain the desktop application

From a terminal: `rms-memory gui` installs the GUI for your platform from the RMS update server
(the signed manifest and the checksum are verified before anything is opened) and opens it; if it is
already installed it just opens it. `--download-only` saves the installer to your Downloads folder.
The GUI starts with a 14-day trial; the MCP server never needs it. Published builds: macOS on Apple
Silicon, Windows x64, Linux x64.

The GUI source repository is private. The newest installer for your platform is one click away
through the RMS update server (permanent "latest" links; the server redirects to the public release
asset and counts the download):

| Platform | Installer |
| --- | --- |
| macOS · Apple Silicon | [⬇ Download .dmg](https://update.rms-ds.com/get/rms-memory-gui/dmg?platform=darwin&arch=arm64&src=readme) |
| Windows · x64 | [⬇ Download .exe](https://update.rms-ds.com/get/rms-memory-gui/exe?platform=windows&arch=amd64&src=readme) · [⬇ Download .msi](https://update.rms-ds.com/get/rms-memory-gui/msi?platform=windows&arch=amd64&src=readme) |
| Linux · x64 | [⬇ Download .AppImage](https://update.rms-ds.com/get/rms-memory-gui/appimage?platform=linux&arch=amd64&src=readme) · [⬇ Download .deb](https://update.rms-ds.com/get/rms-memory-gui/deb?platform=linux&arch=amd64&src=readme) · [⬇ Download .rpm](https://update.rms-ds.com/get/rms-memory-gui/rpm?platform=linux&arch=amd64&src=readme) |

The server lists a release only after its signed manifest (`release.rms-memory-gui.json`) is attached
to the release; older versions and every file stay on this public repository's
[Releases](https://github.com/max-ramas/rms-memory-mcp/releases) page. Choose
the asset whose platform and architecture match your machine:

| Platform | Installer formats |
| --- | --- |
| macOS Apple Silicon | `.dmg` |
| Windows x64 | `.msi` or NSIS `.exe` |
| Linux x64 | `.AppImage`, `.deb`, or `.rpm` |

The public release uses the same `v<version>` tag as the GUI build (unified with MCP:
**`v1.3.1`** for this release). Installer assets are named `rms_memory_gui_<version>_*` (publish fails
if the version is missing from the basename) and checksums live in
`rms_memory_gui_SHA256SUMS.txt`. The GUI pipeline runs both for a pushed `v*` tag
and for a manual dispatch that supplies the same version tag as
`src-tauri/tauri.conf.json`. Cross-repo MCP checkout is pinned to that same tag.
When `TAURI_SIGNING_PRIVATE_KEY` is set on the GUI repo, CI publishes the
signed updater manifest `latest.json`, the installers' `.sig` files and the
macOS `rms_memory_gui.app.tar.gz` onto that public release so in-app Install can
use `…/releases/latest/download/latest.json`. Every signature is verified
against the updater key built into the app before publishing.

The Linux pipeline checks the finished AppImage payload and makes
`AppRun.wrapped` executable for users other than the build owner, then signs
the repaired AppImage again for the updater. This fixes
the launcher permission failure reported by the AppImage catalog for 1.2.0.
The catalog's test of the newly published installer remains an independent
distribution check.

### macOS signing and notarization

macOS installers are signed with an Apple Developer ID certificate and notarized by Apple, so they open
without a Gatekeeper warning. Verify the installer checksum against `rms_memory_gui_SHA256SUMS.txt` when
one is published with the release.

## Publication and trust boundary

The private GUI Actions workflow builds installers, uploads a short-lived
internal artifact containing only allowed installer extensions, then uses a
repository-scoped deploy credential to create or update the matching public MCP
release. It explicitly rejects all other files.

The workflow never transfers GUI source code, build logs, license credentials,
provider credentials or a private GUI release archive to this public repository.
Installer binaries and, when updater signing is enabled, `latest.json` /
arch-qualified `*.app.tar.gz` / companion `.sig` are the intentional public
artifacts. On the GUI repository, the required
`RMS_MEMORY_MCP_TOKEN` must be a fine-grained GitHub PAT that has only
**Contents: Read and write** access to `max-ramas/rms-memory-mcp`. It is stored
as a GitHub Actions secret and is never embedded in the application or checked
into either repository.

Desktop installers are signed in the private GUI release workflow: macOS with Developer ID and
notarization. Windows installers are not signed yet. Before installing any published asset, verify checksums against
`SHA256SUMS.txt` when present. Do not install executables from an unverified
fork.

## Compatibility

The GUI can manage a registered MCP project and its Vault, but it does not
replace the server. For a new or upgraded GUI/MCP pair, start the MCP server and
run **Doctor & Setup → Index code** in the GUI if you want its structural code
projection in the hybrid graph. Existing Markdown memory remains editable and
portable without the GUI.

---

\* GitHub is contacted directly by the app for your convenience (update checks and downloads). RMS does not take part in that exchange.
