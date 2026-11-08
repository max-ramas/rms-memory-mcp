# Dialogs archive (`rms-memory dialogs`)

Collects AI-assistant dialogs that **already sit on your disk** into one archive tree, so a blocked
account, a lost laptop or a move to another machine does not lose the chain of reasoning behind a
project. It only reads files the tools wrote themselves. It does not intercept traffic, hook
processes or talk to any service. Nothing leaves the machine unless you sync the archive folder.

Off by default: nothing is collected until you run `rms-memory dialogs init`.

## Quick start

```
rms-memory dialogs init --encrypt          # archive in ~/dialogs-archive, config in ~/.rms-memory/dialogs/
rms-memory dialogs collect --index         # copy new sessions, then update the search index
rms-memory dialogs search "why we dropped the cloud sync"
rms-memory dialogs session claude-code/<project>/<file>.jsonl
```

## Sources

| Tool | Read from | How |
|---|---|---|
| Claude Code | `~/.claude/projects/**/*.jsonl` | file copy, append-aware |
| Codex | `~/.codex/sessions/**/rollout-*.jsonl` | file copy |
| Antigravity | `~/.gemini/antigravity/brain/<id>/` transcripts and plans, `conversations/<id>.db` | file copy; DB as a consistent snapshot |
| OpenCode | `opencode.db`, tables `session`, `message`, `part` only | readable JSONL export |
| Cursor | `state.vscdb`, keys `composerData:*` and `bubbleId:*` only | readable JSONL export |
| Zed | `threads.db`, table `threads` | readable JSON export |

Never read: OpenCode `credential`/`account`/`control_account`/`auth.json`, Cursor `ItemTable`,
Gemini OAuth and credential files. Only the paths above are opened (allow-list).

## Modes

| Mode | Collects | Exposed to an agent |
|---|---|---|
| `off` | no | no |
| `collect` (default after `init`) | yes | no |
| `collect-and-serve` | yes | yes: MCP tools `rms_dialogs_search`, `rms_dialogs_read` |

`rms-memory dialogs mode collect-and-serve`. In any other mode the two tools are not listed and
cannot be called. Their answers are marked as past conversation data, not instructions.

An encrypted archive is only served when the server has `RMS_DIALOGS_PASSPHRASE` in its environment
(a background server has no terminal to ask on); without it the tools answer that the archive is locked.

## Archive and encryption

```
<archive>/<tool>/<project>/<name>.gNNNN.sNNNN[.age]   append-only segments of a generation
<archive>/.state.json   .index.db[.age]   .keys/identity.age
```

Every change appends only the new tail as the next segment; a rewritten source starts a new
generation (up to 5 kept); after 48 segments the file is compacted. Writes are atomic. Sources that
vanish are kept.

With `--encrypt` each segment and the search index are encrypted to an age X25519 public key, so the
background collector never needs your passphrase. The secret key is stored in `.keys/identity.age`
protected by your passphrase; it is needed to read, search, index and restore. File and project names,
sizes and times are not encrypted. Losing the passphrase makes the archive unreadable.

## Commands

```
dialogs init [--out DIR] [--encrypt]        dialogs mode off|collect|collect-and-serve
dialogs collect [--watch] [--interval 300] [--index]
dialogs index                               update the search index (user and assistant turns only)
dialogs search <words> [--tool T] [--project P] [--limit N]
dialogs session <tool/project/file>         one dialog as Markdown
dialogs status | read <file> [--generation N] | verify [--deep]
dialogs restore --to DIR | --native         native puts original files back, never overwriting
```

`RMS_DIALOGS_PASSPHRASE` skips the passphrase prompt (automation; do not keep it set permanently).
`RMS_DIALOGS_TIMING=1` prints time per source.

Restore: Claude Code, Codex and Antigravity keep original bytes (the tool can resume the session);
OpenCode, Cursor and Zed are readable exports (searchable, not resumable).

## Search index

Only what a person and an assistant said is indexed: tool output, reasoning and injected context are
left out. SQLite FTS5; with an encrypted archive the index is encrypted too and loaded into memory
for a query, so no plaintext of your dialogs is written to disk.

## Resources (measured, release build, 1632 sessions, 4.5 GB encrypted archive)

- First full pass (collect + index): 137 s, 55,031 turns, index 84 MB, peak 1.4 GB (the largest single session, a 409 MB Cursor export, is held in memory while it is encrypted).
- Background collector (`collect`, no passphrase): 50 MB peak, about 3 s per pass, 0.09 s when nothing changed; it does not unlock the key.
- Anything that unlocks the key (`read`, `session`, `search`, `index`, `restore`, `verify`, `collect --index` when sessions changed) pays about 550 MB and 1.3 s once per process: that is scrypt, a memory-hard key derivation by design, freed right after unlocking. Index update and search add up to about 300 MB while the index is in memory.
- No leak: 12 consecutive `--watch --index` passes returned to 9-26 MB between passes with no upward trend; open files stayed at 18.

## Limits

Many small segments for a session edited every minute until compaction; names are visible in
encrypted mode; no VS Code / Copilot Chat source yet; the index is not yet used by `rms_search` or the
graph (planned option "use dialogs in search and graph").
