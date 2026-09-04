use radiant_compiler::{ArgumentValue, BinaryOp, Expr, Node, parse};

#[test]
fn parses_nested_sections_and_expression_precedence() {
    let template = parse(
        "nested.html",
        "{@Vec<Item> items}\n{#if enabled && total > 1}{#for item in items}{item.price + 2 * 3}{#else}empty{/for}{#else}off{/if}",
    )
    .expect("representative template should parse");
    assert!(matches!(template.nodes[0], Node::Parameter(_)));
    let Node::Section(if_section) = &template.nodes[2] else {
        panic!("expected if section")
    };
    assert_eq!(if_section.blocks.len(), 2);
    let Node::Section(for_section) = &if_section.blocks[0].nodes[0] else {
        panic!("expected nested for")
    };
    assert_eq!(for_section.blocks.len(), 2);
    let Node::Output { expression, .. } = &for_section.blocks[0].nodes[0] else {
        panic!("expected output")
    };
    let Expr::Binary { op, right, .. } = expression else {
        panic!("expected addition")
    };
    assert_eq!(*op, BinaryOp::Add);
    assert!(matches!(
        **right,
        Expr::Binary {
            op: BinaryOp::Multiply,
            ..
        }
    ));
}

#[test]
fn parses_parameter_defaults() {
    let template = parse("params", "{@String name = fallback ?: 'guest'}{name}").unwrap();
    let Node::Parameter(parameter) = &template.nodes[0] else {
        panic!("expected parameter declaration")
    };
    assert_eq!(parameter.type_name, "String");
    assert_eq!(parameter.name, "name");
    assert!(parameter.default.is_some());
}

#[test]
fn preserves_comments_and_unparsed_content() {
    let template = parse("raw", "a{! hidden {x} !}{| {not.parsed} |}z").unwrap();
    assert!(matches!(&template.nodes[1], Node::Comment { value, .. } if value.contains("hidden")));
    assert!(
        matches!(&template.nodes[2], Node::Unparsed { value, .. } if value.contains("{not.parsed}"))
    );
}

#[test]
fn multi_pipe_unparsed_blocks_allow_shorter_delimiters() {
    let template = parse("raw", "{||||a|}b|||}c||||}").unwrap();

    assert!(matches!(&template.nodes[0], Node::Unparsed { value, .. } if value == "a|}b|||}c"));
}

#[test]
fn escaped_braces_are_literal_text() {
    let template = parse("braces", r"before \{name\} after {name}").unwrap();

    assert!(
        matches!(&template.nodes[0], Node::Text { value, .. } if value == "before {name} after ")
    );
    assert!(matches!(&template.nodes[1], Node::Output { .. }));
}

#[test]
fn only_qute_tag_starts_open_tags() {
    let source = r#"{"key":true} { name } {_name}"#;
    let template = parse("json", source).unwrap();

    assert!(
        matches!(&template.nodes[0], Node::Text { value, .. } if value == r#"{"key":true} { name } "#)
    );
    assert!(matches!(&template.nodes[1], Node::Output { .. }));
}

#[test]
fn collects_include_dependencies_and_layout_blocks() {
    let template = parse(
        "page",
        "{#include layouts/base title='Hello'}{#title}New title{/title}{#body}{#include partial/item /}{/body}{/include}",
    )
    .unwrap();
    assert_eq!(
        template.dependencies(),
        vec!["layouts/base", "partial/item"]
    );
    let Node::Section(include) = &template.nodes[0] else {
        panic!()
    };
    assert_eq!(include.blocks.len(), 3);
    assert!(matches!(include.arguments[0].value, ArgumentValue::Raw(_)));
}

#[test]
fn collects_user_tag_dependencies_separately_from_builtins() {
    let template = parse(
        "page",
        "{#if shown}{#card item /}{#else}{#include partial /}{/if}{#wrapper}{#nested-content /}{/wrapper}",
    )
    .unwrap();

    assert_eq!(template.dependencies(), vec!["partial"]);
    assert_eq!(
        template.tag_dependencies(),
        vec!["tags/card", "tags/wrapper"]
    );
}

#[test]
fn detects_duplicate_fragments_and_named_arguments() {
    let errors = parse(
        "duplicates",
        "{#fragment card}a{/fragment}{#fragment card}b{/fragment}{#include base x=1 x=2 /}",
    )
    .unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == "E_DUPLICATE_FRAGMENT")
    );
    assert!(
        errors
            .iter()
            .any(|error| error.code == "E_DUPLICATE_ARGUMENT")
    );
}

