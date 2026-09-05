use radiant_compiler::{Argument, ArgumentValue, BUILT_IN_SECTIONS, Expr, Node, Section, Span};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DeclarationKind {
    Parameter,
    LoopAlias,
    LocalBinding,
}

impl DeclarationKind {
    pub(crate) const fn detail(self) -> &'static str {
        match self {
            Self::Parameter => "parameter",
            Self::LoopAlias => "loop alias",
            Self::LocalBinding => "local binding",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Declaration {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) kind: DeclarationKind,
    pub(crate) parameter_type: Option<String>,
    scope: Span,
    depth: usize,
}

impl Declaration {
    pub(crate) fn detail(&self) -> String {
        self.parameter_type.as_ref().map_or_else(
            || self.kind.detail().into(),
            |type_name| format!("{}: {type_name}", self.kind.detail()),
        )
    }
}

#[derive(Debug)]
pub(crate) struct Construct {
    pub(crate) name: String,
    pub(crate) name_span: Span,
    pub(crate) parent: Option<String>,
}

#[derive(Debug)]
pub(crate) struct Reference {
    pub(crate) name: String,
    pub(crate) span: Span,
}

#[derive(Debug)]
pub(crate) struct FragmentDeclaration {
    pub(crate) name: String,
    pub(crate) name_span: Span,
}

#[derive(Debug)]
pub(crate) enum TemplateReference {
    Include {
        target: String,
        target_span: Span,
        fragment: Option<(String, Span)>,
    },
    Tag {
        target: String,
        span: Span,
    },
}

#[derive(Debug, Default)]
pub(crate) struct SemanticIndex {
    declarations: Vec<Declaration>,
    constructs: Vec<Construct>,
    references: Vec<Reference>,
    fragments: Vec<FragmentDeclaration>,
    template_references: Vec<TemplateReference>,
}

impl SemanticIndex {
    pub(crate) fn new(nodes: &[Node], text: &str) -> Self {
        let mut index = Self::default();
        let document = Span::new(0, text.len().saturating_add(1));
        for node in nodes {
            if let Node::Parameter(parameter) = node {
                index.declarations.push(Declaration {
                    name: parameter.name.clone(),
                    name_span: parameter_name_span(
                        text,
                        parameter.span,
                        &parameter.type_name,
                        &parameter.name,
                    ),
                    kind: DeclarationKind::Parameter,
                    parameter_type: Some(parameter.type_name.clone()),
                    scope: document,
                    depth: 0,
                });
                if let Some(default) = &parameter.default {
                    index.visit_expression(default);
                }
            }
        }
        index.visit_sections(nodes, text, 1);
        index
    }

    pub(crate) fn visible_declarations(&self, cursor: usize) -> Vec<&Declaration> {
        let mut visible = Vec::new();
        for declaration in self
            .declarations
            .iter()
            .filter(|declaration| contains(declaration.scope, cursor))
        {
            if let Some(existing) = visible
                .iter()
                .position(|candidate: &&Declaration| candidate.name == declaration.name)
            {
                if declaration.depth >= visible[existing].depth {
                    visible[existing] = declaration;
                }
            } else {
                visible.push(declaration);
            }
        }
        visible
    }

    pub(crate) fn resolve(&self, name: &str, cursor: usize) -> Option<&Declaration> {
        self.visible_declarations(cursor)
            .into_iter()
            .find(|declaration| declaration.name == name)
    }

    pub(crate) fn declaration_at(&self, cursor: usize) -> Option<&Declaration> {
        self.declarations
            .iter()
            .rev()
            .find(|declaration| contains(declaration.name_span, cursor))
    }

    pub(crate) fn construct_at(&self, cursor: usize) -> Option<&Construct> {
        self.constructs
            .iter()
            .rev()
            .find(|construct| contains(construct.name_span, cursor))
    }

    pub(crate) fn reference_at(&self, cursor: usize) -> Option<&Reference> {
        self.references
            .iter()
            .rev()
            .find(|reference| contains(reference.span, cursor))
    }

    pub(crate) fn references_to(&self, declaration: &Declaration) -> Vec<&Reference> {
        self.references
            .iter()
            .filter(|reference| {
                self.resolve(&reference.name, reference.span.start)
                    .is_some_and(|resolved| resolved.name_span == declaration.name_span)
            })
            .collect()
    }

    pub(crate) fn fragments(&self) -> &[FragmentDeclaration] {
        &self.fragments
    }

    pub(crate) fn fragment_at(&self, cursor: usize) -> Option<&FragmentDeclaration> {
        self.fragments
            .iter()
            .find(|fragment| contains(fragment.name_span, cursor))
    }

    pub(crate) fn template_references(&self) -> &[TemplateReference] {
        &self.template_references
    }

