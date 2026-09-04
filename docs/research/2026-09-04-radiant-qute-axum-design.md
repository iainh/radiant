# Research: Qute ideas in idiomatic Rust and Axum

**Date**: 2026-09-04
**Question**: What should a Rust implementation of Quarkus Qute look like when Axum is the primary web framework, while retaining the strengths of Qute, Rust, and the conventions established by `mp-config` and `catnap`?
**Status**: Complete

## Context

Radiant currently contains only a new Rust 2024 binary skeleton. There is no existing API or compatibility constraint.

`mp-config` and `catnap` establish a consistent porting philosophy:

- preserve useful MicroProfile or Quarkus concepts and vocabulary;
- replace CDI, reflection, service loading, and runtime annotation processing with explicit builders, traits, ownership, and procedural macros;
- move declaration errors to compile time;
- generate normal Rust against a small runtime API;
- use ecosystem crates instead of recreating Java infrastructure;
- retain a lower-level dynamic escape hatch.

Radiant should be a semantic port of Qute, not a transliteration of its Java classes.

## Recommendation

Build Radiant as an **async-capable, strict, external-template engine with two first-class paths**:

1. **Checked templates** are Rust structs derived with `Template`. The macro reads a Qute-syntax file, validates its template graph, and emits typed rendering code. Rustc checks model field, method, operator, and extension use. This is the default application path.
2. **Dynamic templates** are parsed to an intermediate representation (IR) by the runtime engine and rendered through a `Value`/`TemplateValue` resolver model. This supports user-provided templates, runtime loaders, and development hot reload, with runtime validation.

Both paths must use the same parser, expression semantics, escaping rules, built-in sections, diagnostics model, and conformance suite. They may have different lowerings, but must not become different template languages.

Keep Axum in a separate adapter crate. Applications own a cheap-to-clone `Engine` handle in Axum state. An Axum `Renderer` extractor combines that engine with request preferences and returns an already rendered response. Buffered rendering is the safe default; streaming is explicit.

```diagram
                         compile time
  templates ───────────▶ parser + semantic analyser ───────▶ typed Rust renderer
      │                         │                                  │
      │                         └──────── diagnostics              │
      │                                                            │
      └──── loader / reload ──▶ dynamic IR ──▶ async evaluator ◀────┘
                                                │
                                  escaping + output writer
                                                │
                            ┌───────────────────┴──────────────────┐
                            │                                      │
                      String / bytes                         byte stream
                            │                                      │
                      Axum response                       opt-in Axum body
```

## Preserve, adapt, and omit

| Qute idea | Radiant form | Decision |
|---|---|---|
| `Engine` | Immutable, cloneable `Engine` backed by `Arc` | Preserve |
| `Template` | Parsed/compiled template definition | Preserve |
| `TemplateInstance` | Owned or borrowing `Render`/typed template value | Adapt; avoid a mutable Java-style bag |
| `@CheckedTemplate` | `#[derive(Template)]` on a Rust context struct | Preserve semantically |
| `@TemplateData` | `#[derive(TemplateValue)]` for dynamic exposure | Preserve semantically |
| Value and namespace resolvers | Explicit ordered traits/functions registered on `EngineBuilder` | Preserve |
| `NotFound` distinct from `null` | `Resolution::NotFound` versus `Value::Null` | Preserve exactly |
| Sections and lexical child scopes | Typed AST/IR nodes with section plug-in boundary | Preserve |
| Include/insert layouts and fragments | Checked template graph and generated fragment entry points | Preserve |
| Content-type variants | `Variant` using HTTP media type and language preferences | Preserve and make HTTP-native |
| Async expression resolution | Rust `Future`; cancellation by dropping the render future | Preserve |
| Reactive `Uni`/`Multi` | `Future` and `Stream<Item = Result<Bytes>>` | Replace |
| CDI/inject namespace | Explicit state, request context, or narrow namespace values | Omit |
| Reflection fallback | Derived accessors or explicit dynamic values | Omit by default |
| Jandex/build items/bytecode transforms | Proc macros, Cargo dependency tracking, and generated Rust | Replace |
| Generic `RawString` | Context-specific trusted output wrappers | Improve |
| Silent missing properties | Strict errors by default; explicit optional/default operators | Preserve strict Qute default |

## Proposed crate boundaries

