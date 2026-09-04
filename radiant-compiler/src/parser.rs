use std::collections::HashSet;

use crate::{
    Argument, ArgumentValue, Block, Diagnostic, Expr, Literal, Node, Parameter, Section, Span,
    Template,
    expr::{make_diag, parse_expression},
};

pub(crate) fn parse_template(name: &str, source: &str) -> Result<Template, Vec<Diagnostic>> {
    let mut parser = Parser {
        name,
        source,
        at: 0,
        diagnostics: Vec::new(),
    };
    let (nodes, stop) = parser.nodes(None, None);
    if let Some(Stop::Close(close, span)) = stop {
        parser.error(
            "E_UNEXPECTED_CLOSE",
            format!("unexpected closing tag `{close}`"),
            span,
        );
    }
    let template = Template {
        name: name.into(),
        source: source.into(),
        nodes,
    };
    validate_into(&template, &mut parser.diagnostics);
    if parser.diagnostics.is_empty() {
        Ok(template)
    } else {
        Err(parser.diagnostics)
    }
}

enum Stop {
    Close(String, Span),
    Block(Tag),
}
#[derive(Clone)]
struct Tag {
    name: String,
    rest: String,
    inner_start: usize,
    span: Span,
    self_closing: bool,
}

struct Parser<'a> {
    name: &'a str,
    source: &'a str,
    at: usize,
    diagnostics: Vec<Diagnostic>,
}
impl Parser<'_> {
    fn error(&mut self, code: &'static str, message: impl Into<String>, span: Span) {
        self.diagnostics
            .push(make_diag(self.name, self.source, code, message, span));
    }

    fn nodes(&mut self, expected: Option<&str>, parent: Option<&str>) -> (Vec<Node>, Option<Stop>) {
        let mut nodes = Vec::new();
        while self.at < self.source.len() {
            let Some(relative) = self.source[self.at..].find('{') else {
                self.push_text(&mut nodes, self.at, self.source.len());
                self.at = self.source.len();
                break;
            };
            let start = self.at + relative;
            if start > self.at && self.source.as_bytes()[start - 1] == b'\\' {
                self.push_text(&mut nodes, self.at, start + 1);
                self.at = start + 1;
                continue;
            }
            self.push_text(&mut nodes, self.at, start);
            if self.source[start..].starts_with("{!") {
                let Some(end_rel) = self.source[start + 2..].find("!}") else {
                    self.error(
                        "E_UNCLOSED_COMMENT",
                        "unterminated comment",
                        Span::new(start, self.source.len()),
                    );
                    self.at = self.source.len();
                    break;
                };
                let end = start + 2 + end_rel + 2;
                nodes.push(Node::Comment {
                    value: self.source[start + 2..end - 2].into(),
                    span: Span::new(start, end),
                });
                self.at = end;
                continue;
            }
            if self.source[start..].starts_with("{|") {
                let pipe_count = self.source[start + 1..]
                    .bytes()
                    .take_while(|byte| *byte == b'|')
                    .count();
                let content_start = start + 1 + pipe_count;
                let delimiter = format!("{}{}", "|".repeat(pipe_count), "}");
                let Some(end_rel) = self.source[content_start..].find(&delimiter) else {
                    self.error(
                        "E_UNCLOSED_UNPARSED",
                        "unterminated unparsed block",
                        Span::new(start, self.source.len()),
                    );
                    self.at = self.source.len();
                    break;
                };
                let content_end = content_start + end_rel;
                let end = content_end + delimiter.len();
                nodes.push(Node::Unparsed {
                    value: self.source[content_start..content_end].into(),
                    span: Span::new(start, end),
                });
                self.at = end;
                continue;
            }
            let Some(end) = tag_end(self.source, start + 1) else {
                self.error(
                    "E_UNCLOSED_TAG",
                    "unterminated tag",
                    Span::new(start, self.source.len()),
                );
                self.at = self.source.len();
                break;
            };
            let span = Span::new(start, end + 1);
            let raw = &self.source[start + 1..end];
            self.at = end + 1;
            let trimmed = raw.trim();
            if let Some(close) = trimmed.strip_prefix('/') {
                let close = close.trim().to_string();
                if close.is_empty() || expected == Some(close.as_str()) {
                    return (nodes, Some(Stop::Close(close, span)));
                }
                self.error(
                    "E_MISMATCHED_CLOSE",
                    format!(
                        "expected `{{/{}}}`, found `{{/{close}}}`",
                        expected.unwrap_or("")
                    ),
                    span,
                );
                return (nodes, Some(Stop::Close(close, span)));
            }
            if let Some(open) = trimmed.strip_prefix('#') {
                let tag = self.parse_tag(open, span);
                if parent.is_some_and(|p| is_block(p, &tag.name)) {
                    return (nodes, Some(Stop::Block(tag)));
                }
                nodes.push(Node::Section(self.section(tag)));
                continue;
            }
            if let Some(decl) = trimmed.strip_prefix('@') {
                self.parameter(decl, span, &mut nodes);
                continue;
            }
            let lead = raw.len() - raw.trim_start().len();
            let expr_text = raw.trim();
            match parse_expression(self.name, self.source, expr_text, start + 1 + lead) {
                Ok(expression) => nodes.push(Node::Output { expression, span }),
                Err(error) => self.diagnostics.push(error),
            }
        }
        if let Some(expected) = expected {
            self.error(
                "E_MISSING_CLOSE",
                format!("section `{expected}` is not closed"),
                Span::new(self.source.len(), self.source.len()),
            );
        }
        (nodes, None)
    }

    fn push_text(&self, nodes: &mut Vec<Node>, start: usize, end: usize) {
        if start < end {
            let text = self.source[start..end]
                .replace("\\{", "{")
                .replace("\\}", "}");
            if let Some(Node::Text { value, span }) = nodes.last_mut()
                && span.end == start
            {
                value.push_str(&text);
                span.end = end;
                return;
            }
            nodes.push(Node::Text {
                value: text,
                span: Span::new(start, end),
            })
        }
    }
    fn parse_tag(&self, open: &str, span: Span) -> Tag {
        let mut content = open.trim();
        let self_closing = content.ends_with('/');
        if self_closing {
            content = content[..content.len() - 1].trim_end()
        }
        let split = content.find(char::is_whitespace).unwrap_or(content.len());
        let name = content[..split].to_string();
        let rest = content[split..].trim().to_string();
        let raw_inner = &self.source[span.start + 1..span.end - 1];
        let hash = raw_inner.find('#').unwrap_or(0);
        let inner_start = span.start + 1 + hash + 1 + content.find(&rest).unwrap_or(content.len());
        Tag {
            name,
            rest,
            inner_start,
            span,
            self_closing,
        }
    }

    fn section(&mut self, tag: Tag) -> Section {
        let arguments = self.arguments(&tag.name, &tag.rest, tag.inner_start);
        if tag.self_closing {
            return Section {
                name: tag.name,
                arguments,
                blocks: Vec::new(),
                self_closing: true,
                span: tag.span,
            };
        }
        let name = tag.name;
        let start = tag.span.start;
        let (body, mut stop) = self.nodes(Some(&name), Some(&name));
        let mut blocks = vec![Block {
            name: name.clone(),
            arguments: Vec::new(),
            nodes: body,
            span: tag.span,
        }];
        let mut end = tag.span.end;
        while let Some(Stop::Block(block_tag)) = stop {
            let block_name = block_tag.name.clone();
            let block_args = self.arguments(&block_name, &block_tag.rest, block_tag.inner_start);
            let explicit = name == "include" && block_name != "else";
            let close = if explicit {
                block_name.as_str()
            } else {
                name.as_str()
            };
            let (block_nodes, next) = self.nodes(Some(close), Some(&name));
            end = next_span(&next).map_or(self.at, |s| s.end);
            blocks.push(Block {
                name: block_name,
                arguments: block_args,
                nodes: block_nodes,
                span: Span::new(block_tag.span.start, end),
            });
            if explicit {
                let (continuation, next_stop) = self.nodes(Some(&name), Some(&name));
                if !continuation.is_empty() {
                    blocks[0].nodes.extend(continuation)
                }
                stop = next_stop;
            } else {
                stop = next;
            }
        }
        if let Some(Stop::Close(_, span)) = stop {
            end = span.end
        }
        Section {
            name,
            arguments,
            blocks,
            self_closing: false,
            span: Span::new(start, end),
        }
    }

    fn parameter(&mut self, decl: &str, span: Span, nodes: &mut Vec<Node>) {
        let Some(type_end) = decl.find(char::is_whitespace) else {
            self.error(
                "E_PARAMETER",
                "parameter declaration must be `{@Type name}` or `{@Type name=default}`",
                span,
            );
            return;
        };
        let type_name = &decl[..type_end];
        let binding = decl[type_end..].trim();
        let (name, default) = if let Some(equals) = top_level_equals(binding) {
            let name = binding[..equals].trim();
            let expression = binding[equals + 1..].trim();
            let base = span.start + decl.find(expression).unwrap_or(type_end + equals + 1);
            (name, Some(self.expression_argument(None, expression, base)))
        } else {
            (binding, None)
        };
        if name.is_empty() || name.chars().any(char::is_whitespace) {
            self.error(
                "E_PARAMETER",
                "parameter declaration must be `{@Type name}` or `{@Type name=default}`",
                span,
            );
            return;
        }
        nodes.push(Node::Parameter(Parameter {
            type_name: type_name.into(),
            name: name.into(),
            default: default.and_then(|argument| match argument.value {
                ArgumentValue::Expression(expression) => Some(expression),
                ArgumentValue::String(value) => Some(Expr::Literal {
                    value: Literal::String(value),
                    span: argument.span,
                }),
                ArgumentValue::Raw(_) => None,
            }),
            span,
        }));
    }

    fn arguments(&mut self, section: &str, text: &str, base: usize) -> Vec<Argument> {
        if text.is_empty() {
            return Vec::new();
        }
        if section == "else"
            && let Some(rest) = text.strip_prefix("if")
            && rest.chars().next().is_some_and(char::is_whitespace)
        {
            let condition = rest.trim_start();
            let condition_base = base + text.len() - condition.len();
            return vec![self.expression_argument(Some("if".into()), condition, condition_base)];
        }
        if section == "if" || section == "when" {
            return vec![self.expression_argument(None, text, base)];
        }
        if section == "for"
            && let Some(pos) = find_word(text, "in")
        {
            let alias = text[..pos].trim();
            let expr = text[pos + 2..].trim();
            let expr_base = base + text.find(expr).unwrap_or(0);
            return vec![
                Argument {
                    name: Some("alias".into()),
                    value: ArgumentValue::Raw(alias.into()),
                    span: Span::new(base, base + pos),
                },
                self.expression_argument(Some("in".into()), expr, expr_base),
            ];
        }
        merge_named_arguments(split_arguments(text, base))
            .into_iter()
            .enumerate()
            .map(|(index, (part, span))| {
                if index == 0 && matches!(section, "include" | "fragment" | "capture") {
                    let value = quoted(&part)
                        .map_or_else(|| ArgumentValue::Raw(part), ArgumentValue::String);
                    return Argument {
                        name: None,
                        value,
                        span,
                    };
                }
                if let Some(eq) = top_level_equals(&part) {
                    let key = part[..eq].trim().to_string();
                    let value = part[eq + 1..].trim();
                    let value_base = span.start + part.find(value).unwrap_or(eq + 1);
                    self.expression_argument(Some(key), value, value_base)
                } else {
                    self.expression_argument(None, &part, span.start)
                }
            })
            .collect()
    }
    fn expression_argument(&mut self, name: Option<String>, text: &str, base: usize) -> Argument {
        let span = Span::new(base, base + text.len());
        let value = if let Some(value) = quoted(text) {
            ArgumentValue::String(value)
        } else {
            match parse_expression(self.name, self.source, text, base) {
                Ok(expr) => ArgumentValue::Expression(expr),
                Err(error) => {
                    self.diagnostics.push(error);
                    ArgumentValue::Raw(text.into())
                }
            }
        };
        Argument { name, value, span }
    }
}

