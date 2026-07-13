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

---

<div align="center">
  <a href="https://ko-fi.com/M7I020HKXX" target="_blank" rel="noopener noreferrer">
    <img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="ko-fi">
  </a>
</div>

---

## ✨ Key Features

| | |
|---|---|
| 🗂️ **Global Centralized Vaults** | Project context lives outside your repo — zero `.mcp` file pollution. |
| 🔍 **Hybrid Retrieval (LanceDB)** | Embedded Vector Search + Tantivy Full-Text Search for zero-fail context hits. |
| 🌐 **Multilingual Semantic Parsing** | `fastembed-rs` + `multilingual-e5-small` — native Russian & English understanding. |
| 🌳 **AST Markdown Chunker** | `pulldown-cmark`-based chunking keeps code blocks and lists bound to their parent heading. |
| 🧩 **Semantic Code Memory** | Optional Tree-sitter indexing for Rust, Go, JS/JSX, TS/TSX, Python, C/C++, Java, Ruby, Swift, and Vue `<script>` blocks; stable segment identities and repeated preambles preserve context when large implementations split. |
| 🕸️ **Knowledge Graph Foundation** | Derived Markdown/code relationships and durable user overrides are stored separately from retrieval chunks, ready for a future visual editor. |
| 🔀 **Federated Corpus Search** | Search `vault`, `code`, or `all`; mixed results use Reciprocal Rank Fusion rather than incompatible raw vector distances. |
| ⚙️ **Dynamic Auto-Installer** | `rms-memory install` scans your system and wires itself into every supported IDE. |
| 📜 **Rules-as-Code Patching** | Non-destructive AST patching of `.cursorrules`, `.zed/assistant.md`, etc. Opt-in by default. |
| 🧪 **Durable Vault Writes** | `rms_write` creates rolling `.bak` backups and atomically replaces `create`/`replace` targets after fsync, so interrupted writes never expose a truncated Markdown file. |
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

### Optional semantic code memory

Markdown memory remains the default corpus. Semantic source indexing is separate, supports all bundled language adapters, and never changes source files:

```bash
rms-memory reindex --code  # build/update only derived code memory
rms-memory reindex --all   # refresh Markdown vault + code memory
```

Registered projects support `code_index_mode = "off" | "manual" | "watch"`; the default is `off`. Set it from the project root with `rms-memory config --code-index-mode watch` (or add `--scope <project-path>`). `watch` is explicitly opt-in, coalesces supported source saves for three seconds, and coordinates concurrent IDE processes so an unchanged workspace stays idle. Code search results include their source language.

Language selection is project-scoped and defaults to every bundled adapter:

```bash
rms-memory config --code-languages auto
rms-memory config --code-languages go,typescript,tsx,vue
```

Supported names are `rust`, `go`, `javascript`, `jsx`, `typescript`, `tsx`, `python`, `c`, `cpp`, `java`, `ruby`, `swift`, and `vue`. Generated paths (`node_modules`, `.next`, `.nuxt`, `target`, `vendor`, and `coverage`) are always excluded. Ambiguous `.h` files are indexed as C exactly once; use `.hpp`, `.hh`, or `.hxx` for C++ headers. Vue indexes only inline JavaScript/TypeScript `<script>` contents and maps results back to the `.vue` host file; templates, styles, `script setup` macros, and external `src` scripts remain outside v1.0.5 semantic extraction.

## 🛠 CLI Commands

| Command | Description |
|---|---|
| `rms-memory serve` | Starts the JSON-RPC stdio server (auto-triggered by your IDE). |
| `rms-memory init` | Registers a project into the global registry. `--dry-run` supported. `--full` forces creation of all IDE rule templates. |
| `rms-memory import` | Scans for existing docs (`README.md`, `docs/`, `ADR/`) and imports them — interactively or via `--auto-import`. |
| `rms-memory install` | Hooks the server into supported IDEs. `--dry-run` supported. |
| `rms-memory uninstall` | Removes the server from all discovered IDE configurations. |
| `rms-memory doctor` | Runs 5-point vault health diagnostics. `--repair-frontmatter` safely repairs duplicate, missing, and known attached frontmatter IDs with backups; arbitrary invalid YAML is reported but never rewritten automatically. |
| `rms-memory config` | Interactive global setup; `--code-index-mode off\|manual\|watch` and `--code-languages auto\|<comma-list>` configure semantic code indexing for the current registered project. |
| `rms-memory reindex [--vault\|--code\|--all]` | Refreshes Markdown memory (default), derived semantic code memory, or both. |
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
<td>Searches Markdown memory by default. Set <code>corpus</code> to <code>code</code> or <code>all</code>; <code>all</code> uses Reciprocal Rank Fusion. Agents are instructed to call this <em>first</em>.</td>
<td><code>{ query, corpus: vault|code|all, limit, include_content, min_confidence }</code></td>
</tr>
<tr>
<td><code>rms-memory_rms_code_search</code></td>
<td>Convenience endpoint for the derived semantic code index. Results include file, symbol, kind, line range, and segment index.</td>
<td><code>{ query, limit, include_content }</code></td>
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
<summary><b>Separate Markdown and Code Corpora</b></summary>

Human-authored Markdown and derived Rust code live in separate tables. Code chunks carry stable symbol identities, line ranges, and preambles; unchanged chunks reuse their vectors. `corpus=all` fuses independently ranked result sets with Reciprocal Rank Fusion, avoiding any assumption that distances from the two corpora are comparable.
</details>

<details>
<summary><b>Graph-ready Knowledge Core</b></summary>

Graph nodes and edges are deliberately independent of retrieval chunk boundaries. Markdown links, Rust imports, trait implementations, and lexical call hints can be reconciled as derived relationships; user-created edges and suppress/restore overrides persist across reindexing. Current Rust call edges are syntax-level hints, not a compiler-accurate call graph.
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
6. PID-aware per-project writer lock and read-only background synchronization across IDE processes
7. Markdown watcher plus an explicitly opt-in, 3s-debounced Rust code watcher with shared-generation suppression
8. Write-guard snapshotting with rolling `.bak` backups (default: 5)
9. Isolated telemetry logging (`~/.rms-memory/rms.log`)
10. `llms.txt` export for flat, decoupled LLM ingestion
</details>

<details>
<summary><b>Validated v1.0.5 Multi-IDE Behavior</b></summary>

Live MCP requests have been verified for `rms_search(corpus=vault|code|all)` and `rms_code_search`. On this repository, `reindex --code` indexed 43 Rust files into 298 semantic items and 438 segments with all vectors reused on an unchanged run. An isolated five-server watcher run coalesced rapid saves into one shared completion-marker update; a later real-project stress gate completed concurrent GeoMail, License Server, RMS Monitoring, and GeoTax Site indexing, then seven MCP servers (after four IDE restarts) stayed at 0.0% CPU with no background reindex.
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
