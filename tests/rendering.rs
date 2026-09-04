use std::collections::BTreeMap;

use futures_executor::block_on;
use futures_util::FutureExt;
use radiant::{
    BoxFuture, Engine, ErrorCode, MediaType, MessageBundle, NamespaceContext, NamespaceResolver,
    Resolution, SafeHtml, Template, TemplateValue, Value,
};

#[derive(TemplateValue)]
struct Item {
    name: String,
    price: i64,
}

#[derive(Template)]
#[template(path = "checked.html")]
struct CheckedPage<'a> {
    title: &'a str,
    items: &'a [Item],
}

#[derive(Template)]
#[template(path = "checked-fragment.html")]
struct CheckedFragment<'a> {
    title: &'a str,
}

#[derive(Template)]
#[template(path = "checked-condition.html")]
struct CheckedCondition {
    first: bool,
    second: bool,
}

#[derive(Template)]
#[template(path = "checked-self-fragment.html")]
struct CheckedSelfFragment<'a> {
    title: &'a str,
}

#[test]
fn checked_template_renders_typed_data_and_escapes_html() {
    let items = vec![Item {
        name: "<Sword>".into(),
        price: 10,
    }];
    let rendered = block_on(Engine::new().unwrap().render(CheckedPage {
        title: "Tools & weapons",
        items: &items,
    }))
    .unwrap();

    assert_eq!(rendered.media_type(), MediaType::Html);
    assert_eq!(
        rendered.to_string(),
        "<h1>Tools &amp; weapons</h1>\n\n<p>&lt;Sword&gt;: 10</p>\n\n"
    );
}

#[test]
fn checked_template_can_append_to_a_reusable_buffer() {
    let engine = Engine::new().unwrap();
    let items = vec![Item {
        name: "<Hammer>".into(),
        price: 12,
    }];
    let mut output = String::from("prefix:");

    let media_type = block_on(engine.render_into(
        CheckedPage {
            title: "Tools",
            items: &items,
        },
        &mut output,
    ))
    .unwrap();

    assert_eq!(media_type, MediaType::Html);
    assert_eq!(
        output,
        "prefix:<h1>Tools</h1>\n\n<p>&lt;Hammer&gt;: 12</p>\n\n"
    );
}

#[test]
fn checked_template_graphs_embed_fragment_dependencies() {
    let rendered = block_on(
        Engine::new()
            .unwrap()
            .render(CheckedFragment { title: "Checked" }),
    )
    .unwrap();

    assert_eq!(rendered.to_string(), "<b>Checked</b>\n");
}

#[test]
fn dynamic_sections_preserve_scope_and_loop_metadata() {
    let engine = Engine::builder()
        .template(
            "items.txt",
            "{#for item in items}{item_count}:{item.name};{#else}empty{/for}|{data:label}|{#let value = 'spaced'}{value}{/let}",
        )
        .build()
        .unwrap();
    let items = Value::Sequence(vec![Value::Map(BTreeMap::from([(
        "name".into(),
        Value::String("A".into()),
    )]))]);

    let template = block_on(engine.template("items")).unwrap();
    let rendered = block_on(template.data("items", items).data("label", "root").render()).unwrap();

    assert_eq!(rendered.to_string(), "1:A;|root|spaced");
}

#[test]
fn else_if_blocks_select_the_first_truthy_condition() {
    let engine = Engine::builder()
        .template(
            "condition.txt",
            "{#if first}first{#else if second}second{#else if third}third{#else}last{/if}",
        )
        .build()
        .unwrap();
    let template = block_on(engine.template("condition.txt")).unwrap();

    let rendered = block_on(
        template
            .data("first", false)
            .data("second", false)
            .data("third", true)
            .render(),
    )
    .unwrap();

    assert_eq!(rendered.to_string(), "third");

    let rendered = block_on(Engine::new().unwrap().render(CheckedCondition {
        first: false,
        second: true,
    }))
    .unwrap();
    assert_eq!(rendered.to_string(), "second\n");
}

#[test]
fn safe_and_default_expressions_are_explicit() {
    let engine = Engine::builder()
        .template(
            "safe.html",
            "{missing??}:{missing ?: 'fallback'}:{missing[0]??}:{missing.call()??}:{trusted}",
        )
        .build()
        .unwrap();
    let template = block_on(engine.template("safe.html")).unwrap();
    let rendered = block_on(
        template
            .data("trusted", SafeHtml::new("<strong>safe</strong>"))
            .render(),
    )
    .unwrap();
    assert_eq!(rendered.to_string(), ":fallback:::<strong>safe</strong>");

    let strict = Engine::builder()
        .template("strict.txt", "before {missing} after")
        .build()
        .unwrap();
    let template = block_on(strict.template("strict.txt")).unwrap();
    let error = block_on(template.instance().render()).unwrap_err();
    assert_eq!(error.code, ErrorCode::MissingValue);
    assert_eq!(error.line, Some(1));
}