```text
radiant/                 Public engine, values, rendering traits, errors, macro re-exports
radiant-compiler/        Parser, source spans, AST, semantic IR, template graph, diagnostics
radiant-macros/          Template and TemplateValue derives; depends on compiler
radiant-axum/            State/extractor/response integration; depends on radiant and Axum
```

`radiant-compiler` avoids a dependency cycle: both the runtime and proc macro need the parser, while generated code refers only to `::radiant`. It may remain an implementation-detail crate initially.

Do not add a `build.rs` requirement for ordinary use. The derive macro should use `include_bytes!` or equivalent references for every transitive static include so Cargo tracks changes. An explicit compiler CLI/build helper can come later for whole-project checks, editor integration, or projects that use only dynamic templates.

Optional ecosystem integrations belong behind features or in adapter crates:

- `serde` for intentionally dynamic contexts;
- `axum` in `radiant-axum`;
- `notify` for development reload;
- `tracing` instrumentation;
- localization/message bundles after the core language stabilizes.

## Public programming model

### Checked templates

Use a Rust struct as the template signature. This is closer to Qute template records and Askama than to Qute's generated `static native` methods.

```rust
use radiant::Template;

#[derive(Template)]
#[template(path = "items/detail.html")]
struct ItemPage<'a> {
    item: &'a Item,
    current_user: &'a User,
}
```

The checked path should guarantee at compile time:

- the file and every static include exist;
- syntax, section parameters, blocks, fragments, and layout slots are valid;
- static include cycles do not exist;
- references resolve against the context and lexical locals;
- field/method/operator/function use type-checks as generated Rust;
- all required fragment and tag arguments are supplied;
- the selected output context has a valid escaping strategy.

The macro should emit span-aware `syn::Error`s for declaration problems and generated Rust for type checking, following `mp-config` and `catnap`. Stable proc macros cannot introspect arbitrary nested Rust types. Radiant should not claim otherwise: exact model checking must come from generated expressions checked by rustc, as Askama does.

Types intended for the dynamic path can opt in separately:

```rust
#[derive(radiant::TemplateValue)]
#[template_value(rename_all = "camelCase")]
struct Item {
    name: String,
    price: Decimal,
    #[template_value(skip)]
    internal_cost: Decimal,
}
```

Only explicitly exposed fields and methods should be visible. Rust privacy remains meaningful; there is no universal reflection fallback.

### Framework-neutral rendering

The normal API should be small:

```rust
let output = engine.render(ItemPage {
    item: &item,
    current_user: &user,
}).await?;
```

`Rendered` owns bytes and metadata such as media type, language, and charset. `Engine::render()` is async even when a particular template has no async values. This leaves one composable API and permits generated renderers, sections, and extension functions to await values without blocking an executor.

For customization, return a builder before execution rather than mutating the template model:

```rust
let output = engine
    .render_with(ItemPage { item: &item, current_user: &user })
    .locale(locale)
    .variant(MediaType::HTML)
    .render()
    .await?;
```

Dynamic templates retain the Qute workflow as an escape hatch:

```rust
let template = engine.template("mail/welcome").await?;
let output = template
    .data("user", user)?
    .data("activation_url", url)?
    .render()
    .await?;
```

Typed and dynamic APIs should produce the same `Rendered` and `RenderError` types.

### Axum integration

The engine belongs in application state, not request extensions:

```rust
#[derive(Clone, axum::extract::FromRef)]
struct AppState {
    db: Database,
    templates: radiant::Engine,
}
```

`radiant_axum::Renderer` should implement `FromRequestParts<S>` when `Engine: FromRef<S>`. It can read `Accept` and `Accept-Language`, plus optional request extensions for a locale, tenant, Content Security Policy nonce, or render deadline.

```rust
use radiant_axum::{RenderRejection, Renderer, TemplateResponse};

async fn item(
    renderer: Renderer,
    State(app): State<AppState>,
    Path(id): Path<ItemId>,
) -> Result<TemplateResponse, RenderRejection> {
    let item = app.db.item(id).await?;
    renderer.render(ItemPage { item: &item, current_user: &item.owner }).await
}
```

Exact application-error conversion remains application-owned; the example above would normally use an application error enum that converts both database and rendering errors.

`IntoResponse::into_response()` is synchronous. Therefore `TemplateResponse` should contain completed bytes and HTTP metadata, not start hidden async work. `RenderRejection` should log/source-chain the detailed error and send a sanitized `500` response by default.

