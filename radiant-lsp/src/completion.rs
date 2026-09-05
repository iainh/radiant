use radiant_compiler::{Argument, BUILT_IN_SECTIONS, Node, Section, Span, built_in_block_names};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, Position};

use crate::DocumentSnapshot;

pub(crate) fn completions(snapshot: &DocumentSnapshot, position: Position) -> Vec<CompletionItem> {
    let cursor = snapshot.line_index.position_to_byte(position);
    if suppressed_by_ast(&snapshot.analysis.template.nodes, cursor) {
        return Vec::new();
    }
    let LexicalContext::Tag { start, content } = lexical_context(&snapshot.text, cursor) else {
        return Vec::new();
    };

    if let Some(section_prefix) = content.strip_prefix('#') {
        if !section_prefix.chars().any(char::is_whitespace) {
            let mut items = BUILT_IN_SECTIONS
                .iter()
                .map(|name| item(name, CompletionItemKind::KEYWORD, "built-in section"))
                .collect::<Vec<_>>();
            if let Some(parent) = containing_section(&snapshot.analysis.template.nodes, start) {
                items.extend(
                    built_in_block_names(&parent.name)
                        .iter()
                        .map(|name| item(name, CompletionItemKind::KEYWORD, "section block")),
                );
            }
            return items;
        }
        if !section_expression_position(section_prefix) {
            return Vec::new();
        }
    } else if content.starts_with(['/', '@', '!', '|']) || !expression_position(content) {
        return Vec::new();
    }

    let mut bindings = Vec::new();
    collect_bindings(
        &snapshot.analysis.template.nodes,
        cursor,
        &snapshot.text,
        &mut bindings,
    );
    bindings
        .into_iter()
        .map(|binding| item(&binding.name, CompletionItemKind::VARIABLE, &binding.detail))
        .collect()
}

fn item(label: &str, kind: CompletionItemKind, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: Some(kind),
        detail: Some(detail.into()),
        ..CompletionItem::default()
    }
}

fn section_expression_position(content: &str) -> bool {
    let content = content.trim_start();
    let split = content.find(char::is_whitespace).unwrap_or(content.len());
    let name = &content[..split];
    let arguments = content[split..].trim_start();
    match name {
        "if" | "when" | "with" => expression_position(arguments),
        "for" => after_word(arguments, "in").is_some_and(expression_position),
        "else" => arguments
            .strip_prefix("if")
            .is_some_and(expression_position),
        "let" | "set" => arguments
            .rsplit_once('=')
            .is_some_and(|(_, expression)| expression_position(expression)),
        _ => arguments
            .rsplit_once('=')
            .is_some_and(|(_, expression)| expression_position(expression)),
    }
}

fn after_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    text.char_indices().find_map(|(at, _)| {
        let end = at + word.len();
        (text.get(at..end) == Some(word)
            && text[..at].chars().last().is_none_or(char::is_whitespace)
            && text[end..].chars().next().is_none_or(char::is_whitespace))
        .then(|| &text[end..])
    })
}

fn expression_position(content: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for character in content.chars() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote.is_some() {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        }
    }
    if quote.is_some() {
        return false;
    }
    let trimmed = content.trim_end();
    let identifier_start = trimmed
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_alphanumeric() || *character == '_')
        .last()
        .map_or(trimmed.len(), |(at, _)| at);
    !matches!(trimmed[..identifier_start].chars().last(), Some('.' | ':'))
}

enum LexicalContext<'a> {
    None,
    Suppressed,
    Tag { start: usize, content: &'a str },
}

