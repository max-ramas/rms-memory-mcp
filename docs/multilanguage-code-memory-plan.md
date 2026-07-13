# Multilanguage Semantic Code Memory — Production Implementation Plan

Status: in implementation for the unreleased 1.0.5 line. The initial Rust/Go registry and Go vertical slice are implemented and covered by fixtures; Go real-project dogfood, language policy/configuration, diagnostics, and subsequent adapters remain pending.

## Outcome

RMS Memory will keep one language-neutral code corpus while parsing each source file through a versioned language adapter. Rust remains supported, and the target language set becomes:

| Tier | Languages and dialects | Delivery expectation |
|---|---|---|
| A — required | Rust, Go, TypeScript, TSX, JavaScript, JSX, Python, C | Production support before declaring the multilingual code index complete |
| B — required after qualification | C++, Java, Ruby, Swift | Production support only after grammar/runtime compatibility and fixture gates pass |
| C — embedded-language adapter | Vue 3 SFC with JavaScript/TypeScript `<script>` regions | Separate two-stage pipeline; never parse an entire `.vue` file as JavaScript or TypeScript |

This is not “install grammars and switch by extension”. Every language must satisfy the same identity, chunking, graph, incremental-update, watcher, diagnostics, and resource contracts already enforced for Rust.

The existing corpus API remains unchanged:

```text
scope = project/vault identity
corpus = vault | code | all
language = optional filter inside the code corpus
```

## Architectural decision

### 1. Extract a language adapter boundary first

Split the current Rust-specific `code_parser.rs` responsibilities into:

```text
src/code/
  mod.rs                 language-neutral public types
  registry.rs            extension/dialect detection and enabled-language policy
  parser.rs              adapter dispatch and parse-quality policy
  segmentation.rs        shared preamble-aware splitter
  relations.rs           normalized relationship types
  languages/
    rust.rs
    go.rs
    javascript.rs
    typescript.rs
    python.rs
    c.rs
    cpp.rs
    java.rs
    ruby.rs
    swift.rs
    vue.rs
```

The exact filenames may vary, but the core boundary must be explicit before the second grammar is added.

Conceptual adapter contract:

```rust
trait LanguageAdapter: Send + Sync {
    fn id(&self) -> LanguageId;
    fn extractor_version(&self) -> &'static str;
    fn supports(&self, path: &Path, source: &[u8]) -> bool;
    fn parse_items(&self, input: ParseInput<'_>) -> Result<ParseOutput>;
    fn extract_relations(&self, input: RelationInput<'_>) -> Result<Vec<RelationHint>>;
}
```

Adapters return normalized items and relationships; they do not write LanceDB, embed text, acquire locks, or know about MCP.

### 2. Normalize semantic items without flattening language meaning

Replace the Rust-only semantic assumptions with language-neutral fields:

| Field | Contract |
|---|---|
| `language` | Stable `LanguageId`, including dialect where required (`typescript`, `tsx`, `javascript`, `jsx`) |
| `container_path` | Lexical namespace/module/package/class/type path; Rust `module_path` migrates here |
| `symbol_name` | Human-readable local name |
| `qualified_symbol` | Best deterministic lexical qualification available without a compiler |
| `kind` | Normalized kind such as function, method, class, struct, enum, interface, trait, impl, module, namespace, constructor, property, constant, type_alias, module_doc |
| `native_kind` | Grammar-specific node kind for diagnostics and future migration |
| `signature` | Declaration without the full body |
| `preamble` | Attached docs, annotations, decorators, attributes, or modifiers needed to understand the item |
| `body` | Complete semantic item body |
| `source_region` | Optional host/embedded byte and line range, required for Vue |

Do not force every language feature into the normalized `kind`. Preserve grammar-specific detail in metadata.

### 3. Stable identity contract

The normal item key becomes:

```text
blake3(language + file_path + container_path + normalized_kind + semantic_identity)
```

`semantic_identity` is adapter-owned and must exclude line numbers and function bodies. It includes enough declaration information to distinguish overloads, receiver methods, constructors, generic/template variants, and multiple implementation blocks.

Required invariants:

- adding unrelated lines above an item changes navigation lines only;
- editing a body changes `item_hash` and affected `content_hash` values, not `item_key`;
- changing a declaration identity creates a new item and removes the old one;
- overloads and same-name members do not collide;
- language is part of identity, so C and C++ interpretations of the same header cannot collide;
- Vue embedded items retain the `.vue` host path and embedded-region discriminator.