fn next_span(stop: &Option<Stop>) -> Option<Span> {
    match stop {
        Some(Stop::Close(_, span)) => Some(*span),
        Some(Stop::Block(tag)) => Some(tag.span),
        None => None,
    }
}
fn is_block(parent: &str, name: &str) -> bool {
    match parent {
        "if" | "for" => name == "else",
        "when" => matches!(name, "else" | "is" | "case"),
        "include" => !matches!(name, "if" | "for" | "when" | "include" | "fragment"),
        _ => false,
    }
}
fn tag_end(source: &str, mut at: usize) -> Option<usize> {
    let mut quote = None;
    while at < source.len() {
        let ch = source[at..].chars().next()?;
        if let Some(q) = quote {
            if ch == '\\' {
                at += ch.len_utf8();
                if at < source.len() {
                    at += source[at..].chars().next()?.len_utf8()
                }
                continue;
            }
            if ch == q {
                quote = None
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch)
        } else if ch == '}' {
            return Some(at);
        }
        at += ch.len_utf8()
    }
    None
}
fn quoted(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    if bytes.len() >= 2
        && (bytes[0] == b'\'' || bytes[0] == b'"')
        && bytes[bytes.len() - 1] == bytes[0]
    {
        Some(text[1..text.len() - 1].into())
    } else {
        None
    }
}
fn find_word(text: &str, word: &str) -> Option<usize> {
    text.char_indices().find_map(|(i, _)| {
        let end = i + word.len();
        (text.get(i..end) == Some(word)
            && text[..i].chars().last().is_none_or(char::is_whitespace)
            && text[end..].chars().next().is_none_or(char::is_whitespace))
        .then_some(i)
    })
}
fn top_level_equals(text: &str) -> Option<usize> {
    let mut depth = 0;
    let mut quote = None;
    for (i, ch) in text.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' | '[' => depth += 1,
            ')' | ']' => depth -= 1,
            '=' if depth == 0 && text.as_bytes().get(i + 1) != Some(&b'=') => return Some(i),
            _ => {}
        }
    }
    None
}
fn split_arguments(text: &str, base: usize) -> Vec<(String, Span)> {
    let mut result = Vec::new();
    let mut start = None;
    let mut depth = 0;
    let mut quote = None;
    for (i, ch) in text.char_indices() {
        if let Some(q) = quote {
            if ch == q {
                quote = None
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                start.get_or_insert(i);
            }
            '(' | '[' => {
                depth += 1;
                start.get_or_insert(i);
            }
            ')' | ']' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(s) = start.take() {
                    result.push((text[s..i].into(), Span::new(base + s, base + i)))
                }
            }
            _ => {
                start.get_or_insert(i);
            }
        }
    }
    if let Some(s) = start {
        result.push((text[s..].into(), Span::new(base + s, base + text.len())))
    }
    result
}

