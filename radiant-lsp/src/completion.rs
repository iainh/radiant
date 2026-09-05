use std::collections::{BTreeMap, BTreeSet};

use radiant_compiler::{Analysis, BUILT_IN_SECTIONS, Node, Section, Span, built_in_block_names};
use tower_lsp::lsp_types::{CompletionItem, CompletionItemKind, InsertTextFormat, Position};

use crate::{DocumentSnapshot, semantic::SemanticIndex};

pub(crate) fn completions(
    snapshot: &DocumentSnapshot,
    position: Position,
    analyses: &BTreeMap<String, &Analysis>,
    snippets: bool,
) -> Vec<CompletionItem> {
    let cursor = snapshot.line_index.position_to_byte(position);
    let LexicalContext::Tag { start, content } = lexical_context(&snapshot.text, cursor) else {
        return Vec::new();
    };

    if let Some(section_prefix) = content.strip_prefix('#') {
        if let Some(reference) = include_reference_prefix(section_prefix) {
            if let Some((target, prefix)) = reference.split_once('$') {
                let target = if target.is_empty() {
                    Some(&snapshot.analysis)
                } else {
                    analyses.get(target).copied()
                };
                return ranked(
                    target
                        .into_iter()
                        .flat_map(fragment_names)
                        .map(|name| item(name, CompletionItemKind::REFERENCE, "fragment")),
                    prefix,
                );
            }
            return ranked(
                analyses
                    .keys()
                    .map(|id| item(id, CompletionItemKind::FILE, "template")),
                reference,
            );
        }
        if !section_prefix.chars().any(char::is_whitespace) {
            let mut items = BUILT_IN_SECTIONS
                .iter()
                .map(|name| section_item(name, snippets))
                .collect::<Vec<_>>();
            items.extend(
                analyses
                    .keys()
                    .filter_map(|id| id.strip_prefix("tags/"))
                    .filter(|name| !BUILT_IN_SECTIONS.contains(name))
                    .map(|name| item(name, CompletionItemKind::REFERENCE, "user tag")),
            );
            if let Some(parent) = containing_section(&snapshot.analysis.template.nodes, start) {
                items.extend(
                    built_in_block_names(&parent.name)
                        .iter()
                        .map(|name| item(name, CompletionItemKind::KEYWORD, "section block")),
                );
                if parent.name == "include"
                    && let Some(target) = parent.arguments.first().and_then(|argument| {
                        (argument.name.is_none())
                            .then(|| argument.static_text())
                            .flatten()
                    })
                    && let Some(target) = target.split('$').next()
                    && let Some(analysis) = analyses.get(target)
                {
                    items.extend(
                        insert_names(analysis)
                            .map(|name| item(name, CompletionItemKind::REFERENCE, "layout block")),
                    );
                }
            }
            return ranked(items, section_prefix);
        }
        if !section_expression_position(section_prefix) {
            return Vec::new();
        }
    } else if content.starts_with(['/', '@', '!', '|']) || !expression_position(content) {
        return Vec::new();
    }

    if suppressed_by_ast(&snapshot.analysis.template.nodes, cursor) {
        return Vec::new();
    }

    let prefix = identifier_prefix(content);
    ranked(
        SemanticIndex::new(&snapshot.analysis.template.nodes, &snapshot.text)
            .visible_declarations(cursor)
            .into_iter()
            .map(|declaration| {
                item(
                    &declaration.name,
                    CompletionItemKind::VARIABLE,
                    &declaration.detail(),
                )
            }),
        prefix,
    )
}

fn include_reference_prefix(section: &str) -> Option<&str> {
    let arguments = section.strip_prefix("include")?;
    if arguments.is_empty() || !arguments.starts_with(char::is_whitespace) {
        return None;
    }
    let argument = arguments.trim_start();
    let Some(first) = argument.chars().next() else {
        return Some("");
    };
    if matches!(first, '\'' | '"') {
        let content = &argument[first.len_utf8()..];
        return (!content.contains(first)).then_some(content);
    }
    (!argument.chars().any(char::is_whitespace)).then_some(argument)
}

fn item(label: &str, kind: CompletionItemKind, detail: &str) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: Some(kind),
        detail: Some(detail.into()),
        ..CompletionItem::default()
    }
}

fn section_item(name: &str, snippets: bool) -> CompletionItem {
    let Some((snippet, plain)) = section_insert_text(name) else {
        return item(name, CompletionItemKind::KEYWORD, "built-in section");
    };
    CompletionItem {
        insert_text: Some(if snippets { snippet } else { plain }.into()),
        insert_text_format: snippets.then_some(InsertTextFormat::SNIPPET),
        ..item(name, CompletionItemKind::SNIPPET, "built-in section")
    }
}

