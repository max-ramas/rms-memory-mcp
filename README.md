<div align="center">

# 🧠 RMS Memory MCP

**Persistent, local-first memory for your AI coding agents.**

Stop re-explaining your architecture to Cursor, Zed, and Claude Code every single session.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/Rust-1.75%2B-orange?logo=rust)](https://www.rust-lang.org/)
[![Release](https://img.shields.io/github/v/release/max-ramas/rms-memory-mcp?color=blue)](https://github.com/max-ramas/rms-memory-mcp/releases)
[![Build](https://img.shields.io/github/actions/workflow/status/max-ramas/rms-memory-mcp/release.yml?branch=main)](https://github.com/max-ramas/rms-memory-mcp/actions)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Linux%20%7C%20Windows-lightgrey)]()
[![MCP](https://img.shields.io/badge/protocol-MCP-blueviolet)](https://modelcontextprotocol.io)

[Features](#-key-features) • [Install](#-installation) • [Quick Start](#-quick-start) • [CLI](#-cli-commands) • [MCP Tools](#-mcp-tools-exposed) • [Architecture](#-architecture-highlights)

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
| 🛡️ **Six-Point Resiliency** | GC, background file-watcher sync, write-guard snapshots, macOS sandbox bypass, `llms.txt` export, isolated logging. |

## 📦 Installation

```bash
# 1. Clone the repository
git clone https://github.com/max-ramas/rms-memory-mcp.git
cd rms-memory-mcp

# 2. Install dependencies (LanceDB needs protoc)
# macOS:   brew install protobuf
# Ubuntu:  sudo apt-get install protobuf-compiler
# Windows: choco install protoc

# 3. Build the optimized release binary
cargo build --release

# 4. Add the binary to your global PATH
cp target/release/rms-memory-mcp ~/.cargo/bin/
```

> Prebuilt binaries for `x86_64-unknown-linux-gnu`, `x86_64-pc-windows-msvc`, `x86_64-apple-darwin`, and `aarch64-apple-darwin` are published on every [release](https://github.com/max-ramas/rms-memory-mcp/releases). One-line installers (`install.sh` / `install.ps1`) auto-detect your architecture.

## 🚀 Quick Start

The fastest way to get every IDE on your machine connected:

```bash
rms-memory install
```

This scans `~/.config/` and `~/Library/Application Support/` and hooks `rms-memory` directly into **Cursor**, **Zed**, **Claude Code**, **OpenCode**, and others — no manual JSON editing.

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
| `rms-memory config` | Interactive setup wizard for global settings (`vault-path`, `auto-add`, `inject-rules`, etc). |
| `rms-memory reindex` | Forces a full re-index of the current project vault. |
| `rms-memory sync` | Incremental LanceDB delete-then-insert sync (also runs automatically during `serve`). |
| `rms-memory gc` | Prunes orphaned LanceDB indices belonging to deleted vaults. |
| `rms-memory log` | Tails the telemetry log (`~/.rms-memory/rms.log`). |
| `rms-memory export-llms` | Compiles the current vault into a single `llms.txt` payload. |

## 🔌 MCP Tools Exposed

Tool descriptions are written to be **action-oriented**, so agents use the vault proactively without being asked.

<table>
<tr><th>Tool</th><th>Purpose</th><th>Input</th></tr>
<tr>
<td><code>rms-memory_rms_search</code></td>
<td>Semantic search across the vault. Agents are instructed to call this <em>first</em>, before making any changes.</td>
<td><code>{ query, limit, include_content }</code></td>
</tr>
<tr>
<td><code>rms-memory_rms_read</code></td>
<td>Reads the full contents of a document found via <code>rms_search</code>.</td>
<td><code>{ path }</code></td>
</tr>
<tr>
<td><code>rms-memory_rms_write</code></td>
<td>Persists new decisions, constraints, or rules. Agents are prompted to call this <em>proactively</em> after solving a tricky bug or learning a preference.</td>
<td><code>{ path, content, mode: replace|append|create }</code></td>
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
<summary><b>Six-Point Production Resiliency</b></summary>

1. macOS sandbox bypass for `fastembed` model downloads
2. `rms-memory gc` — orphaned vector store pruning
3. Background incremental sync (`Delete-then-Insert` on `mtime`)
4. Real-time file watcher with 3s debounced re-sync
5. Write-guard snapshotting with rolling `.bak` backups (default: 5)
6. `llms.txt` export for flat, decoupled LLM ingestion
</details>

## 🧩 Supported IDEs

| IDE | Auto-Install | Rules Injection |
|---|:---:|:---:|
| Cursor | ✅ | `.cursorrules` |
| Zed | ✅ | `.zed/assistant.md` |
| Claude Code | ✅ | `.claude/CLAUDE.md` |
| OpenCode | ✅ | — |
| VS Code | ✅ | — |
| Antigravity | ✅ | — |

## 📄 License

MIT License — see [LICENSE](LICENSE) for details.

---

<div align="center">
<sub>Built by <a href="https://ramzaeff.com">Maksim Ramzaev</a> · <a href="https://rms-ds.com">RMS Digital Services</a></sub>
</div>
