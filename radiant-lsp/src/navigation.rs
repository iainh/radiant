use radiant_compiler::{BUILT_IN_SECTIONS, Span, built_in_block_names};
use tower_lsp::lsp_types::{
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position,
    Url,
};

use crate::{DocumentSnapshot, semantic::SemanticIndex};

pub(crate) fn hover(snapshot: &DocumentSnapshot, position: Position) -> Option<Hover> {
    let cursor = snapshot.line_index.position_to_byte(position);
    let semantic = SemanticIndex::new(&snapshot.analysis.template.nodes, &snapshot.text);

    if let Some(construct) = semantic.construct_at(cursor) {
        let markdown = construct_markdown(&construct.name, construct.parent.as_deref())?;
        return Some(markdown_hover(snapshot, construct.name_span, markdown));
    }

    if let Some(declaration) = semantic.declaration_at(cursor) {
        return Some(markdown_hover(
            snapshot,
            declaration.name_span,
            declaration_markdown(declaration),
        ));
    }

    let reference = semantic.reference_at(cursor)?;
    let declaration = semantic.resolve(&reference.name, cursor)?;
    Some(markdown_hover(
        snapshot,
        reference.span,
        declaration_markdown(declaration),
    ))
}

pub(crate) fn definition(
    snapshot: &DocumentSnapshot,
    uri: &Url,
    position: Position,
) -> Option<GotoDefinitionResponse> {
    let cursor = snapshot.line_index.position_to_byte(position);
    let semantic = SemanticIndex::new(&snapshot.analysis.template.nodes, &snapshot.text);
    if semantic.construct_at(cursor).is_some() {
        return None;
    }
    let declaration = semantic.declaration_at(cursor).or_else(|| {
        let reference = semantic.reference_at(cursor)?;
        semantic.resolve(&reference.name, cursor)
    })?;
    Some(GotoDefinitionResponse::Scalar(Location::new(
        uri.clone(),
        snapshot.line_index.span_to_range(declaration.name_span),
    )))
}

fn markdown_hover(snapshot: &DocumentSnapshot, span: Span, value: String) -> Hover {
    Hover {
        contents: HoverContents::Markup(MarkupContent {
            kind: MarkupKind::Markdown,
            value,
        }),
        range: Some(snapshot.line_index.span_to_range(span)),
    }
}

fn declaration_markdown(declaration: &crate::semantic::Declaration) -> String {
    declaration.parameter_type.as_ref().map_or_else(
        || format!("**{}** `{}`", declaration.kind.detail(), declaration.name),
        |type_name| {
            format!(
                "**{}** `{}`\n\nType: `{type_name}`",
                declaration.kind.detail(),
                declaration.name
            )
        },
    )
}