#[test]
fn layouts_and_fragments_compose_without_global_state() {
    let engine = Engine::builder()
        .template(
            "base.html",
            "<title>{#insert title}Default{/insert}</title><main>{#insert}Empty{/insert}</main>",
        )
        .template(
            "page.html",
            "{#include base.html}{#title}{title}{/title}<p>{message}</p>{/include}",
        )
        .template(
            "parts.html",
            "before{#fragment card}<b>{text}</b>{/fragment}after",
        )
        .template("fragment.html", "{#include parts.html$card text='Hi' /}")
        .template(
            "self-fragment.html",
            "{#fragment card rendered=false}<i>{text}</i>{/fragment}{#include $card text='Self' /}",
        )
        .build()
        .unwrap();

    let template = block_on(engine.template("page.html")).unwrap();
    let page = block_on(
        template
            .data("title", "Custom")
            .data("message", "Body")
            .render(),
    )
    .unwrap();
    assert_eq!(
        page.to_string(),
        "<title>Custom</title><main><p>Body</p></main>"
    );

    let template = block_on(engine.template("fragment.html")).unwrap();
    let fragment = block_on(template.instance().render()).unwrap();
    assert_eq!(fragment.to_string(), "<b>Hi</b>");

    let template = block_on(engine.template("parts.html"))
        .unwrap()
        .fragment("card")
        .unwrap();
    let fragment = block_on(template.data("text", "Direct").render()).unwrap();
    assert_eq!(fragment.to_string(), "<b>Direct</b>");

    let template = block_on(engine.template("self-fragment.html")).unwrap();
    let rendered = block_on(template.instance().render()).unwrap();
    assert_eq!(rendered.to_string(), "<i>Self</i>");

    let rendered = block_on(Engine::new().unwrap().render(CheckedSelfFragment {
        title: "Checked self",
    }))
    .unwrap();
    assert_eq!(rendered.to_string(), "<b>Checked self</b>\n");
}

#[test]
fn user_tags_bind_positional_and_named_arguments() {
    let engine = Engine::builder()
        .template(
            "tags/item.html",
            "{it}:{item}:{label}:{_args.size}:{_args.item}:{_args.label}",
        )
        .template("page.html", "{#item item label='detail' /}")
        .build()
        .unwrap();

    let rendered = block_on(
        block_on(engine.template("page.html"))
            .unwrap()
            .data("item", "chair")
            .render(),
    )
    .unwrap();
    assert_eq!(rendered.to_string(), "chair:chair:detail:2:chair:detail");
}

#[test]
fn fragment_visibility_and_capture_match_qute() {
    let engine = Engine::builder()
        .template(
            "visibility.txt",
            "{#fragment dynamic rendered=show}D{/fragment}{#fragment marker _hidden}M{/fragment}{#capture captured}C{/capture}|{#include $dynamic /}{#include $marker /}{#include $captured /}",
        )
        .build()
        .unwrap();

    let hidden = block_on(engine.template("visibility.txt"))
        .unwrap()
        .data("show", false);
    assert_eq!(block_on(hidden.render()).unwrap().to_string(), "|DMC");

    let visible = block_on(engine.template("visibility.txt"))
        .unwrap()
        .data("show", true);
    assert_eq!(block_on(visible.render()).unwrap().to_string(), "D|DMC");

    let captured = block_on(engine.template("visibility.txt"))
        .unwrap()
        .fragment("captured")
        .unwrap();
    assert_eq!(
        block_on(captured.instance().render()).unwrap().to_string(),
        "C"
    );
}

#[test]
fn recursive_fragment_includes_are_rejected() {
    let engine = Engine::builder()
        .template(
            "recursive.html",
            "{#fragment loop}{#include recursive.html$loop /}{/fragment}",
        )
        .build()
        .unwrap();
    let template = block_on(engine.template("recursive.html"))
        .unwrap()
        .fragment("loop")
        .unwrap();

    let error = block_on(template.instance().render()).unwrap_err();
    assert_eq!(error.code, ErrorCode::IncludeCycle);
}

struct Greeting;