Streaming should be an explicit alternative:

```rust
renderer.stream(ItemPage { /* ... */ })
```

Its response body wraps a `Stream<Item = Result<Bytes, RenderError>>`. Document the trade-off: once headers or chunks are sent, a later render failure can only terminate the body; it cannot turn the response into a clean `500`. Buffered rendering should therefore be the default for pages and error-sensitive responses.

## Template language

Keep Qute syntax where it has clear semantics and helps migration:

```html
{#include base}
  {#title}{item.name}{/title}
  {#for tag in item.tags}
    <span>{tag}</span>
  {#else}
    <span>No tags</span>
  {/for}
{/include}
```

Initial built-ins should be deliberately smaller than all of Qute:

- output expressions, comments, and unparsed text;
- `if`/`else`, `for`/`else`, `let`, and `when`;
- `include` and `insert` for layouts;
- isolated user tags;
- fragments, including hidden fragments for htmx-style responses;
- `??` and `?:` for explicit missing/null handling;
- boolean, comparison, arithmetic, indexing, and member access expressions.

Preserve these Qute scope rules:

- the first unqualified name may search lexical parent scopes;
- subsequent members resolve only against the preceding value;
- `data:` reaches root render data when a local shadows it;
- includes inherit caller scope unless marked isolated;
- user tags are isolated by default;
- locals do not escape their section;
- static dependencies are checked at compile time.

Make include isolation visible and recommend it for reusable components. Implicit parent lookup is convenient but makes dependencies harder to audit.

Rust names should remain Rust names by default (`current_user`, not `currentUser`). `TemplateValue` can support explicit rename rules for migration and serialization interoperability.

Do not implement every Java collection alias or numeric coercion. Rust should use predictable types:

- `Option::None` maps to `Null`, not `NotFound`;
- missing fields/keys map to `NotFound`;
- integer and floating-point operations reject lossy or ambiguous coercions;
- maps, sequences, strings, booleans, and numbers expose a documented small set of operations;
- `Display` controls textual formatting only, not property reflection.

## Values and resolvers

Qute's most important resolver invariant must remain explicit:

```rust
pub enum Resolution<T> {
    Value(T), // T may be Value::Null
    NotFound, // try the next applicable resolver
}
```

The dynamic value model should support borrowed values for normal request rendering and owned/`Arc` values for deferred or cached work. A likely shape is `Value<'a>` with scalar values, `Cow<'a, str>`, borrowed/owned sequences and maps, an opaque `TemplateValue` object, and context-specific safe output.

Advanced runtime extension traits need object-safe async methods, likely returning `BoxFuture`:

```rust
pub trait ValueResolver: Send + Sync {
    fn priority(&self) -> i32 { 0 }
    fn resolve<'a>(&'a self, context: EvalContext<'a>)
        -> BoxFuture<'a, Result<Resolution<Value<'a>>, RenderError>>;
}
```

Sort resolvers once when the engine is built. Cache successful access plans only when the base type and resolver declare that caching is safe.

Common extension functions should have typed closure adapters rather than requiring users to implement this low-level trait:

```rust
let engine = Engine::builder()
    .namespace_fn("money", "format", format_money)
    .async_namespace_fn("avatar", "url", avatar_url)
    .build()?;
```

Registration stays explicit. There should be no `inventory`, linker registration, service loader, or global mutable registry. For checked templates, extension declarations must also be visible to the macro/code generator; a runtime-only registration cannot honestly promise compile-time checking. The exact declaration mechanism needs a prototype before the API is fixed.

Async resolvers are useful for deferred values, localization, and narrow request services, but documentation should discourage database or network access hidden inside templates. Handlers should normally fetch business data first. This avoids invisible N+1 queries and keeps latency ownership clear.

## Rendering and concurrency

Use async as the abstraction, not as permission for uncontrolled parallel evaluation.

- Evaluate nodes in deterministic output order by default.
- Await expression parts sequentially because each depends on the prior value.
- Preserve short-circuiting for `&&`, `||`, defaults, and conditional sections.
- Do not concurrently evaluate arbitrary sibling expressions with possible effects.
- Propagate cancellation by making all work owned by the render future/stream; dropping it stops further evaluation.
- Keep timeout policy outside the framework-neutral core. `radiant-axum` can apply a Tokio deadline from request/application policy.
- Never hold a synchronous lock across `.await`.