fn construct_markdown(name: &str, parent: Option<&str>) -> Option<String> {
    if let Some(parent) = parent {
        if !built_in_block_names(parent).contains(&name) {
            return None;
        }
        let syntax = match (parent, name) {
            ("when", "is" | "case") => format!("{{#{name} value}}"),
            _ => format!("{{#{name}}}"),
        };
        return Some(format!(
            "```radiant\n{syntax}\n```\n\n`{name}` block in the `{parent}` section."
        ));
    }
    if !BUILT_IN_SECTIONS.contains(&name) {
        return None;
    }
    let (syntax, description) = match name {
        "if" => ("{#if condition}…{/if}", "Conditionally renders its body."),
        "for" => (
            "{#for item in items}…{/for}",
            "Iterates with an explicit alias.",
        ),
        "each" => (
            "{#each items}…{/each}",
            "Iterates with the implicit `it` alias.",
        ),
        "let" => (
            "{#let name=value}…{/let}",
            "Introduces named local bindings in its primary block.",
        ),
        "set" => (
            "{#set name=value}…{/set}",
            "Introduces named local bindings in its primary block.",
        ),
        "with" => ("{#with value}…{/with}", "Renders its body with a value."),
        "when" => (
            "{#when value}{#is expected}…{/when}",
            "Selects a matching case block.",
        ),
        "switch" => (
            "{#switch value}…{/switch}",
            "Selects a matching case block.",
        ),
        "include" => ("{#include 'template-id' /}", "Includes a template."),
        "insert" => (
            "{#insert name}…{/insert}",
            "Declares overridable inserted content.",
        ),
        "nested-content" => ("{#nested-content /}", "Renders nested tag content."),
        "fragment" => ("{#fragment name}…{/fragment}", "Declares a named fragment."),
        "capture" => ("{#capture name}…{/capture}", "Captures rendered content."),
        _ => return None,
    };
    Some(format!("```radiant\n{syntax}\n```\n\n{description}"))
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{HoverContents, Position, Url};

    use crate::DocumentStore;

    use super::{definition, hover};

    fn snapshot(source: &str) -> (Url, DocumentStore) {
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        documents.open(uri.clone(), 1, source.into());
        (uri, documents)
    }

    fn markdown(source: &str, byte: usize) -> Option<String> {
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();
        let hover = hover(snapshot, snapshot.line_index.byte_to_position(byte))?;
        let HoverContents::Markup(contents) = hover.contents else {
            panic!("expected Markdown hover")
        };
        Some(contents.value)
    }

    #[test]
    fn hovers_built_in_sections_and_blocks_with_markdown() {
        let source = "{#if shown}yes{#else}no{/if}";

        assert_eq!(
            markdown(source, source.find("if").unwrap()).unwrap(),
            "```radiant\n{#if condition}…{/if}\n```\n\nConditionally renders its body."
        );
        assert_eq!(
            markdown(source, source.find("else").unwrap()).unwrap(),
            "```radiant\n{#else}\n```\n\n`else` block in the `if` section."
        );
    }

    #[test]
    fn hovers_declarations_and_shadowed_uses_with_types_and_exact_ranges() {
        let source = "😀{@String name}{#let name='local'}{name}{/let}{name}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();
        let local_use = source.find("{name}").unwrap() + 1;
        let local = hover(
            snapshot,
            snapshot
                .line_index
                .byte_to_position(local_use + "nam".len()),
        )
        .unwrap();
        let HoverContents::Markup(local_contents) = local.contents else {
            panic!("expected Markdown hover")
        };
        assert_eq!(local_contents.value, "**local binding** `name`");
        assert_eq!(local.range.unwrap().start, Position::new(0, 36));

        assert_eq!(
            markdown(source, source.rfind("{name}").unwrap() + 1).unwrap(),
            "**parameter** `name`\n\nType: `String`"
        );
    }

    #[test]
    fn definitions_target_parameter_alias_and_named_binding_name_spans() {
        let source =
            "😀{@String items}{#for item in items}{#set label=item}{label}{/set}{item}{/for}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();
        for (usage, declaration, length) in [
            ("items}", "items}", "items".len()),
            ("{item}", "item in", "item".len()),
            ("{label}", "label=", "label".len()),
        ] {
            let use_at = source.rfind(usage).unwrap() + usize::from(usage.starts_with('{'));
            let response =
                definition(snapshot, &uri, snapshot.line_index.byte_to_position(use_at)).unwrap();
            let tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location) = response else {
                panic!("expected scalar definition")
            };
            let expected = source.find(declaration).unwrap();
            assert_eq!(
                location.range,
                snapshot
                    .line_index
                    .span_to_range(radiant_compiler::Span::new(expected, expected + length))
            );
        }
    }

    #[test]
    fn definitions_cover_each_alias_and_let_bindings() {
        let source = "{#each items}{it}{/each}{#let answer=42}{answer}{/let}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();

        for (usage, declaration, length) in [
            ("{it}", "each", "each".len()),
            ("{answer}", "answer=", "answer".len()),
        ] {
            let response = definition(
                snapshot,
                &uri,
                snapshot
                    .line_index
                    .byte_to_position(source.find(usage).unwrap() + 1),
            )
            .unwrap();
            let tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location) = response else {
                panic!("expected scalar definition")
            };
            let expected = source.find(declaration).unwrap();
            assert_eq!(
                location.range,
                snapshot
                    .line_index
                    .span_to_range(radiant_compiler::Span::new(expected, expected + length))
            );
        }
    }

    #[test]
    fn returns_none_for_unknown_member_and_out_of_scope_identifiers() {
        let source =
            "{@User user}name{user.name}{missing}{#for item in items}{item}{#else}{item}{/for}";
        assert!(markdown(source, source.find("name").unwrap()).is_none());
        for byte in [
            source.find(".name").unwrap() + 1,
            source.find("missing").unwrap(),
            source.rfind("{item}").unwrap() + 1,
        ] {
            assert!(markdown(source, byte).is_none());
            let (uri, documents) = snapshot(source);
            let snapshot = documents.get(&uri).unwrap();
            assert!(
                definition(snapshot, &uri, snapshot.line_index.byte_to_position(byte)).is_none()
            );
        }
    }

    #[test]
    fn identifier_boundaries_do_not_select_adjacent_punctuation() {
        let source = "{@String name}{name}";
        assert!(markdown(source, source.rfind("name").unwrap()).is_some());
        assert!(markdown(source, source.rfind("name").unwrap() + 3).is_some());
        assert!(markdown(source, source.len() - 1).is_none());
    }

    #[test]
    fn definition_does_not_treat_a_built_in_as_the_implicit_each_alias() {
        let source = "{#each items}{it}{/each}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();

        assert!(
            definition(
                snapshot,
                &uri,
                snapshot
                    .line_index
                    .byte_to_position(source.find("each").unwrap()),
            )
            .is_none()
        );
    }
}