    pub(crate) fn template_reference_at(&self, cursor: usize) -> Option<&TemplateReference> {
        self.template_references
            .iter()
            .find(|reference| match reference {
                TemplateReference::Include {
                    target_span,
                    fragment,
                    ..
                } => {
                    contains(*target_span, cursor)
                        || fragment
                            .as_ref()
                            .is_some_and(|(_, span)| contains(*span, cursor))
                }
                TemplateReference::Tag { span, .. } => contains(*span, cursor),
            })
    }

    fn visit_sections(&mut self, nodes: &[Node], text: &str, depth: usize) {
        for node in nodes {
            if let Node::Output { expression, .. } = node {
                self.visit_expression(expression);
            }
            let Node::Section(section) = node else {
                continue;
            };
            self.constructs.push(Construct {
                name: section.name.clone(),
                name_span: opening_name_span(text, section.span, &section.name),
                parent: None,
            });
            self.add_template_symbol(section, text);
            for argument in &section.arguments {
                self.visit_argument(argument);
            }
            for (index, block) in section.blocks.iter().enumerate() {
                if index > 0 {
                    self.constructs.push(Construct {
                        name: block.name.clone(),
                        name_span: opening_name_span(text, block.span, &block.name),
                        parent: Some(section.name.clone()),
                    });
                }
                for argument in &block.arguments {
                    self.visit_argument(argument);
                }
                let scope = block_body_span(section, index, text);
                if index == 0 {
                    self.add_section_declarations(section, text, scope, depth);
                }
                self.visit_sections(&block.nodes, text, depth + 1);
            }
        }
    }

    fn visit_argument(&mut self, argument: &Argument) {
        if let ArgumentValue::Expression(expression) = &argument.value {
            self.visit_expression(expression);
        }
    }

    fn visit_expression(&mut self, expression: &Expr) {
        match expression {
            Expr::Identifier { name, span } => self.references.push(Reference {
                name: name.clone(),
                span: *span,
            }),
            Expr::Unary { expression, .. } | Expr::Safe { expression, .. } => {
                self.visit_expression(expression);
            }
            Expr::Binary { left, right, .. } => {
                self.visit_expression(left);
                self.visit_expression(right);
            }
            Expr::Member { object, .. } => self.visit_expression(object),
            Expr::Call {
                callee, arguments, ..
            } => {
                self.visit_expression(callee);
                for argument in arguments {
                    self.visit_expression(argument);
                }
            }
            Expr::Index { object, index, .. } => {
                self.visit_expression(object);
                self.visit_expression(index);
            }
            Expr::Literal { .. } | Expr::Namespace { .. } => {}
        }
    }

