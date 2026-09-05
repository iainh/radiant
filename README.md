# Radiant

A strict, async, Qute-style template engine for Rust, with first-class Axum support.

Radiant brings the useful ideas from [Quarkus Qute](https://quarkus.io/guides/qute-reference) to Rust without copying its Java object model. Templates are checked at compile time, data is exposed through explicit derives rather than reflection and rendering fails loudly instead of printing blanks.

## Features

- **Checked templates.** `#[derive(Template)]` parses the template and every static include at compile time, so a missing file or a broken include graph fails the build.
- **Compiled hot paths.** Templates made of text, expressions, conditionals and loops compile to direct Rust field access and control flow. Templates that need includes, resolvers or variants fall back to the full evaluator automatically.
- **Strict by default.** Unresolved values are errors unless you make them safe with `??` or supply a default with `?:`.
- **Async resolvers.** Value and namespace resolvers are asynchronous and run in deterministic priority order.
- **Media and locale variants.** `invoice.html`, `invoice.json` and `invoice.fr.html` are selected from the request, and output is escaped for the media type it targets.
- **Axum integration.** A `Renderer` extractor handles content negotiation, locale selection, deadlines and error responses.
- **Runtime templates.** The same parser and evaluator serve templates loaded from disk, built at runtime or authored by end users under a restricted engine.

## Workspace

| Crate | Purpose |
| --- | --- |
| `radiant` | Runtime, value model, checked template derives, loaders and rendering |
| `radiant-compiler` | Dependency-light parser and owned AST for runtime use and tooling |
| `radiant-macros` | Compile-time template loading, dependency validation and derives |
| `radiant-axum` | Request extraction, content negotiation, deadlines and responses |
| `radiant-lsp` | Language server for diagnostics, completion and navigation |

## Quick start

Radiant isn't published to crates.io yet. Add it as a path or git dependency:

```toml
[dependencies]
radiant = { path = "../radiant" }
radiant-axum = { path = "../radiant/radiant-axum" }
```

Put templates under `templates/` at your crate root:

```html
<!-- templates/products.html -->
<h1>{title}</h1>
{#for product in products}
  <p>{product.name}: {product.price}</p>
{#else}
  <p>No products</p>
{/for}
```

Derive `TemplateValue` on the data you want templates to see and `Template` on the page model:

```rust
use radiant::{Engine, Template, TemplateValue};

#[derive(TemplateValue)]
struct Product {
    name: String,
    price: i64,
}

#[derive(Template)]
#[template(path = "products.html")]
struct ProductsPage<'a> {
    title: &'a str,
    products: &'a [Product],
}

async fn example() -> Result<(), radiant::RenderError> {
    let rendered = Engine::new()?
        .render(ProductsPage { title: "Products", products: &[] })
        .await?;
    println!("{rendered}");
    Ok(())
}
```

Templates cannot reflect over arbitrary application objects. Only fields exposed through `TemplateValue` are visible, and the derive embeds the whole dependency graph in the binary with `include_bytes!`, so Cargo rebuilds when any template changes.

## Checked templates

For templates built from text, expressions, conditionals and loops, the derive emits direct Rust field access and control flow. Adjacent literal output is merged, values are escaped straight into the destination and integers and floats are formatted on the stack. Templates that use includes, runtime resolvers, locale or media variants, or other dynamic features use the full Qute evaluator instead. You don't choose between the two paths; the derive does.

High-throughput callers can reuse one output buffer across renders:

```rust
let engine = Engine::new()?;
let mut output = String::new();

engine
    .render_into(ProductsPage { title: "Products", products: &[] }, &mut output)
    .await?;
```

## Axum

Store an `Engine` in router state and extract a request-scoped `Renderer`:

```rust
use axum::{Router, routing::get};
use radiant::Engine;
use radiant_axum::{RenderRejection, Renderer, TemplateResponse};

async fn products(renderer: Renderer) -> Result<TemplateResponse, RenderRejection> {
    renderer
        .render(ProductsPage { title: "Products", products: &[] })
        .await
}

fn router() -> Router {
    Router::new()
        .route("/products", get(products))
        .with_state(Engine::new().expect("valid engine"))
}
```

`Renderer` does the following:

- negotiates HTML, text, JSON or XML from the `Accept` header, and returns 406 for unsupported representations
- selects locale variants from `Accept-Language`
- sets `Content-Type` and `Content-Language` on the response
- enforces a rendering timeout when a `RenderDeadline` is present in request extensions

`Renderer::render` buffers the whole template before sending headers, so a render error becomes a clean HTTP error response. `Renderer::stream` defers rendering to the response body; use it when time to first byte matters more than clean error handling.

## Template language

Radiant supports the Qute concepts that compose cleanly in Rust:

- strict expressions with member and index access, calls, arithmetic, comparisons, `&&`, `||` and `!`
- safe lookup (`??`) and Elvis defaults (`?:`)
- `{#if}`, `{#for}`/`{#each}`, `{#let}`/`{#set}` and `{#when}`/`{#switch}` sections
- includes, insert blocks, layouts, fragments, isolated tag templates and include-cycle detection
- comments (`{! ... !}`), unparsed blocks (`{| ... |}`) and parameter declarations (`{@Type name}`)
- asynchronous value and namespace resolvers with deterministic priorities
- media-aware HTML, XML and JSON string escaping, with `SafeHtml`, `SafeXml` and `SafeJsonString` wrappers for content you've already escaped
- media variants such as `invoice.html` and `invoice.json`, and locale variants such as `invoice.fr.html`
- locale-aware message namespaces with exact-language, base-language and default fallbacks

### Runtime templates

Runtime templates use the same parser and evaluator as checked templates:

```rust
let engine = Engine::builder()
    .template("greeting.txt", "Hello {name ?: 'world'}")
    .build()?;

let output = engine
    .template("greeting")
    .await?
    .data("name", "Mina")
    .render()
    .await?;
```

Use `FileLoader` to load templates from disk on demand. `Engine::reload` and `Engine::replace` refresh templates explicitly during development; there is no hidden watcher or global cache.

Enable the `serde` feature to convert dynamic data with `Value::from_serialize`. Serde isn't required for checked templates or the core value model.

## Language server

Build or install the stdio language server from this workspace:

```console
cargo install --path radiant-lsp
```

`radiant-lsp` provides live parser diagnostics, document symbols, snippet completion for built-in sections, scoped variables, template IDs, user tags, referenced fragments and layout blocks, hover documentation, and go-to-definition for local declarations, includes and user tags. Completion candidates are filtered and ranked server-side. It discovers templates below each workspace's `templates/` directory and refreshes them when the editor reports file changes.

### Neovim

The repository includes a Tree-sitter grammar in `tree-sitter-radiant/`. It extends the standard HTML grammar, so HTML tags and attributes remain highlighted alongside Radiant declarations, sections and expressions. To install its vendored C parser and queries for Neovim 0.11 or newer without an editor plugin, run from the repository root:

```console
parser_dir="${XDG_DATA_HOME:-$HOME/.local/share}/nvim/site/parser"
query_dir="${XDG_DATA_HOME:-$HOME/.local/share}/nvim/site/queries/radiant"
mkdir -p "$parser_dir" "$query_dir"
cc -O2 -fPIC -shared -I tree-sitter-radiant/src \
  tree-sitter-radiant/src/parser.c tree-sitter-radiant/src/scanner.c \
  -o "$parser_dir/radiant.so"
cp tree-sitter-radiant/queries/*.scm "$query_dir/"
```

Then mark files below `templates/` as Radiant templates, start Tree-sitter highlighting and enable the built-in LSP client:

```lua
vim.filetype.add({
  pattern = { [".*/templates/.*"] = "radiant" },
})

vim.api.nvim_create_autocmd("FileType", {
  pattern = "radiant",
  callback = function(args)
    vim.treesitter.start(args.buf, "radiant")
  end,
})

vim.lsp.config.radiant = {
  cmd = { "radiant-lsp" },
  filetypes = { "radiant" },
  root_markers = { "Cargo.toml", ".git" },
}
vim.lsp.enable("radiant")
```

The server uses full-document synchronization. It understands declarations and lexical scope within templates, but does not yet resolve Rust types or provide member-field completion. Formatting, rename and code actions are also not implemented.

## Security model

Rendering is strict by default: unresolved values are errors unless made safe with `??` or handled with `?:`. Template IDs can't be absolute or escape their configured root. Engines cap include depth and output size, and report structured errors with source spans and render stacks.

For templates written by end users, start from `Engine::builder().restricted()`. A restricted engine disables includes and tag invocation, permits only data-oriented sections and the `data:` namespace, and lowers the output limit. You must opt in to any additional sections or namespaces explicitly.

## Development

```console
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

The optional editor acceptance test requires Neovim 0.11.4, a built server binary and a compiled parser:

```console
cargo build -p radiant-lsp
mkdir -p target/tree-sitter
cc -O2 -fPIC -shared -I tree-sitter-radiant/src \
  tree-sitter-radiant/src/parser.c tree-sitter-radiant/src/scanner.c \
  -o target/tree-sitter/radiant.so
RADIANT_LSP="$PWD/target/debug/radiant-lsp" \
RADIANT_TS_PARSER="$PWD/target/tree-sitter/radiant.so" \
  nvim --clean --headless -l radiant-lsp/tests/neovim_smoke.lua
```

The design rationale, including how Radiant maps Qute concepts onto Rust and Axum, is in [`docs/research`](docs/research/).

## Licence

Licensed under either of the [MIT licence](LICENSE-MIT) or the [Apache License, Version 2.0](LICENSE-APACHE), at your option.
