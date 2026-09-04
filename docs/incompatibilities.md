# Qute incompatibilities

Radiant implements Qute's core template model, but it is not a Quarkus or Java
compatibility layer. This page lists the remaining differences that are
intentional, require an application-level facility, or would compromise
Radiant's explicit and predictable Rust execution model.

## Language and parsing

| Qute feature | Radiant behaviour | Migration |
| --- | --- | --- |
| Java numeric suffixes and Java/`BigDecimal` coercion | Numbers are `i64` or `f64`, with checked integer operations and Rust-oriented coercion. | Remove `L`, `F`, `D`, `BI` and `BD` suffixes. Convert decimal values in application code or expose a resolver-backed decimal type. |
| Numeric dot indexing (`items.0`) | Indexing uses `items[0]`. A number after `.` is not an identifier. | Use bracket indexing. |
| Arbitrary virtual-method infix syntax | Radiant supports symbolic operators, Qute's unambiguous textual comparison aliases, and ordinary calls. General infix calls are omitted because they make tokenization dependent on the registered resolver set. | Write `value.method(argument)` or register a normal value resolver. |
| `or` with context-dependent boolean/Elvis meaning | Radiant keeps `||` for boolean OR and `?:` for null/not-found recovery. | Replace `or` with `||` or `?:` according to intent. |
| Ternary expressions and `orEmpty` | Radiant has no ternary node. It uses `{#if}` for branching and `??`/`?:` for explicit recovery. | Use an `if` section; use `value??` or `value?:fallback` for missing values. |
| Permissive non-whitespace identifiers | Identifiers use a Rust-like ASCII start and Unicode-alphanumeric continuation. This keeps operators and member access unambiguous. | Use bracket lookup for arbitrary map keys. |
| Optional closing tags for `let`, `set`, and `include` | Sections close explicitly or are self-closing. Parent-boundary auto-closing is intentionally omitted because local edits can otherwise change a distant section's extent. | Add the matching closing tag. |
| Automatic standalone-line removal | Source whitespace is preserved. Implicit line removal makes byte output depend on whether a line happens to contain only syntax. | Format the template for the desired output or use an application source preprocessor. |
| Advanced `when` predicates | `when`/`switch` cases use equality. Ordering, membership, not-in, and enum-like raw constants are not a second expression language in Radiant. | Use an `if`/`else if` chain with ordinary expressions. |

## Composition and extension points

| Qute feature | Radiant behaviour | Migration |
| --- | --- | --- |
| `frg:`/`fragment:` and `cap:`/`capture:` expression namespaces | Fragments are rendered with `{#include template$fragment /}`, `{#include $fragment /}`, or `DynamicTemplate::fragment()`. Expression namespaces would require nested rendering to materialize a trusted string during expression evaluation. | Use fragment include syntax, which writes directly to the current output sink and preserves media escaping. |
| `_ignoreFragments` | `$` is reserved as the fragment separator in include IDs. | Avoid `$` in template IDs. |
| Configurable loop metadata prefixes | Metadata uses the alias prefix, for example `item_index` and `item_odd`. A global mutable naming policy is omitted. | Rename the loop alias or update the metadata references. |
| Lazy iterators, streams, and arbitrary async iterables as loop values | `Value` owns finite sequences and maps; positive integers are also iterable. Materializing at the model boundary makes repeat rendering deterministic and keeps lifetimes out of the evaluator. | Collect into `Vec<T>`/`Value::Sequence`, or fetch and paginate in application code. |
| Runtime `eval` | Templates are parsed when registered or embedded. Runtime source evaluation is omitted because it defeats graph validation, adds parsing to the render path, and broadens the injection surface. | Register a template explicitly, or select a pre-registered template by ID. |
| Cached sections | Radiant does not own an application cache or cache rendered, media-dependent output. | Cache application data or the final `Rendered` value at the application boundary. |
| Custom parser/section helper factories | Sections have compiler-known AST and scope rules. Runtime hooks are limited to async value and namespace resolvers. | Implement data behaviour as a resolver; compose rendering with includes and user tags. |
| Universal `.raw`/`.safe` strings | Trusted output is media-specific: `SafeHtml`, `SafeXml`, or `SafeJsonString`. This prevents a value trusted for one grammar from bypassing another grammar's escaping. | Construct the appropriate trusted wrapper in audited Rust code. |

## Checked templates and data

| Qute feature | Radiant behaviour | Migration |
| --- | --- | --- |
| Reflection, generated Java member resolvers, Jandex, and CDI model discovery | Rust values are exposed through `TemplateValue`, `IntoValue`, or an explicit resolver. | Derive `TemplateValue` or register a resolver. |
| Complete Qute-style type checking for every expression | Radiant directly compiles simple field output, conditions, and loops. More complex expressions are parsed at compile time but use the dynamic evaluator; missing members can therefore remain render-time errors. | Keep complex logic in typed Rust data preparation. Test dynamic portions. |
| Java type names in `{@Type name}` declarations | Declarations are schema metadata and defaults; they do not resolve Java or Rust type names. Rust types come from the checked template struct. | Define types on the Rust struct and use declarations only for template documentation/defaults. |
| Generated typed fragment methods | Fragment selection is currently dynamic, including for an embedded checked graph. | Create a small checked wrapper template that includes the fragment. |
| Transparent deferred/future values | Futures are awaited only through async resolvers. Storing futures in cloneable `Value` would make ownership, cancellation, memoization, and repeat rendering implicit. | Await business data before rendering or expose a deliberate async resolver. |
| A single opaque root object | Dynamic instances use a named root map. This keeps data exposure explicit and makes `data:` stable. | Bind the object under a name or convert it to `Value::Map`. |

## Runtime and Quarkus integration

| Qute feature | Radiant behaviour | Migration |
| --- | --- | --- |
| True chunked output streaming | `radiant-axum::stream()` currently defers work but yields one buffered chunk. The evaluator enforces escaping and output limits on a `String` sink. | Use buffered rendering. Apply backpressure while sending the completed body if transport chunking is required. |
| Full Qute message templates and generated message bundles | `MessageBundle` is a lightweight positional formatter using `{0}` placeholders. | Render a normal checked template for named, conditional, or structured localized messages. |
| CDI/inject, generated static/enum, global, config, string, and time namespaces | Core owns only `data:` and fragment composition syntax. Applications register explicit namespace resolvers. | Register a small resolver for application configuration or domain utilities. Dependency injection remains application-owned. |
| Arbitrary media types and encodings | Built-in variants are UTF-8 HTML, text, JSON, and XML so escaping remains a closed enum. | Render text and set a custom response content type in the integration layer when no special escaping is needed. |
| Byte-identical HTML apostrophe entities | Radiant may emit a different standards-equivalent entity spelling. | Compare parsed/decoded output rather than entity spelling unless byte identity is a protocol requirement. |
| Automatic classpath discovery, production file watching, and global registration | Checked graphs are embedded; dynamic templates use explicit registration, loaders, and reload. This avoids ambient global state and production filesystem polling. | Drive `Engine::reload()` from development tooling. |
| Engine-owned render timeouts | Cancellation and deadlines belong to the async caller, avoiding an extra timer and policy in every render. | Wrap the render future with the runtime or web framework timeout facility. |
| Quarkus debug mode and Jakarta REST integration | Diagnostics and integrations use Rust APIs and framework crates such as `radiant-axum`. | Use `RenderError` source locations and the integration crate for the selected framework. |

These choices do not prevent adding opt-in integration crates. They keep the
core renderer free of reflection, ambient discovery, blocking I/O, hidden
allocation policies, and media-unsafe escape bypasses.