fn section_insert_text(name: &str) -> Option<(&'static str, &'static str)> {
    Some(match name {
        "if" => ("if ${1:condition}}${0}{/if}", "if condition}{/if}"),
        "for" => (
            "for ${1:item} in ${2:items}}${0}{/for}",
            "for item in items}{/for}",
        ),
        "each" => ("each ${1:items}}${0}{/each}", "each items}{/each}"),
        "let" => (
            "let ${1:name}=${2:value}}${0}{/let}",
            "let name=value}{/let}",
        ),
        "set" => (
            "set ${1:name}=${2:value}}${0}{/set}",
            "set name=value}{/set}",
        ),
        "with" => ("with ${1:value}}${0}{/with}", "with value}{/with}"),
        "when" => ("when ${1:value}}${0}{/when}", "when value}{/when}"),
        "switch" => ("switch ${1:value}}${0}{/switch}", "switch value}{/switch}"),
        "include" => ("include ${1:template-id} /}", "include template-id /}"),
        "insert" => ("insert ${1:name}}${0}{/insert}", "insert name}{/insert}"),
        "nested-content" => ("nested-content /}", "nested-content /}"),
        "fragment" => (
            "fragment ${1:name}}${0}{/fragment}",
            "fragment name}{/fragment}",
        ),
        "capture" => (
            "capture ${1:name}}${0}{/capture}",
            "capture name}{/capture}",
        ),
        _ => return None,
    })
}

fn fragment_names(analysis: &Analysis) -> impl Iterator<Item = &str> {
    analysis
        .template
        .fragments()
        .into_iter()
        .filter_map(|section| section.arguments.first()?.static_text())
        .collect::<BTreeSet<_>>()
        .into_iter()
}

fn insert_names(analysis: &Analysis) -> impl Iterator<Item = &str> {
    fn visit<'a>(nodes: &'a [Node], names: &mut BTreeSet<&'a str>) {
        for node in nodes {
            if let Node::Section(section) = node {
                if section.name == "insert"
                    && let Some(name) = section.arguments.first().and_then(|arg| arg.static_text())
                {
                    names.insert(name);
                }
                for block in &section.blocks {
                    visit(&block.nodes, names);
                }
            }
        }
    }
    let mut names = BTreeSet::new();
    visit(&analysis.template.nodes, &mut names);
    names.into_iter()
}

fn identifier_prefix(content: &str) -> &str {
    let trimmed = content.trim_end();
    let start = trimmed
        .char_indices()
        .rev()
        .take_while(|(_, character)| character.is_alphanumeric() || *character == '_')
        .last()
        .map_or(trimmed.len(), |(at, _)| at);
    &trimmed[start..]
}