fn lexical_context(text: &str, cursor: usize) -> LexicalContext<'_> {
    let prefix = &text[..cursor];
    let mut at = 0;
    while let Some(relative) = prefix[at..].find('{') {
        let start = at + relative;
        if start > 0 && prefix.as_bytes()[start - 1] == b'\\' {
            at = start + 1;
            continue;
        }
        let rest = &prefix[start + 1..];
        if let Some(comment) = rest.strip_prefix('!') {
            let Some(end) = comment.find("!}") else {
                return LexicalContext::Suppressed;
            };
            at = start + 2 + end + 2;
            continue;
        }
        if rest.starts_with('|') {
            let pipes = rest.bytes().take_while(|byte| *byte == b'|').count();
            let delimiter = format!("{}{}", "|".repeat(pipes), "}");
            let Some(end) = rest[pipes..].find(&delimiter) else {
                return LexicalContext::Suppressed;
            };
            at = start + 1 + pipes + end + delimiter.len();
            continue;
        }
        let recognized = rest.chars().next().is_none_or(|character| {
            character.is_ascii_alphanumeric()
                || character == '_'
                || matches!(character, '#' | '/' | '@')
        });
        if !recognized {
            at = start + 1;
            continue;
        }
        if let Some(end) = tag_end(rest) {
            at = start + 1 + end + 1;
            continue;
        }
        return LexicalContext::Tag {
            start,
            content: rest,
        };
    }
    LexicalContext::None
}

fn tag_end(text: &str) -> Option<usize> {
    let mut quote = None;
    let mut escaped = false;
    for (at, character) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if character == '\\' && quote.is_some() {
            escaped = true;
        } else if quote == Some(character) {
            quote = None;
        } else if quote.is_none() && matches!(character, '\'' | '"') {
            quote = Some(character);
        } else if quote.is_none() && character == '}' {
            return Some(at);
        }
    }
    None
}

fn suppressed_by_ast(nodes: &[Node], cursor: usize) -> bool {
    nodes.iter().any(|node| match node {
        Node::Comment { span, .. } | Node::Unparsed { span, .. } => contains(*span, cursor),
        Node::Section(section) => section
            .blocks
            .iter()
            .any(|block| suppressed_by_ast(&block.nodes, cursor)),
        _ => false,
    })
}

fn containing_section(nodes: &[Node], cursor: usize) -> Option<&Section> {
    nodes.iter().find_map(|node| {
        let Node::Section(section) = node else {
            return None;
        };
        (section.span.start < cursor && cursor <= section.span.end).then(|| {
            section
                .blocks
                .iter()
                .find_map(|block| containing_section(&block.nodes, cursor))
                .unwrap_or(section)
        })
    })
}

#[derive(Debug)]
struct Binding {
    name: String,
    detail: String,
}

fn collect_bindings(nodes: &[Node], cursor: usize, text: &str, bindings: &mut Vec<Binding>) {
    for node in nodes {
        if let Node::Parameter(parameter) = node {
            push_binding(
                bindings,
                &parameter.name,
                format!("parameter: {}", parameter.type_name),
            );
        }
    }

    for node in nodes {
        let Node::Section(section) = node else {
            continue;
        };
        if !(section.span.start < cursor && cursor <= section.span.end) {
            continue;
        }
        let Some((index, block)) = section.blocks.iter().enumerate().find(|(index, block)| {
            let body_start = opening_end(text, block.span).unwrap_or(block.span.start);
            let body_end = section
                .blocks
                .get(index + 1)
                .map_or(section.span.end, |next| next.span.start);
            body_start <= cursor && cursor <= body_end
        }) else {
            return;
        };
        if index == 0 {
            section_bindings(section, bindings);
        }
        collect_bindings(&block.nodes, cursor, text, bindings);
        return;
    }
}

fn section_bindings(section: &Section, bindings: &mut Vec<Binding>) {
    match section.name.as_str() {
        "for" | "each" => {
            let alias = section
                .arguments
                .iter()
                .find(|argument| argument.name.as_deref() == Some("alias"))
                .and_then(Argument::static_text)
                .unwrap_or("it");
            push_binding(bindings, alias, "loop alias".into());
        }
        "let" | "set" => {
            for name in section
                .arguments
                .iter()
                .filter_map(|argument| argument.name.as_deref())
            {
                push_binding(bindings, name, "local binding".into());
            }
        }
        _ => {}
    }
}

