use radiant_compiler::{Argument, Node, Section, Span};
use tower_lsp::lsp_types::{DocumentSymbol, SymbolKind};

use crate::DocumentSnapshot;

pub(crate) fn document_symbols(snapshot: &DocumentSnapshot) -> Vec<DocumentSymbol> {
    symbols_for_nodes(snapshot, &snapshot.analysis.template.nodes)
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
    use tower_lsp::lsp_types::{Position, SymbolKind, Url};

    use crate::DocumentStore;

    use super::document_symbols;

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
}