The deterministic collision suffix currently used as a final safety net remains, but a fixture that reaches it is treated as an adapter defect and emits a diagnostic counter.

### 4. Shared chunking contract

All adapters use the existing preamble-aware segmentation:

- 1500 Unicode characters per target segment;
- approximately 200 characters of body overlap;
- every oversized segment repeats docs/annotations and the declaration signature;
- preambles are never silently truncated;
- `segment_index` is zero-based and stable for unchanged content;
- embeddings remain batched at no more than 8.

Each adapter must define what constitutes the preamble. Examples include Python decorators and docstrings, Java annotations, Swift attributes, C/C++ comments and attributes, and JS/TS JSDoc plus decorators/modifiers.

## Grammar and dialect policy

All grammar crates are pinned to exact compatible versions after a dependency spike. Acceptance requires one compatible Tree-sitter ABI/runtime family across all release targets. CI runs `cargo tree -d` and fails the grammar-qualification job on an unexplained duplicate Tree-sitter runtime.

| Adapter | Candidate grammar | Files/dialects | Important qualification |
|---|---|---|---|
| Rust | `tree-sitter-rust` | `.rs` | Existing baseline |
| Go | `tree-sitter-go` | `.go` | Receivers, grouped declarations, interfaces, build tags |
| TypeScript | `tree-sitter-typescript` | `.ts`, `.mts`, `.cts`; separate TSX grammar for `.tsx` | TypeScript and TSX are separate dialects and must select different grammar constants |
| JavaScript | `tree-sitter-javascript` | `.js`, `.mjs`, `.cjs`, `.jsx` | JSX is supported by the JavaScript grammar; preserve dialect in metadata |
| Python | `tree-sitter-python` | `.py`, optionally `.pyi` as a dialect | Decorators, async defs, nested defs, class methods, docstrings |
| C | `tree-sitter-c` | `.c`; `.h` only after detection/override | Macros and conditional compilation are syntax hints, not compiler truth |
| C++ | `tree-sitter-cpp` | `.cc`, `.cpp`, `.cxx`, `.hh`, `.hpp`, `.hxx`; ambiguous `.h` by policy | Templates, namespaces, overloads, constructors/destructors, qualified definitions |
| Java | `tree-sitter-java` | `.java` | Packages, nested/anonymous types, annotations, overloads |
| Ruby | `tree-sitter-ruby` | `.rb`, plus explicit known filenames | Singleton methods/classes, modules, blocks, DSL-heavy syntax |
| Swift | active `alex-pinkus/tree-sitter-swift` crate | `.swift` | Do not use the archived abandoned `tree-sitter/tree-sitter-swift` repository |
| Vue 3 | qualified maintained Vue 3 SFC grammar, with `octorus-tree-sitter-vue3` as the first candidate | `.vue` | SFC structure only; embedded script is reparsed by JS/TS adapter |

Dependency admission checklist for every grammar:

1. Rust binding exposes `LanguageFn`/`LANGUAGE` compatible with the pinned runtime.
2. Parser sources and any external scanner build on macOS ARM, Linux x64/ARM, and Windows x64.
3. License is compatible with distribution.
4. Repository is maintained or the grammar is vendored/pinned with an explicit ownership decision.
5. Binary-size and clean-build-time deltas are recorded.
6. Real project samples parse above the quality threshold.

## Per-language extraction scope

### Slice M1 — Go first

Go is the first new production adapter because it immediately unlocks dogfood on the user's largest available repositories.

Extract:

- package docs and package name;
- functions and methods, including receiver identity;
- structs, interfaces, named types, aliases, constants, and variables when documented or exported;
- import declarations;
- lexical calls;
- interface embedding and type relationships as unresolved hints.

Identity includes package/container, receiver type, symbol, and normalized signature discriminator. Fixtures cover pointer/value receivers, generics, grouped declarations, embedded interfaces, build tags, and `_test.go` files.

### Slice M2 — TypeScript/TSX and JavaScript/JSX family

Implement shared ECMAScript helpers, but retain separate adapters/dialects.

Extract:

- function declarations, named function expressions assigned to stable bindings, and arrow functions assigned to stable bindings;
- classes, constructors, methods, accessors, interfaces, enums, type aliases, namespaces/modules;
- exported constants when their initializer is a function/class/object API surface;
- JSDoc, decorators, accessibility/static/async modifiers;
- imports, exports/re-exports, extends/implements, and lexical calls.

Do not create symbols for anonymous callback expressions unless they are assigned to a stable lexical binding or exported property. Fixtures cover ESM/CJS, overload signatures, generics, declaration files, JSX/TSX components, React hooks, decorators, and nested namespaces.

### Slice M3 — Python

Extract modules, classes, functions, async functions, methods, decorated definitions, type aliases where syntactically recognizable, imports, class bases, and lexical calls.

Preamble includes decorators and the leading docstring. Identity includes the complete lexical container path. Fixtures cover nested functions, async code, properties/classmethods/staticmethods, overload decorators, `.pyi`, multiple inheritance, and syntax-error recovery during an editor save.

### Slice M4 — C and C++ together

Implement separate adapters with shared declarator utilities.

C extracts functions, structs/unions/enums, typedefs, global declarations with API significance, includes, and lexical calls.

C++ additionally extracts namespaces, classes, methods, constructors/destructors, templates, aliases, overloads, inheritance, and qualified out-of-class definitions.

Header policy:

1. Explicit `code_language_overrides` wins.
2. `.hpp/.hh/.hxx` is C++; `.c` is C.
3. `.h` uses project evidence (`compile_commands.json`, neighboring source family, C++-only syntax score).
4. If still ambiguous, choose C and emit an ambiguity diagnostic; never index the same header twice by default.

Preprocessor includes and macro invocations are unresolved graph hints. RMS Memory does not claim compiler-accurate preprocessing or template instantiation.

### Slice M5 — Java, Ruby, Swift

Java extracts packages, classes, interfaces, records, enums, constructors, methods, annotations, imports, extends/implements, and lexical calls.

Ruby extracts modules, classes, instance/singleton methods, constants, `require`/`require_relative`, superclass relations, and lexical calls. DSL calls remain calls, not invented class/method definitions, unless a later opt-in framework adapter owns that interpretation.

Swift extracts imports, protocols, classes, structs, actors, enums, extensions, initializers, functions, methods, properties with substantial bodies, inheritance/conformance, attributes, and lexical calls.

Each adapter ships only after its grammar passes the same fixtures and cross-platform build matrix as Tier A. “Grammar parses the file” is not sufficient acceptance.

## Vue 3 SFC design

Vue is a host-language adapter with embedded regions.

### Stage 1 — Parse the SFC shell

The Vue grammar identifies:

- `template` region;
- one or more `script` regions;
- `setup` attribute;
- `lang="ts"`, `lang="js"`, or absent language;
- `src` attribute;
- style regions, which are ignored by semantic code memory in the first release.

The adapter records the component node for the `.vue` file and source mappings for embedded regions.

### Stage 2 — Reparse script content

For each inline script region:

1. Extract the exact inner byte slice, excluding the `<script>` tags.
2. Select TypeScript/TSX or JavaScript/JSX adapter from `lang` and dialect policy.
3. Parse the extracted content as a virtual document.
4. Map byte and line ranges back to the original `.vue` file.
5. Emit normal code items with `file_path` equal to the `.vue` host, plus `host_language=vue`, `embedded_language`, region index, and `setup` metadata.

For `src="..."`, resolve only project-relative, containment-safe paths. The referenced JS/TS file is indexed by its normal adapter; the Vue component gets a derived `uses_script` edge rather than duplicate chunks.

### Vue semantic additions

The first Vue release supports:

- `<script setup>` bindings;
- `defineProps`, `defineEmits`, `defineExpose`, and `defineOptions` macro calls as structured component metadata/edges;
- Composition API functions and composables through the normal JS/TS adapter;
- Options API object sections (`props`, `emits`, `methods`, `computed`, `setup`) through a Vue-specific projection;
- template component tags as unresolved `uses_component` hints.

Template expression semantics, CSS, framework-aware component resolution, and Volar-level type analysis remain out of scope. Tree-sitter language injection is the architectural model, but RMS Memory performs the nested parse explicitly so it can preserve source maps and stable identities. Tree-sitter officially models mixed-language files as parent trees plus injected child trees, which matches this pipeline.

## Parse quality and editor-save behavior