Generated checked renderers can write directly to the selected output sink. The dynamic evaluator should interpret compiled IR, not repeatedly walk parser objects. Both should support buffered output; the dynamic evaluator can additionally yield chunks with backpressure.

## Escaping and trusted output

Qute's content-type-based escaping is worth preserving, but a single generic `RawString` is too broad.

Start with:

- automatic HTML/XML text escaping for HTML/XML variants;
- JSON string escaping for JSON variants;
- no escaping for plain text;
- explicit `SafeHtml`, `SafeXml`, and `SafeJsonString` wrappers;
- no blanket “safe everywhere” value.

The compiler should track output context in HTML templates and eventually distinguish text, quoted attribute, URL, JavaScript, and CSS contexts. Until those contexts are implemented safely, reject or conservatively escape ambiguous interpolation sites. A value trusted as HTML text must not automatically be trusted as a URL or script.

Raw output should require a Rust-side trusted wrapper or a visibly unsafe template operation. Checked templates should not make `.raw` available on every object.

## Variants and negotiation

A template ID may have multiple variants, for example:

```text
items/detail.html
items/detail.txt
items/detail.fr.html
```

Model variants using established HTTP types where possible:

```rust
struct Variant {
    media_type: mime::Mime,
    language: Option<LanguageTag>,
    charset: Charset,
}
```

The engine owns variant metadata and escaping selection. The Axum adapter owns negotiation from request headers. An explicit render option overrides negotiated values. Variant selection failure should become `406 Not Acceptable`; template load/render failures should become `500`.

Do not put Axum request types in the core engine.

## Errors and diagnostics

Use one structured error family, with stable categories rather than message matching:

- source/load and duplicate-template errors;
- lex/parse errors;
- template graph cycles or missing static dependencies;
- checked type/expression errors;
- missing dynamic values and namespace failures;
- extension/section failures;
- output and cancellation/deadline failures.

Every error should retain:

- template ID and source path/URI;
- byte span plus line/column;
- expression or section text;
- include/layout/fragment render stack;
- resolver attempts where relevant;
- underlying source error.

Aggregate independent compiler errors in one run. Format diagnostics with source snippets and labels, using `miette` or `codespan-reporting` if it fits without leaking those types into the stable API. Runtime errors should implement `std::error::Error`; HTTP clients must not receive internal source or model details.

Proc-macro compile-fail tests are part of the public API, as they are in `mp-config` and `catnap`.

## Caching and development reload

Use immutable compiled artifacts behind `Arc` and generation-stamped snapshots. In-flight requests retain their generation while a watcher atomically installs a new one.

Track template dependency edges so a changed include, layout, tag, or fragment invalidates reverse dependants. Do not clear the whole cache for every edit.

Checked generated renderers require recompilation when their source changes. Dynamic development mode can reparse changed templates without restarting the process, but loses compile-time model checking for edits made after compilation. Make that trade-off visible:

- **checked dev mode**: Cargo rebuild/restart, full Rust type checking;
- **live dev mode**: immediate dynamic IR reload, runtime schema checking;
- **production**: embedded checked renderers and templates by default; dynamic loaders opt in.

The parser and conformance suite must ensure all three modes agree on language semantics.

## Security boundaries

- Templates are trusted application code by default, not a sandbox.
- User-supplied templates require an explicit restricted engine with an allowlist of sections, functions, namespaces, loaders, recursion depth, output size, and execution deadline.
- Do not expose arbitrary application state or Axum request objects to templates.
- Request values such as user, locale, tenant, request ID, flash messages, and CSP nonce should be passed explicitly or through narrow request namespaces.
- Dynamic template IDs must be constrained by the loader; reject traversal and ambiguous aliases.
- Detect static recursion at compile time and cap dynamic include/eval recursion at runtime.

## Options considered

