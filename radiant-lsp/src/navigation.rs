use radiant_compiler::{BUILT_IN_SECTIONS, Span, built_in_block_names};
use tower_lsp::lsp_types::{
    GotoDefinitionResponse, Hover, HoverContents, Location, MarkupContent, MarkupKind, Position,
    Url,
};

use crate::{
    DocumentSnapshot, DocumentStore, LineIndex,
    semantic::{SemanticIndex, TemplateReference},
    workspace::WorkspaceIndex,
};

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
    workspace: &WorkspaceIndex,
    documents: &DocumentStore,
) -> Option<GotoDefinitionResponse> {
    let cursor = snapshot.line_index.position_to_byte(position);
    let semantic = SemanticIndex::new(&snapshot.analysis.template.nodes, &snapshot.text);
    if let Some(reference) = semantic.template_reference_at(cursor) {
        let source_id = workspace.template_id(uri)?;
        let (target, fragment) = match reference {
            TemplateReference::Include {
                target,
                target_span: _,
                fragment,
            } if fragment
                .as_ref()
                .is_some_and(|(_, span)| contains(*span, cursor)) =>
            {
                (
                    if target.is_empty() {
                        &source_id
                    } else {
                        target
                    },
                    fragment.as_ref().map(|(name, _)| name.as_str()),
                )
            }
            TemplateReference::Include { target, .. } if !target.is_empty() => (target, None),
            TemplateReference::Tag { target, .. } => (target, None),
            TemplateReference::Include { .. } => return None,
        };
        if !workspace.valid_id(target) {
            return None;
        }
        let target = workspace
            .documents(documents, Some(uri))
            .into_iter()
            .find(|document| document.id == *target)?;
        let range = if let Some(fragment) = fragment {
            let target_semantic = SemanticIndex::new(
                &target.analysis.template.nodes,
                &target.analysis.template.source,
            );
            let declaration = target_semantic
                .fragments()
                .iter()
                .find(|declaration| declaration.name == fragment)?;
            LineIndex::new(&target.analysis.template.source).span_to_range(declaration.name_span)
        } else {
            Default::default()
        };
        return Some(GotoDefinitionResponse::Scalar(Location::new(
            target.uri, range,
        )));
    }
    if let Some(fragment) = semantic.fragment_at(cursor) {
        return Some(GotoDefinitionResponse::Scalar(Location::new(
            uri.clone(),
            snapshot.line_index.span_to_range(fragment.name_span),
        )));
    }
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

pub(crate) fn references(
    snapshot: &DocumentSnapshot,
    uri: &Url,
    position: Position,
    include_declaration: bool,
    workspace: &WorkspaceIndex,
    documents: &DocumentStore,
) -> Option<Vec<Location>> {
    let cursor = snapshot.line_index.position_to_byte(position);
    let semantic = SemanticIndex::new(&snapshot.analysis.template.nodes, &snapshot.text);
    if let Some(declaration) = semantic.declaration_at(cursor).or_else(|| {
        let reference = semantic.reference_at(cursor)?;
        semantic.resolve(&reference.name, cursor)
    }) {
        let mut locations = semantic
            .references_to(declaration)
            .into_iter()
            .map(|reference| {
                Location::new(
                    uri.clone(),
                    snapshot.line_index.span_to_range(reference.span),
                )
            })
            .collect::<Vec<_>>();
        if include_declaration {
            locations.push(Location::new(
                uri.clone(),
                snapshot.line_index.span_to_range(declaration.name_span),
            ));
        }
        sort_locations(&mut locations);
        return Some(locations);
    }

    let source_id = workspace.template_id(uri)?;
    let target = if let Some(fragment) = semantic.fragment_at(cursor) {
        ReferenceTarget::Fragment(source_id, fragment.name.clone())
    } else {
        match semantic.template_reference_at(cursor)? {
            TemplateReference::Include {
                target,
                target_span,
                fragment,
            } if fragment
                .as_ref()
                .is_some_and(|(_, span)| contains(*span, cursor)) =>
            {
                ReferenceTarget::Fragment(
                    if target.is_empty() {
                        source_id
                    } else {
                        target.clone()
                    },
                    fragment.as_ref()?.0.clone(),
                )
            }
            TemplateReference::Include {
                target,
                target_span,
                ..
            } if contains(*target_span, cursor) && !target.is_empty() => {
                ReferenceTarget::Template(target.clone())
            }
            TemplateReference::Tag { target, .. } => ReferenceTarget::Template(target.clone()),
            TemplateReference::Include { .. } => return None,
        }
    };

    let workspace_documents = workspace.documents(documents, Some(uri));
    if !workspace.valid_id(target.template_id()) {
        return None;
    }
    let declaration = workspace_documents
        .iter()
        .find(|document| document.id == target.template_id())?;
    if let ReferenceTarget::Fragment(_, name) = &target {
        let target_semantic = SemanticIndex::new(
            &declaration.analysis.template.nodes,
            &declaration.analysis.template.source,
        );
        if !target_semantic
            .fragments()
            .iter()
            .any(|fragment| fragment.name == *name)
        {
            return None;
        }
    }

    let mut locations = Vec::new();
    for document in &workspace_documents {
        let index = SemanticIndex::new(
            &document.analysis.template.nodes,
            &document.analysis.template.source,
        );
        let lines = LineIndex::new(&document.analysis.template.source);
        for reference in index.template_references() {
            match (reference, &target) {
                (
                    TemplateReference::Include {
                        target: referenced,
                        target_span,
                        fragment: _,
                    },
                    ReferenceTarget::Template(expected),
                ) if referenced == expected => locations.push(Location::new(
                    document.uri.clone(),
                    lines.span_to_range(*target_span),
                )),
                (
                    TemplateReference::Tag {
                        target: referenced,
                        span,
                    },
                    ReferenceTarget::Template(expected),
                ) if referenced == expected => locations.push(Location::new(
                    document.uri.clone(),
                    lines.span_to_range(*span),
                )),
                (
                    TemplateReference::Include {
                        target: referenced,
                        fragment: Some((fragment, span)),
                        ..
                    },
                    ReferenceTarget::Fragment(expected_template, expected_fragment),
                ) if (referenced == expected_template
                    || referenced.is_empty() && document.id == *expected_template)
                    && fragment == expected_fragment =>
                {
                    locations.push(Location::new(
                        document.uri.clone(),
                        lines.span_to_range(*span),
                    ));
                }
                _ => {}
            }
        }
    }
    if include_declaration {
        let lines = LineIndex::new(&declaration.analysis.template.source);
        let span = match &target {
            ReferenceTarget::Template(_) => {
                Span::new(0, declaration.analysis.template.source.len())
            }
            ReferenceTarget::Fragment(_, name) => {
                SemanticIndex::new(
                    &declaration.analysis.template.nodes,
                    &declaration.analysis.template.source,
                )
                .fragments()
                .iter()
                .find(|fragment| fragment.name == *name)?
                .name_span
            }
        };
        locations.push(Location::new(
            declaration.uri.clone(),
            lines.span_to_range(span),
        ));
    }
    sort_locations(&mut locations);
    Some(locations)
}

enum ReferenceTarget {
    Template(String),
    Fragment(String, String),
}

impl ReferenceTarget {
    fn template_id(&self) -> &str {
        match self {
            Self::Template(id) | Self::Fragment(id, _) => id,
        }
    }
}

fn sort_locations(locations: &mut [Location]) {
    locations.sort_by(|left, right| {
        left.uri
            .as_str()
            .cmp(right.uri.as_str())
            .then_with(|| left.range.start.line.cmp(&right.range.start.line))
            .then_with(|| left.range.start.character.cmp(&right.range.start.character))
            .then_with(|| left.range.end.line.cmp(&right.range.end.line))
            .then_with(|| left.range.end.character.cmp(&right.range.end.character))
    });
}

const fn contains(span: Span, cursor: usize) -> bool {
    span.start <= cursor && cursor < span.end
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
    use std::fs;

    use tempfile::tempdir;
    use tower_lsp::lsp_types::{HoverContents, Position, Url};

    use crate::{DocumentStore, workspace::WorkspaceIndex};

    use super::{definition, hover, references};

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

    fn local_definition(
        snapshot: &crate::DocumentSnapshot,
        uri: &Url,
        position: Position,
        documents: &DocumentStore,
    ) -> Option<tower_lsp::lsp_types::GotoDefinitionResponse> {
        definition(
            snapshot,
            uri,
            position,
            &WorkspaceIndex::default(),
            documents,
        )
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
            let response = local_definition(
                snapshot,
                &uri,
                snapshot.line_index.byte_to_position(use_at),
                &documents,
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
    fn definitions_cover_each_alias_and_let_bindings() {
        let source = "{#each items}{it}{/each}{#let answer=42}{answer}{/let}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();

        for (usage, declaration, length) in [
            ("{it}", "each", "each".len()),
            ("{answer}", "answer=", "answer".len()),
        ] {
            let response = local_definition(
                snapshot,
                &uri,
                snapshot
                    .line_index
                    .byte_to_position(source.find(usage).unwrap() + 1),
                &documents,
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
                local_definition(
                    snapshot,
                    &uri,
                    snapshot.line_index.byte_to_position(byte),
                    &documents,
                )
                .is_none()
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
            local_definition(
                snapshot,
                &uri,
                snapshot
                    .line_index
                    .byte_to_position(source.find("each").unwrap()),
                &documents,
            )
            .is_none()
        );
    }

    #[test]
    fn definitions_resolve_static_includes_and_user_tags_but_not_dynamic_or_escaping_paths() {
        let workspace = tempdir().unwrap();
        fs::create_dir_all(workspace.path().join("templates/layouts")).unwrap();
        fs::create_dir_all(workspace.path().join("templates/tags")).unwrap();
        let layout = workspace.path().join("templates/layouts/base.html");
        let tag = workspace.path().join("templates/tags/card.html");
        fs::write(&layout, "layout").unwrap();
        fs::write(&tag, "tag").unwrap();
        fs::write(workspace.path().join("templates/_id=target.html"), "decoy").unwrap();
        let source =
            "{#include 'layouts/base' /}{#card /}{#include _id=target /}{#include ../secret /}";
        let uri = Url::from_file_path(workspace.path().join("templates/page.html")).unwrap();
        let mut documents = DocumentStore::default();
        documents.open(uri.clone(), 1, source.into());
        let snapshot = documents.get(&uri).unwrap();
        let mut templates = WorkspaceIndex::default();
        templates.set_roots([Url::from_file_path(workspace.path()).unwrap()]);

        for (reference, target) in [("layouts/base", layout), ("card", tag)] {
            let response = definition(
                snapshot,
                &uri,
                snapshot
                    .line_index
                    .byte_to_position(source.find(reference).unwrap()),
                &templates,
                &documents,
            )
            .unwrap();
            let tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(location) = response else {
                panic!("expected scalar definition")
            };
            assert_eq!(location.uri, Url::from_file_path(target).unwrap());
            assert_eq!(location.range.start, Position::new(0, 0));
        }
        for reference in ["target", "../secret"] {
            assert!(
                definition(
                    snapshot,
                    &uri,
                    snapshot
                        .line_index
                        .byte_to_position(source.find(reference).unwrap()),
                    &templates,
                    &documents,
                )
                .is_none()
            );
        }
    }

    #[test]
    fn references_follow_exact_lexical_bindings_and_declaration_context() {
        let source = "😀{@String item}{item}{#for item in items}{item}{#let item='x'}{item}{/let}{item}{#else}{item}{/for}{item}";
        let (uri, documents) = snapshot(source);
        let snapshot = documents.get(&uri).unwrap();
        let workspace = WorkspaceIndex::default();

        let inner_use = source.find("{item}{/let}").unwrap() + 1;
        let inner = references(
            snapshot,
            &uri,
            snapshot.line_index.byte_to_position(inner_use),
            true,
            &workspace,
            &documents,
        )
        .unwrap();
        assert_eq!(inner.len(), 2);
        assert_eq!(
            inner[0].range.start,
            snapshot
                .line_index
                .byte_to_position(source.find("item='x'").unwrap())
        );
        assert_eq!(
            inner[1].range.start,
            snapshot.line_index.byte_to_position(inner_use)
        );

        let parameter = references(
            snapshot,
            &uri,
            snapshot
                .line_index
                .byte_to_position(source.find("item}").unwrap()),
            false,
            &workspace,
            &documents,
        )
        .unwrap();
        assert_eq!(parameter.len(), 3);
        let parameter_uses = [
            source.find("{item}").unwrap() + 1,
            source.find("{#else}{item}").unwrap() + "{#else}{".len(),
            source.rfind("{item}").unwrap() + 1,
        ];
        assert_eq!(
            parameter
                .iter()
                .map(|location| location.range.start)
                .collect::<Vec<_>>(),
            parameter_uses.map(|byte| snapshot.line_index.byte_to_position(byte))
        );
    }

    #[test]
    fn fragment_definitions_and_references_use_exact_external_and_current_name_ranges() {
        let workspace_dir = tempdir().unwrap();
        let templates_dir = workspace_dir.path().join("templates");
        fs::create_dir_all(&templates_dir).unwrap();
        let fragments_path = templates_dir.join("fragments.html");
        let fragment_source = "😀\n{#fragment café /}\n{#capture note /}";
        fs::write(&fragments_path, fragment_source).unwrap();
        let page_source = "{#include fragments$café /}{#include fragments$note /}";
        let page_uri = Url::from_file_path(templates_dir.join("page.html")).unwrap();
        let fragments_uri = Url::from_file_path(&fragments_path).unwrap();
        let mut documents = DocumentStore::default();
        documents.open(page_uri.clone(), 1, page_source.into());
        documents.open(
            fragments_uri.clone(),
            1,
            format!("{fragment_source}\n{{#include $café /}}"),
        );
        let mut workspace = WorkspaceIndex::default();
        workspace.set_roots([Url::from_file_path(workspace_dir.path()).unwrap()]);
        let page = documents.get(&page_uri).unwrap();

        let definition = definition(
            page,
            &page_uri,
            page.line_index
                .byte_to_position(page_source.find("café").unwrap()),
            &workspace,
            &documents,
        )
        .unwrap();
        let tower_lsp::lsp_types::GotoDefinitionResponse::Scalar(definition) = definition else {
            panic!("expected scalar definition")
        };
        assert_eq!(definition.uri, fragments_uri);
        assert_eq!(
            definition.range,
            tower_lsp::lsp_types::Range::new(Position::new(1, 11), Position::new(1, 15))
        );

        let fragment = documents.get(&fragments_uri).unwrap();
        let locations = references(
            fragment,
            &fragments_uri,
            fragment
                .line_index
                .byte_to_position(fragment.text.find("café").unwrap()),
            true,
            &workspace,
            &documents,
        )
        .unwrap();
        assert_eq!(locations.len(), 3);
        assert_eq!(locations[0].uri, fragments_uri);
        assert_eq!(locations[0].range.start, Position::new(1, 11));
        assert_eq!(locations[1].range.start, Position::new(3, 11));
        assert_eq!(locations[2].uri, page_uri);
        assert_eq!(locations[2].range.start, Position::new(0, 20));
    }

    #[test]
    fn references_find_static_includes_and_tags_but_exclude_dynamic_and_other_roots() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        for workspace in [&first, &second] {
            fs::create_dir_all(workspace.path().join("templates/tags")).unwrap();
            fs::write(workspace.path().join("templates/layout.html"), "layout").unwrap();
            fs::write(workspace.path().join("templates/tags/card.html"), "card").unwrap();
        }
        let source = "{#include layout /}{#include _id=layout /}{#card /}{#card /}";
        let first_uri = Url::from_file_path(first.path().join("templates/page.html")).unwrap();
        let second_uri = Url::from_file_path(second.path().join("templates/page.html")).unwrap();
        let mut documents = DocumentStore::default();
        documents.open(first_uri.clone(), 1, source.into());
        documents.open(second_uri, 1, source.into());
        let mut workspace = WorkspaceIndex::default();
        workspace.set_roots([
            Url::from_file_path(first.path()).unwrap(),
            Url::from_file_path(second.path()).unwrap(),
        ]);
        let snapshot = documents.get(&first_uri).unwrap();

        let includes = references(
            snapshot,
            &first_uri,
            snapshot
                .line_index
                .byte_to_position(source.find("layout").unwrap()),
            false,
            &workspace,
            &documents,
        )
        .unwrap();
        assert_eq!(includes.len(), 1);
        assert_eq!(&snapshot.text[10..16], "layout");
        assert_eq!(includes[0].range.start, Position::new(0, 10));

        let tags = references(
            snapshot,
            &first_uri,
            snapshot
                .line_index
                .byte_to_position(source.find("card").unwrap()),
            true,
            &workspace,
            &documents,
        )
        .unwrap();
        assert_eq!(tags.len(), 3);
        let tag_starts = source
            .match_indices("{#card")
            .map(|(byte, _)| snapshot.line_index.byte_to_position(byte + 2))
            .collect::<Vec<_>>();
        assert_eq!(
            tags.iter()
                .filter(|location| location.uri == first_uri)
                .map(|location| location.range.start)
                .collect::<Vec<_>>(),
            tag_starts
        );
    }
}