The current Rust parser rejects a whole file when the root contains any syntax error. That policy must change before dynamic languages and Vue watchers ship.

New policy:

- never commit partial rows over a previously good file snapshot when parse quality falls below threshold;
- allow extraction of items that do not intersect `ERROR` or `MISSING` nodes when the quality threshold passes;
- retain last-known-good rows and schedule one retry for transient incomplete saves;
- record per-file diagnostics: language, grammar version, error-node count, skipped-item count, ambiguity, parse duration;
- do not log source content or secrets;
- manual reindex summarizes failures by language and returns nonzero only for systemic failure or configured strict mode.

Default quality threshold proposal: no error node may cover a top-level semantic declaration, and error bytes must remain below 2% of the file. The threshold is finalized with fixtures, not intuition.

## Storage and migration

The existing `language` field becomes authoritative. Add nullable fields through a zero-downtime migration:

- `container_path`;
- `native_kind`;
- `extractor_version`;
- `dialect`;
- `host_language`;
- `region_index`;
- `region_start_byte` / `region_end_byte`.

Keep reading legacy Rust `module_path` rows during migration. A full code reindex writes the new schema; Vault tables remain untouched.

Graph extractors are versioned per adapter, for example:

```text
rust-tree-sitter-v2
go-tree-sitter-v1
typescript-tree-sitter-v1
javascript-tree-sitter-v1
python-tree-sitter-v1
c-tree-sitter-v1
cpp-tree-sitter-v1
java-tree-sitter-v1
ruby-tree-sitter-v1
swift-tree-sitter-v1
vue-sfc-tree-sitter-v1
```

Reconciliation prunes only rows owned by the same extractor. User-created graph edges and overrides remain untouched.

## Configuration and public API

Proposed project configuration:

```toml
code_index_mode = "off"                 # off | manual | watch
code_languages = ["auto"]               # or an explicit allow-list
code_language_overrides = [
  { glob = "native/**/*.h", language = "cpp" }
]
vue_sfc_mode = "script"                 # off | script | component
```

Semantics:

- `auto` means all production-qualified bundled adapters, based on file detection;
- experimental adapters are never enabled by `auto`;
- explicit language lists are validated atomically by `ConfigManager`;
- changing languages or overrides marks the code index dirty and schedules one generation only in `watch` mode;
- manual `reindex --code` always honors the configured language policy.

Extend code search with optional filters while preserving existing calls:

```json
{
  "query": "where is tax calculation dispatched",
  "languages": ["go", "typescript"],
  "kinds": ["function", "method"],
  "path_prefix": "internal/",
  "limit": 20
}
```

Every code result returns `language`, `dialect`, `container_path`, and host-region metadata when applicable. `rms_search(corpus=all)` keeps RRF and may forward the optional code filters only to the code branch.

## Watcher and incremental indexing

Replace `is_watched_rust_path` with registry-backed detection. Watcher behavior remains project-wide:

- only enabled production adapters contribute watched paths;
- ignored/generated/vendor directories remain excluded;
- rapid mixed-language saves share the same three-second dirty generation;
- the project lock and `.code-index.updated` marker remain authoritative;
- deleted, renamed, newly unsupported, or reclassified files remove stale rows;
- a language-policy change forces a full code generation;
- per-language counts are emitted in job progress and final statistics.

Do not spawn one watcher or embedding model per language.

## Delivery slices and gates

### M0 — Language-neutral refactor

- Introduce `LanguageId`, adapter registry, normalized kinds/relations, parse diagnostics, and shared segmentation.
- Move Rust behind the adapter without changing Rust output.
- Add schema migration and optional search filters.
- Acceptance: existing Rust golden fixtures and live reindex counts remain stable; no extra idle work.

### M1 — Go vertical slice

- Add grammar, adapter, graph hints, watcher detection, CLI statistics, and fixtures.
- Dogfood on the user's large Go project.
- Acceptance: stable IDs across line/body edits, changed-only embeddings, useful search results, one watcher generation, idle CPU approximately 0%.

### M2 — Web language family

- Add TypeScript, TSX, JavaScript, and JSX adapters with shared helpers.
- Acceptance includes a real mixed TS/JS project and framework-neutral fixtures.

### M3 — Python

- Add Python and optional `.pyi` dialect support.
- Acceptance includes decorators, nested scopes, docstrings, incomplete-save recovery, and a real project.