impl NamespaceResolver for Greeting {
    fn namespace(&self) -> &str {
        "greet"
    }

    fn resolve<'a>(
        &'a self,
        context: NamespaceContext<'a>,
    ) -> BoxFuture<'a, Result<Resolution<Value>, radiant::RenderError>> {
        async move {
            if context.name == "hello" {
                let target = match context.arguments.first() {
                    Some(Value::String(value)) => value.clone(),
                    _ => "world".into(),
                };
                Ok(Resolution::Value(Value::String(format!("Hello {target}"))))
            } else {
                Ok(Resolution::NotFound)
            }
        }
        .boxed()
    }
}

#[test]
fn async_namespaces_use_ordered_explicit_extensions() {
    let engine = Engine::builder()
        .namespace_resolver(Greeting)
        .template("hello.txt", "{greet:hello(name)}")
        .build()
        .unwrap();
    let template = block_on(engine.template("hello.txt")).unwrap();
    let rendered = block_on(template.data("name", "Mina").render()).unwrap();
    assert_eq!(rendered.to_string(), "Hello Mina");
}

#[test]
fn output_limits_and_template_replacement_are_enforced() {
    let engine = Engine::builder()
        .max_output_bytes(3)
        .template("value.txt", "four")
        .build()
        .unwrap();
    let template = block_on(engine.template("value.txt")).unwrap();
    let error = block_on(template.instance().render()).unwrap_err();
    assert_eq!(error.code, ErrorCode::OutputLimit);

    engine.replace("value.txt", "ok").unwrap();
    let template = block_on(engine.template("value.txt")).unwrap();
    let rendered = block_on(template.instance().render()).unwrap();
    assert_eq!(rendered.to_string(), "ok");
}

#[test]
fn variants_and_restricted_templates_fail_closed() {
    let engine = Engine::builder()
        .template("page.html", "html")
        .build()
        .unwrap();
    let template = block_on(engine.template("page")).unwrap();
    let error = block_on(template.instance().variant(MediaType::Json).render()).unwrap_err();
    assert_eq!(error.code, ErrorCode::NotAcceptable);

    let error = Engine::builder()
        .restricted()
        .template("unsafe.txt", "{#include secret /}")
        .build()
        .err()
        .expect("restricted engines reject includes");
    assert_eq!(error.code, ErrorCode::UnknownSection);
}

#[test]
fn localized_messages_follow_render_locale_with_fallback() {
    let messages = MessageBundle::builder("msg")
        .default_locale("en")
        .message("en", "hello", "Hello {0} — use {{braces}}")
        .message("fr", "hello", "Bonjour {0} — utilisez {{accolades}}")
        .build()
        .unwrap();
    let engine = Engine::builder()
        .namespace_resolver(messages)
        .template("message.txt", "{msg:hello(name)}")
        .build()
        .unwrap();
    let template = block_on(engine.template("message.txt")).unwrap();
    let rendered = block_on(template.data("name", "Mina").locale("fr-CA").render()).unwrap();
    assert_eq!(rendered.to_string(), "Bonjour Mina — utilisez {accolades}");
}

#[test]
fn locale_variants_apply_to_root_templates_and_includes() {
    let engine = Engine::builder()
        .template("page.html", "default: {#include greeting /}")
        .template("page.fr.html", "français: {#include greeting /}")
        .template("greeting.html", "hello")
        .template("greeting.fr.html", "bonjour")
        .build()
        .unwrap();

    let template = block_on(engine.template("page")).unwrap();
    let rendered = block_on(template.instance().locale("fr-CA").render()).unwrap();

    assert_eq!(rendered.to_string(), "français: bonjour");
}

#[test]
fn checked_templates_can_be_registered_concurrently() {
    let engine = Engine::new().unwrap();
    let threads = (0..8)
        .map(|_| {
            let engine = engine.clone();
            std::thread::spawn(move || {
                block_on(engine.render(CheckedPage {
                    title: "Concurrent",
                    items: &[],
                }))
                .map(|rendered| rendered.to_string())
            })
        })
        .collect::<Vec<_>>();

    for thread in threads {
        assert!(thread.join().unwrap().unwrap().contains("Concurrent"));
    }
}

#[cfg(feature = "serde")]
#[test]
fn serde_values_are_an_explicit_dynamic_adapter() {
    let value = Value::from_serialize(&serde_json::json!({
        "name": "Mina",
        "roles": ["admin", "author"]
    }))
    .unwrap();

    assert!(matches!(value, Value::Map(values) if values["name"] == Value::String("Mina".into())));
}