| Option | Pros | Cons | Decision |
|---|---|---|---|
| Askama-style generated renderers only | Best Rust type checking and speed; simple runtime | No immediate hot reload or truly dynamic templates; async additions are harder | Use for checked path, not exclusively |
| MiniJinja/Tera-style runtime VM only | Flexible loaders, reload, dynamic values, smaller code | Cannot honestly provide Rust-level compile-time model checking | Use for dynamic path, not exclusively |
| Serde JSON as the sole value model | Familiar and simple | Allocates/erases types, cannot expose borrowed methods or async values well | Offer as an adapter only |
| Hide rendering inside `IntoResponse` | Concise handlers | `IntoResponse` is synchronous; async failures and blocking become awkward | Reject |
| Stream every response | Low first-byte latency and bounded memory | Late failures cannot produce `500`; more complex testing and middleware behaviour | Explicit opt-in |
| Put engine in Axum `Extension` | Easy to add as middleware | Axum recommends `State` for global data; dynamic and failure-prone | Reject |
| Expose services through a CDI-like namespace | Qute familiarity | Hidden dependencies, hard testing, N+1 I/O, broad attack surface | Reject |
| One generic raw/safe string | Simple and Qute-compatible | Safety does not transfer between HTML, URL, JS, CSS, and JSON contexts | Reject |

## Suggested delivery sequence

### Phase 1: checked HTML core

- Workspace/crate boundaries and shared parser.
- Source spans, expressions, strict resolution, `if`, `for`, `let`.
- `Template` derive with external-file tracking.
- HTML/text rendering, `SafeHtml`, structured diagnostics.
- Buffered framework-neutral API and `radiant-axum` adapter.

This produces a useful engine before implementing Qute's full extension surface.

### Phase 2: composition

- Includes, insert/layout blocks, isolated tags, fragments.
- Whole-template graph validation and cycle errors.
- Generated checked fragment entry points.
- Variants and Axum content/language negotiation.

### Phase 3: dynamic and async extension model

- `TemplateValue`, `Value`, runtime IR evaluator, loaders, and strict dynamic errors.
- Ordered value/namespace resolvers and typed function adapters.
- Deferred/async values and explicit streaming.
- Dependency-aware development reload.

### Phase 4: ecosystem features

- Serde dynamic contexts, localization/message bundles, tracing, restricted untrusted engine.
- Compiler CLI/editor protocol.
- Contextual HTML escaping beyond text nodes.

Do not begin with caching sections, dynamic `eval`, debugger integration, or every Qute built-in alias. They add substantial policy or security surface without proving the core model.

## Decisions to validate with prototypes

1. **Checked extension functions**: determine the least repetitive way to make explicitly registered functions visible to proc-macro code generation without a hidden global registry.
2. **Borrowed async values**: prototype lifetimes for `Value<'a>`, resolver futures, loops, and streamed output before stabilizing public resolver traits.
3. **Generated async renderer size**: compare direct generated code with static IR plus generated typed accessors for compile time, binary size, and runtime throughput.
4. **Live-mode parity**: verify the dynamic evaluator can match checked rendering exactly for scoping, escaping, defaults, and includes.
5. **Template syntax compatibility**: decide whether migration compatibility requires all Qute aliases and standalone-line stripping or only the core syntax.

These are implementation choices, not reasons to weaken the recommended external model.

## References

- [Qute reference guide](https://quarkus.io/guides/qute-reference)
- [Qute core `Engine`](https://github.com/quarkusio/quarkus/blob/main/independent-projects/qute/core/src/main/java/io/quarkus/qute/Engine.java)
- [Qute `Template`](https://github.com/quarkusio/quarkus/blob/main/independent-projects/qute/core/src/main/java/io/quarkus/qute/Template.java)
- [Qute `TemplateInstance`](https://github.com/quarkusio/quarkus/blob/main/independent-projects/qute/core/src/main/java/io/quarkus/qute/TemplateInstance.java)
- [Qute evaluator](https://github.com/quarkusio/quarkus/blob/main/independent-projects/qute/core/src/main/java/io/quarkus/qute/EvaluatorImpl.java)
- [Quarkus Qute build-time processor](https://github.com/quarkusio/quarkus/blob/main/extensions/qute/deployment/src/main/java/io/quarkus/qute/deployment/QuteProcessor.java)
- [`mp-config`](https://github.com/iainh/mp-config)
- [`catnap`](https://github.com/iainh/catnap)
- [Axum state extractor](https://github.com/tokio-rs/axum/blob/main/axum/src/extract/state.rs)
- [Axum `IntoResponse`](https://docs.rs/axum/latest/axum/response/trait.IntoResponse.html)
- [Askama](https://github.com/askama-rs/askama)
- [MiniJinja](https://github.com/mitsuhiko/minijinja)
- [Tera](https://github.com/Keats/tera)
- [Maud](https://github.com/lambda-fairy/maud)
