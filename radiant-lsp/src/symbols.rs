use radiant_compiler::{Argument, Node, Section, Span};
use tower_lsp::lsp_types::{DocumentSymbol, Location, SymbolInformation, SymbolKind};

use crate::{
    DocumentSnapshot, DocumentStore, LineIndex, semantic::SemanticIndex, workspace::WorkspaceIndex,
};

pub(crate) fn document_symbols(snapshot: &DocumentSnapshot) -> Vec<DocumentSymbol> {
    symbols_for_nodes(snapshot, &snapshot.analysis.template.nodes)
}

#[allow(deprecated)]
pub(crate) fn workspace_symbols(
    query: &str,
    workspace: &WorkspaceIndex,
    documents: &DocumentStore,
) -> Vec<SymbolInformation> {
    let query = query.to_lowercase();
    let mut symbols = Vec::new();
    for document in workspace.documents(documents, None) {
        let lines = LineIndex::new(&document.analysis.template.source);
        if document.id.to_lowercase().contains(&query) {
            symbols.push(SymbolInformation {
                name: document.id.clone(),
                kind: SymbolKind::FILE,
                tags: None,
                deprecated: None,
                location: Location::new(
                    document.uri.clone(),
                    lines.span_to_range(Span::new(0, document.analysis.template.source.len())),
                ),
                container_name: None,
            });
        }
        let semantic = SemanticIndex::new(
            &document.analysis.template.nodes,
            &document.analysis.template.source,
        );
        for fragment in semantic.fragments() {
            let qualified = format!("{}${}", document.id, fragment.name);
            if fragment.name.to_lowercase().contains(&query)
                || qualified.to_lowercase().contains(&query)
            {
                symbols.push(SymbolInformation {
                    name: fragment.name.clone(),
                    kind: SymbolKind::OBJECT,
                    tags: None,
                    deprecated: None,
                    location: Location::new(
                        document.uri.clone(),
                        lines.span_to_range(fragment.name_span),
                    ),
                    container_name: Some(document.id.clone()),
                });
            }
        }
    }
    symbols.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.location.uri.as_str().cmp(right.location.uri.as_str()))
            .then_with(|| {
                left.location
                    .range
                    .start
                    .line
                    .cmp(&right.location.range.start.line)
            })
            .then_with(|| {
                left.location
                    .range
                    .start
                    .character
                    .cmp(&right.location.range.start.character)
            })
    });
    symbols
}

fn symbols_for_nodes(snapshot: &DocumentSnapshot, nodes: &[Node]) -> Vec<DocumentSymbol> {
    nodes
        .iter()
        .filter_map(|node| match node {
            Node::Parameter(parameter) => Some(symbol(
                parameter.name.clone(),
                Some(parameter.type_name.clone()),
                SymbolKind::VARIABLE,
                parameter.span,
                name_span(&snapshot.text, parameter.span, &parameter.name),
                Vec::new(),
                snapshot,
            )),
            Node::Section(section) => Some(section_symbol(snapshot, section)),
            _ => None,
        })
        .collect()
}

fn section_symbol(snapshot: &DocumentSnapshot, section: &Section) -> DocumentSymbol {
    let declaration_span = opening_name_span(&snapshot.text, section.span, '#', &section.name);
    let (name, detail, kind, selection_span) =
        if matches!(section.name.as_str(), "fragment" | "capture") {
            let id = section.arguments.first().and_then(Argument::static_text);
            (
                id.unwrap_or(&section.name).to_owned(),
                Some(section.name.clone()),
                SymbolKind::OBJECT,
                section
                    .arguments
                    .first()
                    .map_or(declaration_span, |argument| argument.span),
            )
        } else {
            (
                section.name.clone(),
                Some("section".into()),
                SymbolKind::NAMESPACE,
                declaration_span,
            )
        };

    let children = section
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let end = section
                .blocks
                .get(index + 1)
                .map_or(section.span.end, |next| next.span.start);
            symbol(
                block.name.clone(),
                Some("block".into()),
                SymbolKind::STRUCT,
                Span::new(block.span.start, end),
                opening_name_span(&snapshot.text, block.span, '#', &block.name),
                symbols_for_nodes(snapshot, &block.nodes),
                snapshot,
            )
        })
        .collect();

    symbol(
        name,
        detail,
        kind,
        section.span,
        selection_span,
        children,
        snapshot,
    )
}

#[allow(deprecated)]
fn symbol(
    name: String,
    detail: Option<String>,
    kind: SymbolKind,
    span: Span,
    selection_span: Span,
    children: Vec<DocumentSymbol>,
    snapshot: &DocumentSnapshot,
) -> DocumentSymbol {
    DocumentSymbol {
        name,
        detail,
        kind,
        tags: None,
        deprecated: None,
        range: snapshot.line_index.span_to_range(span),
        selection_range: snapshot.line_index.span_to_range(selection_span),
        children: (!children.is_empty()).then_some(children),
    }
}