    fn add_section_declarations(
        &mut self,
        section: &Section,
        text: &str,
        scope: Span,
        depth: usize,
    ) {
        match section.name.as_str() {
            "for" => {
                if let Some(alias) = section
                    .arguments
                    .iter()
                    .find(|argument| argument.name.as_deref() == Some("alias"))
                    .and_then(Argument::static_text)
                {
                    let argument = &section.arguments[0];
                    self.push_declaration(
                        alias,
                        trimmed_value_span(text, argument.span, alias),
                        DeclarationKind::LoopAlias,
                        scope,
                        depth,
                    );
                }
            }
            "each" => {
                self.push_declaration(
                    "it",
                    opening_name_span(text, section.span, &section.name),
                    DeclarationKind::LoopAlias,
                    scope,
                    depth,
                );
            }
            "let" | "set" => {
                for argument in &section.arguments {
                    if let Some(name) = argument.name.as_deref() {
                        self.push_declaration(
                            name,
                            named_argument_span(text, section.span, argument.span, name),
                            DeclarationKind::LocalBinding,
                            scope,
                            depth,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn push_declaration(
        &mut self,
        name: &str,
        name_span: Span,
        kind: DeclarationKind,
        scope: Span,
        depth: usize,
    ) {
        self.declarations.push(Declaration {
            name: name.into(),
            name_span,
            kind,
            parameter_type: None,
            scope,
            depth,
        });
    }

    fn add_template_symbol(&mut self, section: &Section, text: &str) {
        if matches!(section.name.as_str(), "fragment" | "capture") {
            if let Some(argument) = section.arguments.first()
                && let Some(name) = argument.static_text()
            {
                self.fragments.push(FragmentDeclaration {
                    name: name.into(),
                    name_span: static_value_span(text, argument.span, name),
                });
            }
            return;
        }
        if section.name == "include" {
            if section
                .arguments
                .iter()
                .any(|argument| argument.name.as_deref() == Some("_id"))
            {
                return;
            }
            let Some(argument) = section.arguments.first() else {
                return;
            };
            if argument.name.is_some() {
                return;
            }
            let value = match &argument.value {
                ArgumentValue::String(value) | ArgumentValue::Raw(value) => value,
                ArgumentValue::Expression(_) => return,
            };
            if value.starts_with("_id=") {
                return;
            }
            let value_span = static_value_span(text, argument.span, value);
            let (target, fragment) = value.split_once('$').map_or_else(
                || (value.clone(), None),
                |(target, fragment)| {
                    let start = value_span.start + target.len() + 1;
                    (
                        target.into(),
                        Some((fragment.into(), Span::new(start, start + fragment.len()))),
                    )
                },
            );
            self.template_references.push(TemplateReference::Include {
                target_span: Span::new(value_span.start, value_span.start + target.len()),
                target,
                fragment,
            });
        } else if !BUILT_IN_SECTIONS.contains(&section.name.as_str()) {
            self.template_references.push(TemplateReference::Tag {
                target: format!("tags/{}", section.name),
                span: opening_name_span(text, section.span, &section.name),
            });
        }
    }
}

fn block_body_span(section: &Section, index: usize, text: &str) -> Span {
    let block = &section.blocks[index];
    let start = opening_end(text, block.span).unwrap_or(block.span.start);
    let mut end = section.blocks.get(index + 1).map_or_else(
        || closing_start(section, text).unwrap_or(section.span.end),
        |next| next.span.start,
    );
    if end == text.len() && closing_start(section, text).is_none() {
        end = end.saturating_add(1);
    }
    Span::new(start.min(end), end)
}

fn closing_start(section: &Section, text: &str) -> Option<usize> {
    let source = text.get(section.span.start..section.span.end.min(text.len()))?;
    let closing = format!("{{/{}", section.name);
    source.rfind(&closing).map(|at| section.span.start + at)
}

fn opening_end(text: &str, span: Span) -> Option<usize> {
    let source = text.get(span.start..span.end.min(text.len()))?;
    tag_end(source).map(|relative| span.start + relative + 1)
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

pub(crate) fn opening_name_span(text: &str, span: Span, name: &str) -> Span {
    let source = &text[span.start..span.end.min(text.len())];
    source
        .find('#')
        .and_then(|hash| source[hash + 1..].find(name).map(|at| hash + 1 + at))
        .map_or(span, |at| {
            Span::new(span.start + at, span.start + at + name.len())
        })
}

fn parameter_name_span(text: &str, span: Span, type_name: &str, name: &str) -> Span {
    let source = &text[span.start..span.end.min(text.len())];
    source
        .find(type_name)
        .and_then(|type_at| {
            source[type_at + type_name.len()..]
                .find(name)
                .map(|name_at| type_at + type_name.len() + name_at)
        })
        .map_or(span, |at| {
            Span::new(span.start + at, span.start + at + name.len())
        })
}

fn trimmed_value_span(text: &str, span: Span, value: &str) -> Span {
    text[span.start..span.end.min(text.len())]
        .find(value)
        .map_or(span, |at| {
            Span::new(span.start + at, span.start + at + value.len())
        })
}

pub(crate) fn static_value_span(text: &str, span: Span, value: &str) -> Span {
    trimmed_value_span(text, span, value)
}

fn named_argument_span(text: &str, section: Span, value: Span, name: &str) -> Span {
    text[section.start..value.start.min(text.len())]
        .rfind(name)
        .map_or(value, |at| {
            Span::new(section.start + at, section.start + at + name.len())
        })
}

const fn contains(span: Span, cursor: usize) -> bool {
    span.start <= cursor && cursor < span.end
}

#[cfg(test)]
mod tests {
    use radiant_compiler::analyze;

    use super::{DeclarationKind, SemanticIndex};

    fn index(source: &str) -> SemanticIndex {
        SemanticIndex::new(&analyze("test", source).template.nodes, source)
    }

    #[test]
    fn resolves_shadowing_and_isolates_sibling_blocks() {
        let source = "{@String item}{#for item in items}{item}{#else}{item}{/for}";
        let semantic = index(source);
        let primary_use = source.find("{item}").unwrap() + 1;
        let sibling_use = source.rfind("{item}").unwrap() + 1;

        assert_eq!(
            semantic.resolve("item", primary_use).unwrap().kind,
            DeclarationKind::LoopAlias
        );
        assert_eq!(
            semantic.resolve("item", sibling_use).unwrap().kind,
            DeclarationKind::Parameter
        );
    }

    #[test]
    fn records_exact_unicode_declaration_name_spans_and_scope_boundaries() {
        let source = "😀{@String café=café}{#let café=café}{café}{/let}{café}";
        let semantic = index(source);
        let parameter = semantic
            .declarations
            .iter()
            .find(|declaration| declaration.kind == DeclarationKind::Parameter)
            .unwrap();
        let local = semantic
            .declarations
            .iter()
            .find(|declaration| declaration.kind == DeclarationKind::LocalBinding)
            .unwrap();

        assert_eq!(parameter.name_span.start, source.find("café").unwrap());
        assert_eq!(&source[local.name_span.start..local.name_span.end], "café");
        assert_eq!(
            semantic
                .resolve("café", source.find("{café}").unwrap() + 1)
                .unwrap()
                .kind,
            DeclarationKind::LocalBinding
        );
        assert_eq!(
            semantic
                .resolve("café", source.rfind("{café}").unwrap() + 1)
                .unwrap()
                .kind,
            DeclarationKind::Parameter
        );
    }
}
