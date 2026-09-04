# Radiant

Radiant brings the useful ideas from [Quarkus Qute](https://quarkus.io/guides/qute-reference) to Rust without copying its Java object model. It combines a Qute-style template language with Rust data derives, asynchronous extension points, strict rendering, and an Axum-native response layer.

The workspace contains:

- `radiant`: runtime, value model, checked template derives, loaders, and rendering
- `radiant-compiler`: dependency-light parser and owned AST for runtime use and tooling
- `radiant-macros`: compile-time template loading, dependency validation, and derives
- `radiant-axum`: request extraction, content negotiation, deadlines, and responses

## Checked templates

Put application templates below `templates/` and derive `Template` on their data model:

```html
<!-- templates/products.html -->
<h1>{title}</h1>
{#for product in products}
  <p>{product.name}: {product.price}</p>
{#else}
  <p>No products</p>
{/for}
```

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

# async fn example() -> Result<(), radiant::RenderError> {
let rendered = Engine::new()?
    .render(ProductsPage { title: "Products", products: &[] })
    .await?;
# Ok(())
# }
```

The derive reads and parses the root template and every static include during compilation. The complete dependency graph is embedded in the binary, and `include_bytes!` makes Cargo rebuild it when any template changes. Rust fields are exposed explicitly through `TemplateValue`; templates cannot reflect over arbitrary application objects.

## Axum

Store an `Engine` in router state and extract a request-scoped `Renderer`:

```rust
use axum::{Router, routing::get};
use radiant::Engine;
use radiant_axum::{RenderRejection, Renderer, TemplateResponse};

async fn products(
    renderer: Renderer,
) -> Result<TemplateResponse, RenderRejection> {
    renderer.render(ProductsPage { title: "Products", products: &[] }).await
}

# fn router() -> Router {
Router::new()
    .route("/products", get(products))
    .with_state(Engine::new().expect("valid engine"))
# }
```

`Renderer` negotiates HTML, text, JSON, or XML from `Accept`, selects locale variants from `Accept-Language`, and sets `Content-Type` and `Content-Language`. Unsupported representations return 406. Add `RenderDeadline` to request extensions to enforce a rendering timeout. `Renderer::render` buffers before sending headers so errors become clean HTTP responses; `Renderer::stream` defers rendering to the response body when that trade-off is appropriate.

## Template language

Radiant supports the Qute concepts that compose cleanly in Rust:

- strict expressions, member/index access, calls, arithmetic, comparisons, `&&`, `||`, `!`, safe lookup (`??`), and Elvis defaults (`?:`)
- `{#if}`, `{#for}`/`{#each}`, `{#let}`/`{#set}`, and `{#when}`/`{#switch}` sections
- includes, insert blocks, layouts, fragments, isolated tag templates, and include-cycle detection
- comments (`{! ... !}`), unparsed blocks (`{| ... |}`), and parameter declarations (`{@Type name}`)
- asynchronous value and namespace resolvers with deterministic priorities
- media-aware HTML, XML, and JSON string escaping with explicit `SafeHtml`, `SafeXml`, and `SafeJsonString` wrappers
- media variants such as `invoice.html` and `invoice.json`, plus locale variants such as `invoice.fr.html`
- locale-aware message namespaces with deterministic exact-language, base-language, and default fallbacks

Runtime templates use exactly the same parser and evaluator:

```rust
# use radiant::Engine;
# async fn example() -> Result<(), radiant::RenderError> {
let engine = Engine::builder()
    .template("greeting.txt", "Hello {name ?: 'world'}")
    .build()?;
let output = engine.template("greeting").await?
    .data("name", "Mina")
    .render()
    .await?;
# Ok(())
# }
```

Use `FileLoader` for templates loaded on demand. `Engine::reload` and `Engine::replace` provide explicit development-time refresh without a hidden watcher or global cache.

Enable the `serde` feature to convert intentionally dynamic data with `Value::from_serialize`. Serde isn't required for checked templates or the core value model.

## Security model

Rendering is strict by default: unresolved values are errors unless made safe with `??` or handled with `?:`. Template IDs cannot be absolute or escape their configured root. Engines cap include depth and output size and report structured errors with source spans and render stacks.

For user-authored templates, start with `EngineBuilder::restricted()`. It disables includes and tag invocation, permits only data-oriented sections and the `data:` namespace, and lowers the output limit. Additional sections and namespaces must be opted into explicitly.

## Validation

```console
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```
