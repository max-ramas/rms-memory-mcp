# Code layer: trusting what code search returns

Code search is only useful when the agent knows how far to trust it. Three signals come with every
code search (`rms_code_search`, or `rms_search` with `corpus: "code"|"all"`):

## `code_index` — is the index current?

| status | meaning | what the agent does |
| --- | --- | --- |
| `fresh` | no indexed file changed since the last index | trust the hits |
| `stale` | `changed_since_index` files were edited (or `removed_since_index` deleted) after the last index | re-read those files before relying on hits for them |
| `not_indexed` | `code_index_mode` is `off`, or the index was never built | use grep/read, and tell the user the `hint` (`rms-memory config --code-index-mode manual`, then `rms-memory reindex --code`) |

The index stays opt-in (`off` by default): enabling it costs disk and embedding time, which is the
user's decision. `watch` keeps it fresh automatically. `rms-memory doctor` prints the same state.

Staleness is computed from file modification times of the indexed files; files *added* since the last
index are not counted (finding them would need a tree walk on every search).

## `related_notes` — what has the team already decided about this file?

Each code file in the results carries up to three vault notes that mention it, attached once to the
first hit of that file. Matching is deterministic, no model involved:

* `path`: the note names the file (`src/updater.rs`, or a longer path ending in it);
* `symbol`: a backticked identifier in the note matches the hit's symbol (qualified names, or bare
  names of 6+ characters);
* `name`: only the bare file name is mentioned (generic names such as `mod.rs` or `index.ts` never match);
* patterns also match through their `refs` frontmatter and rank by `pattern_status`;
* superseded notes are skipped.

`code_changed_since_note: true` means the file was modified after the note was last edited. The note
may describe older code: verify it instead of trusting it. The lookup reads the vault's Markdown
files and is cached until a note is added, removed or edited. `include_notes: false` disables it.

## Call-graph edges are hints

`calls_symbol` edges come from syntax, not type resolution. `rms_graph` neighbors show each edge's
`resolution` (`resolved`, `ambiguous`, `unresolved`) and `confidence`, and report `unconfirmed_edges`.
Treat anything that is not `resolved` as a lead to verify with grep or by reading the code.