fn ranked(items: impl IntoIterator<Item = CompletionItem>, prefix: &str) -> Vec<CompletionItem> {
    let mut items = items
        .into_iter()
        .filter(|item| item.label.starts_with(prefix))
        .collect::<Vec<_>>();
    if !prefix.is_empty() {
        items.sort_by(|left, right| {
            (left.label != prefix)
                .cmp(&(right.label != prefix))
                .then_with(|| left.label.len().cmp(&right.label.len()))
                .then_with(|| left.label.cmp(&right.label))
        });
    }
    for (rank, item) in items.iter_mut().enumerate() {
        item.sort_text = Some(format!("{rank:04}"));
    }
    items
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

fn contains(span: Span, cursor: usize) -> bool {
    span.start <= cursor && cursor < span.end
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use radiant_compiler::{Analysis, analyze};
    use tower_lsp::lsp_types::{
        CompletionItem, CompletionItemKind, InsertTextFormat, Position, Url,
    };

    use crate::DocumentStore;

    use super::completions;

    fn labels(source: &str, marker: &str) -> Vec<String> {
        workspace_labels(source, marker, &[], &[])
    }

    fn items(
        source: &str,
        marker: &str,
        templates: &[(&str, &str)],
        snippets: bool,
    ) -> Vec<CompletionItem> {
        let cursor = source.find(marker).unwrap();
        let source = source.replacen(marker, "", 1);
        let uri = Url::parse("file:///workspace/templates/page.html").unwrap();
        let mut documents = DocumentStore::default();
        let snapshot = documents.open(uri, 1, source);
        let owned = templates
            .iter()
            .map(|(id, source)| ((*id).to_owned(), analyze(id, source)))
            .collect::<BTreeMap<String, Analysis>>();
        let analyses = owned
            .iter()
            .map(|(id, analysis)| (id.clone(), analysis))
            .collect();
        completions(
            snapshot,
            snapshot.line_index.byte_to_position(cursor),
            &analyses,
            snippets,
        )
    }

    fn workspace_labels(
        source: &str,
        marker: &str,
        template_ids: &[String],
        tag_names: &[String],
    ) -> Vec<String> {
        let owned = template_ids
            .iter()
            .cloned()
            .chain(tag_names.iter().map(|name| format!("tags/{name}")))
            .collect::<Vec<String>>();
        let templates = owned.iter().map(|id| (id.as_str(), "")).collect::<Vec<_>>();
        items(source, marker, &templates, false)
            .into_iter()
            .map(|item: CompletionItem| item.label)
            .collect()
    }

    #[test]
    fn completes_built_in_sections_in_incomplete_tags() {
        let labels = labels("<main>{#i<CURSOR>", "<CURSOR>");

        assert!(labels.contains(&"if".into()));
        assert!(labels.contains(&"include".into()));
        assert!(!labels.contains(&"fragment".into()));
        assert!(!labels.contains(&"else".into()));
    }

    #[test]
    fn completes_include_ids_and_user_tags_in_incomplete_tags() {
        let templates = ["layouts/base".into(), "tags/card".into()];
        let tags = ["card".into()];

        assert_eq!(
            workspace_labels("{#include 'lay<CURSOR>", "<CURSOR>", &templates, &tags),
            ["layouts/base"]
        );
        assert_eq!(
            workspace_labels("{#ca<CURSOR>", "<CURSOR>", &templates, &tags),
            ["card", "capture"]
        );
        assert!(
            workspace_labels(
                "{#include layouts/base x=<CURSOR>",
                "<CURSOR>",
                &templates,
                &tags
            )
            .is_empty()
        );
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

        assert_eq!(in_scope, ["label"]);
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

        assert_eq!(
            completions(snapshot, Position::new(0, 20), &BTreeMap::new(), false)[0].label,
            "name"
        );
    }

    #[test]
    fn filters_and_deterministically_ranks_typed_prefixes() {
        assert_eq!(
            labels("{#i<CURSOR>", "<CURSOR>"),
            ["if", "for", "each", "with", "switch", "insert", "include"]
                .into_iter()
                .filter(|label| label.starts_with('i'))
                .collect::<Vec<_>>()
        );
        let ranked = labels(
            "{@String item}{@String items}{@String itemized}{item<CURSOR>}",
            "<CURSOR>",
        );
        assert_eq!(ranked, ["item", "items", "itemized"]);
    }

    #[test]
    fn emits_snippets_only_for_capable_clients_and_self_closes_leaf_sections() {
        let snippet = items("{#i<CURSOR>", "<CURSOR>", &[], true)
            .into_iter()
            .find(|item| item.label == "if")
            .unwrap();
        assert_eq!(snippet.kind, Some(CompletionItemKind::SNIPPET));
        assert_eq!(
            snippet.insert_text.as_deref(),
            Some("if ${1:condition}}${0}{/if}")
        );
        assert_eq!(snippet.insert_text_format, Some(InsertTextFormat::SNIPPET));

        let plain = items("{#i<CURSOR>", "<CURSOR>", &[], false)
            .into_iter()
            .find(|item| item.label == "include")
            .unwrap();
        assert_eq!(plain.insert_text.as_deref(), Some("include template-id /}"));
        assert_eq!(plain.insert_text_format, None);
        let nested = items("{#n<CURSOR>", "<CURSOR>", &[], true).remove(0);
        assert_eq!(nested.insert_text.as_deref(), Some("nested-content /}"));
        assert!(!nested.insert_text.as_deref().unwrap().contains("{/"));
    }

    #[test]
    fn completes_fragments_and_layout_blocks_from_referenced_templates() {
        let templates = [
            (
                "layouts/base",
                "{#insert header}{/insert}{#if shown}{#insert body /}{/if}",
            ),
            (
                "parts/card",
                "{#fragment primary /}{#capture private /}{#fragment other /}",
            ),
        ];
        assert_eq!(
            items(
                "{#include parts/card$pr<CURSOR>",
                "<CURSOR>",
                &templates,
                false,
            )
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>(),
            ["primary", "private"]
        );
        assert_eq!(
            items(
                "{#include layouts/base}{#b<CURSOR>",
                "<CURSOR>",
                &templates,
                false,
            )
            .into_iter()
            .map(|item| item.label)
            .collect::<Vec<_>>(),
            ["body"]
        );
    }
}