### M4 — C/C++

- Add both adapters, declarator utilities, header classification, and overrides.
- Acceptance includes `compile_commands.json`, templates/overloads, macros, and ambiguous-header tests.

### M5 — Java/Ruby/Swift

- Qualify grammars and land adapters independently behind production flags.
- A failed grammar qualification delays only that adapter, not the already qualified languages.

### M6 — Vue SFC

- Add SFC shell parser, embedded source mapping, JS/TS nested parsing, script-setup macros, and component hints.
- Acceptance covers JS/TS, setup/non-setup, Options API, external `src`, multiple blocks, malformed template with valid script, and original-line navigation.

### M7 — Full release gate

- Cross-platform release builds with every bundled grammar.
- Mixed-language repository migration from Rust-only schema.
- Five-process watcher test with rapid saves across at least three languages.
- Large Go dogfood plus one web and one C-family project.
- Record binary size, clean build time, scan/parse/embed duration, peak RSS, reused/embedded counts, parse diagnostics, lock contention, and idle CPU.
- Update README, CLI help, MCP schemas, roadmap, changelog, and memory artifacts.

## Fixture matrix required for every adapter

Each language must cover:

1. top-level and nested symbols;
2. docs/annotations/decorators attached to the correct item;
3. same-name symbols in different containers;
4. overload/receiver/generic variants where the language permits them;
5. stable IDs after unrelated lines are inserted;
6. stable IDs after body-only edits;
7. preamble repetition in oversized segments;
8. imports/dependencies, inheritance/implementation, and lexical calls;
9. syntax errors and incomplete editor saves;
10. Unicode identifiers/content where supported;
11. generated/vendor/ignored paths;
12. deletion, rename, language reclassification, and orphan cleanup.

Golden snapshots assert normalized items, identities, ranges, preambles, graph hints, and diagnostics. Grammar node names are allowed only inside an adapter and its fixtures.

## Resource and packaging budgets

The multilingual release is rejected if it regresses idle behavior.

- No grammar work occurs when `code_index_mode=off`.
- Grammar objects are lightweight and may be registered eagerly; parsers are created/reused per job, not per file when avoidable.
- The embedding model remains a single shared instance per process.
- Parse work uses bounded concurrency independent of ONNX threads.
- Initial default: 2 parser workers, configurable downward. A hard ceiling of `min(available_parallelism, 4)` may be enabled only after the five-IDE resource gate passes; embedding remains serialized through the existing indexer.
- Record per-grammar contribution to release binary size. If the full bundle becomes unacceptable, evaluate feature-gated bundles before considering runtime grammar downloads.
- No grammar is downloaded at runtime in the default production build.

## Security and correctness boundaries

- Source parsing is read-only.
- Vue external `src` paths use canonical containment checks and never fetch URLs.
- Tree-sitter results are syntax-level. Calls, imports, includes, inheritance, and component references remain unresolved or ambiguous until a future resolver proves otherwise.
- Do not claim compiler/LSP accuracy for macros, dynamic dispatch, monkey patching, templates, build tags, conditional compilation, or framework DSLs.
- Malformed source never deletes last-known-good rows merely because an editor save is incomplete.
- Derived graph reconciliation never mutates user edges or overrides.

## Definition of done

A language is “supported” only when it has:

- a pinned, cross-platform grammar dependency;
- a production adapter and versioned extractor;
- the full fixture matrix;
- stable identity and preamble-aware segmentation evidence;
- incremental reuse and stale-row cleanup;
- graph hints with honest resolution state;
- watcher and mixed-process coverage;
- search result metadata and filters;
- dogfood measurements on a real project;
- public documentation.

Until all conditions pass, the language is `experimental`, not silently included in `auto`.

## Primary references

- Tree-sitter language injections: <https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html>
- TypeScript and TSX grammars: <https://github.com/tree-sitter/tree-sitter-typescript>
- JavaScript and JSX grammar: <https://github.com/tree-sitter/tree-sitter-javascript>
- Official parser directory: <https://github.com/tree-sitter/tree-sitter/wiki/List-of-parsers>
- Active Swift grammar: <https://github.com/alex-pinkus/tree-sitter-swift>
- Vue 3 grammar candidate: <https://docs.rs/octorus-tree-sitter-vue3/latest/tree_sitter_vue3/>