fn push_binding(bindings: &mut Vec<Binding>, name: &str, detail: String) {
    if let Some(existing) = bindings.iter_mut().find(|binding| binding.name == name) {
        existing.detail = detail;
    } else {
        bindings.push(Binding {
            name: name.into(),
            detail,
        });
    }
}

fn opening_end(text: &str, span: Span) -> Option<usize> {
    tag_end(&text[span.start + 1..]).map(|relative| span.start + relative + 2)
}

fn contains(span: Span, cursor: usize) -> bool {
    span.start <= cursor && cursor < span.end
}

#[cfg(test)]
mod tests {
    use tower_lsp::lsp_types::{CompletionItem, Position, Url};

    use crate::DocumentStore;

    use super::completions;

    fn labels(source: &str, marker: &str) -> Vec<String> {
        let cursor = source.find(marker).unwrap();
        let source = source.replacen(marker, "", 1);
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        let snapshot = documents.open(uri, 1, source);
        completions(snapshot, snapshot.line_index.byte_to_position(cursor))
            .into_iter()
            .map(|item: CompletionItem| item.label)
            .collect()
    }

    #[test]
    fn completes_built_in_sections_in_incomplete_tags() {
        let labels = labels("<main>{#i<CURSOR>", "<CURSOR>");

        assert!(labels.contains(&"if".into()));
        assert!(labels.contains(&"fragment".into()));
        assert!(!labels.contains(&"else".into()));
    }

    #[test]
    fn adds_only_context_valid_blocks_inside_sections() {
        let labels = labels("{#when state}{#<CURSOR>{/when}", "<CURSOR>");

        assert!(labels.contains(&"if".into()));
        assert!(labels.contains(&"is".into()));
        assert!(labels.contains(&"case".into()));
        assert!(labels.contains(&"else".into()));
    }

    #[test]
    fn completes_blocks_in_incomplete_nested_tags_at_end_of_file() {
        let labels = labels("{#if shown}{#<CURSOR>", "<CURSOR>");

        assert!(labels.contains(&"else".into()));
    }

    #[test]
    fn completes_parameters_and_lexically_scoped_locals() {
        let source = "{@Vec<Item> items}{@String title}{#for item in items}{#let label=title}{la<CURSOR>}{/let}{/for}";
        let in_scope = labels(source, "<CURSOR>");

        assert_eq!(in_scope, ["items", "title", "item", "label"]);
        let outside = labels(
            "{@String title}{#let label=title}{label}{/let}{outside<CURSOR>}",
            "<CURSOR>",
        );
        assert!(!outside.contains(&"label".into()));
        assert!(
            labels("{@String title}{#let label = ti<CURSOR>", "<CURSOR>").contains(&"title".into())
        );
    }

    #[test]
    fn suppresses_completion_outside_expression_and_tag_contexts() {
        for source in [
            "<div>plain<CURSOR> html</div>",
            "{! hidden {na<CURSOR>} !}",
            "{! unfinished {na<CURSOR>",
            "{| raw {na<CURSOR>} |}",
            "{|| raw {na<CURSOR> |}",
            "{#include 'na<CURSOR>me' /}",
        ] {
            assert!(labels(source, "<CURSOR>").is_empty(), "source: {source}");
        }
        assert!(
            labels("{@User user}{user.na<CURSOR>}", "<CURSOR>").is_empty(),
            "member completion needs Rust type information"
        );
    }

    #[test]
    fn accepts_utf16_completion_positions() {
        let source = "😀{@String name}{na";
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        let snapshot = documents.open(uri, 1, source.into());

        assert_eq!(completions(snapshot, Position::new(0, 20))[0].label, "name");
    }
}