fn opening_name_span(text: &str, span: Span, marker: char, name: &str) -> Span {
    let opening = &text[span.start..span.end.min(text.len())];
    opening
        .find(marker)
        .and_then(|marker_at| {
            opening[marker_at + marker.len_utf8()..]
                .find(name)
                .map(|at| marker_at + 1 + at)
        })
        .map_or(span, |at| {
            Span::new(span.start + at, span.start + at + name.len())
        })
}

fn name_span(text: &str, span: Span, name: &str) -> Span {
    text[span.start..span.end.min(text.len())]
        .rfind(name)
        .map_or(span, |at| {
            Span::new(span.start + at, span.start + at + name.len())
        })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use tower_lsp::lsp_types::{Position, SymbolKind, Url};

    use crate::{DocumentStore, LineIndex, workspace::WorkspaceIndex};

    use super::{document_symbols, workspace_symbols};

    #[test]
    fn builds_hierarchical_symbols_with_utf16_ranges() {
        let source = "😀{@String title}\n{#if title}{#for item in items}{#fragment card}{#capture note}x{/capture}{/fragment}{#else}none{/for}{#else}off{/if}";
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        let symbols = document_symbols(documents.open(uri, 1, source.into()));

        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "title");
        assert_eq!(symbols[0].detail.as_deref(), Some("String"));
        assert_eq!(symbols[0].selection_range.start, Position::new(0, 11));

        let if_section = &symbols[1];
        assert_eq!(if_section.name, "if");
        assert_eq!(if_section.kind, SymbolKind::NAMESPACE);
        assert_eq!(if_section.children.as_ref().unwrap().len(), 2);
        let for_section = &if_section.children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert_eq!(for_section.name, "for");
        assert_eq!(for_section.children.as_ref().unwrap().len(), 2);
        let fragment = &for_section.children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert_eq!(fragment.name, "card");
        assert_eq!(fragment.detail.as_deref(), Some("fragment"));
        let capture = &fragment.children.as_ref().unwrap()[0]
            .children
            .as_ref()
            .unwrap()[0];
        assert_eq!(capture.name, "note");
        assert_eq!(capture.detail.as_deref(), Some("capture"));
        assert!(if_section.range.end.character > if_section.selection_range.end.character);
    }

    #[test]
    fn retains_symbols_from_valid_regions_of_invalid_source() {
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        let snapshot = documents.open(uri, 1, "{#fragment good}{/fragment}{broken +}".into());

        assert_eq!(document_symbols(snapshot)[0].name, "good");
        assert!(!snapshot.analysis.diagnostics.is_empty());
    }

    #[test]
    fn workspace_symbols_filter_sort_multiple_roots_and_use_open_overlays() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        for root in [&first, &second] {
            fs::create_dir_all(root.path().join("templates")).unwrap();
        }
        let first_path = first.path().join("templates/alpha.html");
        let second_path = second.path().join("templates/beta.html");
        fs::write(&first_path, "{#fragment stale /}").unwrap();
        fs::write(&second_path, "{#capture shared /}").unwrap();
        let first_uri = Url::from_file_path(&first_path).unwrap();
        let open_uri = Url::from_file_path(first.path().join("templates/open.html")).unwrap();
        let hidden_uri = Url::from_file_path(first.path().join("templates/.hidden.html")).unwrap();
        let mut documents = DocumentStore::default();
        documents.open(first_uri.clone(), 1, "😀{#fragment Shared /}".into());
        documents.open(open_uri.clone(), 1, "{#capture overlay /}".into());
        documents.open(hidden_uri, 1, "{#fragment hidden /}".into());
        let mut workspace = WorkspaceIndex::default();
        workspace.set_roots([
            Url::from_file_path(second.path()).unwrap(),
            Url::from_file_path(first.path()).unwrap(),
        ]);

        let symbols = workspace_symbols("SHAR", &workspace, &documents);
        assert_eq!(
            symbols
                .iter()
                .map(|symbol| (symbol.name.as_str(), symbol.container_name.as_deref()))
                .collect::<Vec<_>>(),
            [("Shared", Some("alpha")), ("shared", Some("beta"))]
        );
        assert_eq!(symbols[0].location.uri, first_uri);
        assert_eq!(symbols[0].location.range.start, Position::new(0, 13));
        assert!(workspace_symbols("stale", &workspace, &documents).is_empty());
        assert!(workspace_symbols("hidden", &workspace, &documents).is_empty());

        let overlays = workspace_symbols("overlay", &workspace, &documents);
        assert_eq!(overlays.len(), 1);
        assert_eq!(overlays[0].location.uri, open_uri);
        assert_eq!(overlays[0].location.range.start, Position::new(0, 10));

        let templates = workspace_symbols("a", &workspace, &documents);
        assert!(templates.iter().any(|symbol| {
            symbol.name == "alpha"
                && symbol.kind == SymbolKind::FILE
                && symbol.location.range.end
                    == LineIndex::new("😀{#fragment Shared /}")
                        .byte_to_position("😀{#fragment Shared /}".len())
        }));
        assert!(templates.iter().any(|symbol| symbol.name == "beta"));
    }
}