fn merge_named_arguments(arguments: Vec<(String, Span)>) -> Vec<(String, Span)> {
    let mut merged = Vec::with_capacity(arguments.len());
    let mut arguments = arguments.into_iter().peekable();
    while let Some((mut argument, mut span)) = arguments.next() {
        if argument.ends_with('=') && !argument.ends_with("==") {
            if let Some((value, value_span)) = arguments.next() {
                argument.push_str(&value);
                span.end = value_span.end;
            }
        } else if let Some((next, _)) = arguments.peek()
            && next.starts_with('=')
            && !next.starts_with("==")
            && let Some((equals, equals_span)) = arguments.next()
        {
            let needs_value = equals == "=";
            argument.push_str(&equals);
            span.end = equals_span.end;
            if needs_value && let Some((value, value_span)) = arguments.next() {
                argument.push_str(&value);
                span.end = value_span.end;
            }
        }
        merged.push((argument, span));
    }
    merged
}

pub(crate) fn validate_template(template: &Template) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    validate_into(template, &mut diagnostics);
    diagnostics
}

fn validate_into(template: &Template, diagnostics: &mut Vec<Diagnostic>) {
    fn visit(
        template: &Template,
        nodes: &[Node],
        diagnostics: &mut Vec<Diagnostic>,
        fragments: &mut HashSet<String>,
    ) {
        for node in nodes {
            if let Node::Section(section) = node {
                if matches!(
                    section.name.as_str(),
                    "if" | "for" | "when" | "include" | "fragment" | "capture"
                ) && section.arguments.is_empty()
                {
                    diagnostics.push(make_diag(
                        &template.name,
                        &template.source,
                        "E_SECTION_SHAPE",
                        format!("section `{}` requires an argument", section.name),
                        section.span,
                    ));
                }
                let mut names = HashSet::new();
                for arg in &section.arguments {
                    if let Some(name) = &arg.name
                        && name != "alias"
                        && name != "in"
                        && !names.insert(name.clone())
                    {
                        diagnostics.push(make_diag(
                            &template.name,
                            &template.source,
                            "E_DUPLICATE_ARGUMENT",
                            format!("duplicate named argument `{name}`"),
                            arg.span,
                        ));
                    }
                }
                if matches!(section.name.as_str(), "fragment" | "capture")
                    && let Some(id) = section.arguments.first().and_then(Argument::static_text)
                {
                    if !id
                        .chars()
                        .all(|character| character.is_ascii_alphanumeric() || character == '_')
                    {
                        diagnostics.push(make_diag(
                            &template.name,
                            &template.source,
                            "E_FRAGMENT_ID",
                            "fragment IDs may contain only ASCII letters, digits, and underscores",
                            section.arguments[0].span,
                        ));
                    }
                    if !fragments.insert(id.into()) {
                        diagnostics.push(make_diag(
                            &template.name,
                            &template.source,
                            "E_DUPLICATE_FRAGMENT",
                            format!("duplicate fragment `{id}`"),
                            section.span,
                        ));
                    }
                }
                if section.name == "include" {
                    let mut blocks = HashSet::new();
                    for block in section.blocks.iter().skip(1) {
                        if !blocks.insert(&block.name) {
                            diagnostics.push(make_diag(
                                &template.name,
                                &template.source,
                                "E_DUPLICATE_BLOCK",
                                format!("duplicate include block `{}`", block.name),
                                block.span,
                            ));
                        }
                    }
                }
                for block in &section.blocks {
                    visit(template, &block.nodes, diagnostics, fragments)
                }
            }
        }
    }
    visit(template, &template.nodes, diagnostics, &mut HashSet::new());
}
