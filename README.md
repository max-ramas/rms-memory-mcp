<div align="center">

# 🧠 RMS Memory MCP

**Persistent, local-first memory for your AI coding agents.**

Stop re-explaining your architecture to Cursor, Zed, and Claude Code every single session.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![Crates.io](https://img.shields.io/crates/v/rms-memory-mcp)](https://crates.io/crates/rms-memory-mcp)
[![Release](https://img.shields.io/github/v/release/max-ramas/rms-memory-mcp?color=blue)](https://github.com/max-ramas/rms-memory-mcp/releases)
![Build](https://github.com/max-ramas/rms-memory-mcp/actions/workflows/release.yml/badge.svg) 
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()
[![MCP](https://img.shields.io/badge/protocol-MCP-blueviolet)](https://modelcontextprotocol.io)

[Features](#-key-features) • [Download](https://github.com/max-ramas/rms-memory-mcp/releases/latest) • [Install](#-installation) • [Quick Start](#-quick-start) • [CLI](#-cli-commands) • [MCP Tools](#-mcp-tools-exposed) • [Architecture](#-architecture-highlights)

</div>

---

## The Problem

You're developing a single project but switching between different agents — Cursor, Zed, Claude Code, OpenCode. Every one of them loses context of architectural decisions, system requirements, and user preferences the moment you close the tab. You end up re-explaining the same things over and over, or copy-pasting a stale `CLAUDE.md` between tools.

**RMS Memory MCP** bridges this gap: a single, isolated, centralized Markdown vault — perfectly structured for LLM consumption — that any MCP-compatible IDE can read from and write to.

## ✨ Key Features

| | |
|---|---|
| 🗂️ **Global Centralized Vaults** | Project context lives outside your repo — zero `.mcp` file pollution. |
| 🔍 **Hybrid Retrieval (LanceDB)** | Embedded Vector Search + Tantivy Full-Text Search for zero-fail context hits. |
| 🌐 **Multilingual Semantic Parsing** | `fastembed-rs` + `multilingual-e5-small` — native Russian & English understanding. |
| 🌳 **AST Markdown Chunker** | `pulldown-cmark`-based chunking keeps code blocks and lists bound to their parent heading. |
| ⚙️ **Dynamic Auto-Installer** | `rms-memory install` scans your system and wires itself into every supported IDE. |
| 📜 **Rules-as-Code Patching** | Non-destructive AST patching of `.cursorrules`, `.zed/assistant.md`, etc. Opt-in by default. |
| 🧪 **Dry-Run & Auditing** | `--dry-run` everywhere. Every write gets a rolling `.bak` backup. |
| 🛡️ **Ten-Point Resiliency** | GC, background sync, write-guard snapshots, macOS sandbox bypass, `llms.txt` export, path traversal + injection protection, zombie prevention, graceful shutdown. |
| 🔒 **Security Hardened** | Panic-free database layer, symlink traversal blocked, JSON-RPC error responses, request size limits. 3-agent audit completed with 14 critical/high bugs resolved. |
| 🧠 **Audit Metadata** | Every record auto-receives `last_modified_by`, `timestamp`, `confidence`, `source` — agents can filter by reliability. |
| 🔀 **Multi-Scope** | `--scope` flag supports arbitrary identifiers beyond filesystem paths (thread IDs, lead IDs, etc.). |

## 📦 Installation

### Option 1: Homebrew (macOS Apple Silicon & Linux)

```bash
brew tap max-ramas/tap
brew install rms-memory-mcp
```

Installs a prebuilt binary — no Rust toolchain required. The formula updates
automatically with every release.

> **Not covered by Homebrew:** macOS Intel (dropped as of v1.0.1) and
> Windows (Homebrew doesn't run there — use Option 2 or the `.zip` below).

### Option 2: Cargo

If you have Rust installed:

```bash
cargo install rms-memory-mcp
```

### Option 3: Build from Source

```bash
# 1. Clone the repository
git clone https://github.com/max-ramas/rms-memory-mcp.git
cd rms-memory-mcp

# 2. Build the optimized release binary
cargo build --release

# 3. Add the binary to your global PATH
cp target/release/rms-memory ~/.cargo/bin/
```

> Prebuilt binaries for `aarch64-apple-darwin` (Apple Silicon), `x86_64-unknown-linux-gnu`,
> `aarch64-unknown-linux-gnu`, and `x86_64-pc-windows-msvc` are published on every
> [release](https://github.com/max-ramas/rms-memory-mcp/releases), along with
> `.deb`/`.rpm` packages for Linux. One-line installers (`install.sh` / `install.ps1`)
> auto-detect your architecture.

## 🚀 Quick Start

The fastest way to get every IDE on your machine connected:

```bash
rms-memory install
```

This scans `~/.config/` and `~/Library/Application Support/` and hooks `rms-memory` directly into **Cursor**, **Zed**, **Claude Code**, **OpenCode**, and others — no manual JSON editing.

For virtual projects without a filesystem path (threads, leads, etc.), use `--scope`:

```bash
rms-memory --scope "thread:abc-123" serve
```

[See multi-scope documentation →](docs/multi-scope-usage.md)

### Configure your vault

The simplest way to configure the server is to run the interactive setup wizard. You don't need to memorize any CLI flags — just run:

```bash
rms-memory config
```

*(Alternatively, you can pass flags directly: `rms-memory config --vault-path ~/MyVaults/ --auto-add true`)*

The next time you open a project in a connected IDE, the server reads the `rootUri` from the MCP `initialize` handshake and provisions a clean, structured vault:

```text
~/MyVaults/
  └── <ProjectHash>/
      ├── rules/
      ├── decisions/
      ├── architecture/
      ├── artifacts/
      ├── docs/
      └── api/
```

## 🛠 CLI Commands

| Command | Description |
|---|---|
| `rms-memory serve` | Starts the JSON-RPC stdio server (auto-triggered by your IDE). |
| `rms-memory init` | Registers a project into the global registry. `--dry-run` supported. `--full` forces creation of all IDE rule templates. |
| `rms-memory import` | Scans for existing docs (`README.md`, `docs/`, `ADR/`) and imports them — interactively or via `--auto-import`. |
| `rms-memory install` | Hooks the server into supported IDEs. `--dry-run` supported. |
| `rms-memory uninstall` | Removes the server from all discovered IDE configurations. |
| `rms-memory doctor` | Runs 5-point vault health diagnostics. `--repair-frontmatter` safely repairs duplicate IDs with backups; `--repair-path` targets one registered-vault file. |
| `rms-memory config` | Interactive setup wizard for global settings (`vault-path`, `auto-add`, `inject-rules`, etc). |
| `rms-memory reindex` | Forces a full re-index of the current project vault. |
| `rms-memory sync` | Incremental LanceDB delete-then-insert sync (also runs automatically during `serve`). |
| `rms-memory gc` | Prunes orphaned LanceDB indices belonging to deleted vaults. |
| `rms-memory log` | Tails the telemetry log (`~/.rms-memory/rms.log`). |
| `rms-memory export-llms` | Compiles the current vault into a single `llms.txt` payload. |
| **All commands** | Accept `--scope <id>` to target arbitrary vaults (threads, leads, etc). See `docs/multi-scope-usage.md`. |

## 🔌 MCP Tools Exposed

Tool descriptions are written to be **action-oriented**, so agents use the vault proactively without being asked.

<table>
<tr><th>Tool</th><th>Purpose</th><th>Input</th></tr>
<tr>
<td><code>rms-memory_rms_search</code></td>
<td>Semantic search across the vault. Agents are instructed to call this <em>first</em>, before making any changes. Supports <code>min_confidence</code> filtering.</td>
<td><code>{ query, limit, include_content, min_confidence }</code></td>
</tr>
<tr>
<td><code>rms-memory_rms_read</code></td>
<td>Reads the full contents of a document found via <code>rms_search</code>.</td>
<td><code>{ path }</code></td>
</tr>
<tr>
<td><code>rms-memory_rms_write</code></td>
<td>Persists new decisions, constraints, or rules. Agents are prompted to call this <em>proactively</em> after solving a tricky bug or learning a preference. Auto-injects audit metadata.</td>
<td><code>{ path, content, mode: replace|append|create, confidence, source }</code></td>
</tr>
</table>

## 🏗 Architecture Highlights

<details>
<summary><b>Unified Configuration & Knowledge Isolation</b></summary>

A central `~/.rms-memory/registry.toml` routes every project to an isolated vault, computed from a hash of the project path. No `.mcp` files, no per-repo config — global MCP entries (e.g. Zed's `settings.json`) can target any workspace automatically.
</details>

<details>
<summary><b>Linked Documents (zero-copy import)</b></summary>

Instead of duplicating existing docs into the vault, `rms-memory import` can create lightweight **Link Files** — Markdown stubs with a `link: <path>` frontmatter property. Reads/writes are transparently redirected to the source file, while the vector index still respects the vault's directory structure.
</details>

<details>
<summary><b>Hybrid Search (LanceDB + Tantivy)</b></summary>

Embedded LanceDB (`~/.rms-memory/dbs/`) combines vector similarity with full-text search, so a query never comes back empty just because the exact keywords didn't match.
</details>

<details>
<summary><b>AST-Aware Chunking</b></summary>

`pulldown-cmark` parses the Markdown AST directly. Chunks are built by walking up to the parent heading, with a strict 1500-character boundary and ~200-character overlapping window for oversized code blocks — no mid-sentence truncation.
</details>

<details>
<summary><b>Ten-Point Production Resiliency</b></summary>

1. Path traversal + filter injection prevention
2. Zombie process prevention (watcher shutdown on EOF + `std::process::exit(0)`)
3. Graceful shutdown (`SIGINT`/`Ctrl+C` handler)
4. macOS sandbox bypass for `fastembed` model downloads
5. `rms-memory gc` — orphaned vector store pruning
6. Background incremental sync (`Delete-then-Insert` on `mtime`)
7. Real-time file watcher with 3s debounced re-sync
8. Write-guard snapshotting with rolling `.bak` backups (default: 5)
9. Isolated telemetry logging (`~/.rms-memory/rms.log`)
10. `llms.txt` export for flat, decoupled LLM ingestion
</details>

## 🧩 Supported IDEs

| IDE | Auto-Install | Rules Injection |
|---|:---:|:---:|
| Cursor | ✅ | `.cursorrules` |
| Zed | ✅ | `.zed/assistant.md` |
| Claude Code | ✅ | `.claude/CLAUDE.md` |
| OpenCode | ✅ | — |
| Codex | ✅ | — |
| VS Code | ✅ | — |
| Antigravity | ✅ | — |

## 📄 License

MIT License — see [LICENSE](LICENSE) for details.

---

<div align="center">
<sub>Built by <a href="https://ramzaeff.com">Maksim Ramzaev</a> · <a href="https://rms-ds.com">RMS Digital Services</a></sub>
</div>