#[test]
fn malformed_input_has_precise_locations() {
    let errors = parse("bad.html", "first\n{#if a +}{/for}").unwrap_err();
    assert!(
        errors
            .iter()
            .any(|error| error.code == "E_EXPR_EXPECTED" && error.line == 2)
    );
    let close = errors
        .iter()
        .find(|error| error.code == "E_MISMATCHED_CLOSE")
        .unwrap();
    assert_eq!((close.line, close.column), (2, 10));
}

#[test]
fn safe_suffix_and_elvis_are_distinct() {
    let template = parse("safe", "{user.name?? ?: 'anonymous'}").unwrap();
    let Node::Output { expression, .. } = &template.nodes[0] else {
        panic!()
    };
    let Expr::Binary {
        op: BinaryOp::Elvis,
        left,
        ..
    } = expression
    else {
        panic!()
    };
    assert!(matches!(**left, Expr::Safe { .. }));
}

#[test]
fn parses_qute_textual_operator_aliases() {
    for expression in [
        "a eq b", "a is b", "a ne b", "a gt b", "a ge b", "a lt b", "a le b", "a and b",
    ] {
        parse("aliases", format!("{{{expression}}}"))
            .unwrap_or_else(|errors| panic!("failed to parse `{expression}`: {errors:?}"));
    }
}

#[test]
fn parses_else_if_as_a_conditional_block() {
    let template = parse(
        "condition",
        "{#if first}first{#else if second && third}second{#else}last{/if}",
    )
    .unwrap();
    let Node::Section(section) = &template.nodes[0] else {
        panic!("expected if section")
    };

    assert_eq!(section.blocks.len(), 3);
    assert_eq!(section.blocks[1].name, "else");
    assert!(matches!(
        section.blocks[1].arguments.as_slice(),
        [radiant_compiler::Argument {
            name: Some(name),
            value: ArgumentValue::Expression(Expr::Binary {
                op: BinaryOp::And,
                ..
            }),
            ..
        }] if name == "if"
    ));
    assert!(section.blocks[2].arguments.is_empty());
}

#[test]
fn accepts_whitespace_around_named_argument_equals() {
    let template = parse(
        "arguments",
        "{#let first=1 second =2 third= 3 fourth = 'four value'}{/let}",
    )
    .unwrap();
    let Node::Section(section) = &template.nodes[0] else {
        panic!("expected let section")
    };

    assert_eq!(
        section
            .arguments
            .iter()
            .map(|argument| argument.name.as_deref())
            .collect::<Vec<_>>(),
        [Some("first"), Some("second"), Some("third"), Some("fourth")]
    );
    assert!(matches!(
        &section.arguments[3].value,
        ArgumentValue::String(value) if value == "four value"
    ));
}

#[test]
fn validates_fragment_and_capture_identifiers_together() {
    let errors = parse(
        "fragments",
        "{#fragment invalid-id}{/fragment}{#fragment same}{/fragment}{#capture same}{/capture}",
    )
    .unwrap_err();

    assert!(errors.iter().any(|error| error.code == "E_FRAGMENT_ID"));
    assert!(
        errors
            .iter()
            .any(|error| error.code == "E_DUPLICATE_FRAGMENT")
    );
}
