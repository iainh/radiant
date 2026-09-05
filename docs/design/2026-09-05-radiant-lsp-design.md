# Radiant language server design

Status: implementation plan  
Date: 2026-09-05

## Goal

Add a small, dependable Language Server Protocol (LSP) server for Radiant templates. The first release should make authoring `.html` templates safer without introducing a second parser or pretending to know Rust types that the compiler cannot yet resolve.

The staged implementation must provide:

- live parser and validator diagnostics;
- section, block, fragment, and parameter completion;
- document symbols for parameters, sections, blocks, fragments, and captures;
- hover information for language constructs and local declarations;
- go-to-definition for local declarations and static template references;
- an executable `radiant-lsp` server that works with a standard editor LSP client.

## Constraints and non-goals

- `radiant-compiler` remains the source of truth for syntax, spans, and validation.
- LSP positions use UTF-16 code units while compiler spans use UTF-8 byte offsets. Every protocol boundary must use one tested conversion module.
- The server owns open document text. Disk reads are only used to discover and navigate templates that are not open.
- Files under a workspace's `templates/` directory are template IDs relative to that directory. A document outside such a directory has no inferred template root.
- Rust type and field resolution is out of scope. It requires Cargo metadata and Rust semantic information that Radiant does not currently expose.
- Formatting, rename, references, code actions, and incremental parsing are out of scope for the first release.

## Architecture

Add a `radiant-lsp` workspace crate with a library and binary:

```diagram
┌──────────────┐  JSON-RPC/stdio  ┌────────────────────┐
│ Editor client│◀────────────────▶│ radiant-lsp server │
└──────────────┘                  └─────────┬──────────┘
                                           │
                              ┌────────────▼────────────┐
                              │ open document snapshots │
                              │ + shared analysis model │
                              └────────────┬────────────┘
                                           │
                              ┌────────────▼────────────┐
                              │ radiant-compiler parser │
                              │ AST + diagnostics       │
                              └─────────────────────────┘
```

The library owns feature logic so tests can call it without a subprocess. The binary only starts the Tokio runtime and serves LSP over stdin/stdout. `tower-lsp` supplies protocol types and transport.

### Recoverable compiler output

Introduce `radiant_compiler::analyze(name, source) -> Analysis`, where `Analysis` contains the best-effort `Template` and all diagnostics. Keep `parse` source-compatible by returning `Err(analysis.diagnostics)` when diagnostics are present.

The parser already accumulates nodes and diagnostics. Exposing both prevents the LSP from maintaining a divergent parser and preserves symbols from valid regions of a temporarily broken document.

### Document analysis

Each open document snapshot stores its text, version, compiler analysis, and inferred template root. Feature handlers derive their answers from this immutable snapshot. Re-analysis is initially whole-document because templates are small and the parser is linear; incremental parsing can be added only if measurement justifies it.

One `LineIndex` converts byte offsets to and from LSP UTF-16 positions. It clamps malformed/out-of-range requests to safe source boundaries and is covered with ASCII, multibyte, astral, CRLF, and multiline tests.

### Semantic index

A traversal over the recoverable AST records:

- declarations: template parameters, `for`/`each` aliases, and named `let`/`set` bindings;
- scopes: template, section body, and block ranges;
- language constructs: section and block names;
- static template references: the first argument to `include` and user-tag section names.

The narrowest containing scope wins. Hover and go-to-definition share this index so they cannot disagree about name resolution.

## Staged delivery

Each stage ends with focused tests, a full workspace test run, a commit, and a push before the next stage starts.

### Stage 1 — recoverable analysis, transport, and diagnostics

1. Add the recoverable compiler API while preserving `parse` behaviour.
2. Add the `radiant-lsp` crate, stdio binary, document store, and UTF-16 line index.
3. Advertise text synchronization and publish compiler diagnostics on open/change; clear them on close.
4. Map compiler code, message, severity, and exact span to LSP diagnostics.

Tests:

- compiler tests prove valid and invalid sources both return the expected best-effort AST;
- unit tests cover every position conversion edge case;
- backend tests cover open, change, stale-version rejection, close, and diagnostic clearing;
- JSON-RPC integration test initializes the binary and observes published diagnostics.

### Stage 2 — structural navigation and syntax completion

1. Return hierarchical document symbols for parameters, sections, blocks, fragments, and captures.
2. Complete built-in section names after `{#` and valid block names after `{#` inside a section.
3. Complete in-scope parameter and local names in expression positions.
4. Keep completion context detection deliberately lexical and cursor-local so incomplete tags still work.

Tests:

- symbol hierarchy and source ranges, including Unicode before a symbol;
- built-in, block, and local completion contexts;
- suppression in comments, unparsed blocks, and plain HTML;
- protocol capability and response-shape integration tests.

### Stage 3 — hover and local go-to-definition

1. Hover built-in sections/blocks with concise syntax documentation.
2. Hover a declared name with its declaration kind and declared type when available.
3. Resolve local uses to the nearest visible parameter, loop alias, or `let`/`set` binding.
4. Return no result for unknown identifiers rather than guessing.

Tests:

- nested scope shadowing and sibling-scope isolation;
- cursor positions at identifier boundaries and on multibyte text;
- hover Markdown and definition target ranges;
- end-to-end JSON-RPC requests against the binary.

### Stage 4 — template discovery and navigation

1. Discover template IDs beneath each workspace `templates/` directory.
2. Complete static `include` IDs and user-tag names from `templates/tags/`.
3. Resolve static includes and user tags to file locations.
4. Refresh discovery when watched template files are created or deleted.

Tests:

- temporary multi-root workspaces;
- normalized IDs, nested paths, and missing targets;
- include and tag completion and definitions;
- file-watch refresh without restarting the server.

### Stage 5 — editor acceptance and documentation

1. Document installation, stdio invocation, supported features, and a minimal editor configuration.
2. Build the release binary.
3. Drive the server from a real editor in headless mode against a fixture workspace.
4. Assert that the editor receives diagnostics, completion, symbols, hover, and both local and template definitions.

Neovim is the reference editor because its built-in LSP client can be scripted reproducibly. The acceptance script downloads no editor plugins and fails on missing or malformed responses. Protocol integration tests remain the primary regression suite; the editor run proves interoperability rather than replacing them.

## Completion and correctness criteria

The first release is complete when:

- `cargo test --locked --workspace --all-features` and Clippy pass;
- malformed templates retain useful symbols from successfully parsed regions;
- all reported ranges remain correct with non-ASCII source text;
- no feature reports declarations outside lexical scope;
- template navigation cannot escape the inferred template root;
- a clean Neovim session starts `radiant-lsp` and exercises every advertised feature;
- README limitations match actual server capabilities.

## Future extensions

Type-aware member completion should be a separate project. It will need a stable bridge from template parameter type names to Rust Analyzer or compiler metadata, plus invalidation tied to Cargo changes. That work should extend the semantic index rather than alter LSP transport or reparsing.
