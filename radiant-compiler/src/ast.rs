use crate::{Expr, Span};

#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub source: String,
    pub nodes: Vec<Node>,
}

impl Template {
    /// Returns unique, statically-known include template IDs in source order.
    #[must_use]
    pub fn dependencies(&self) -> Vec<&str> {
        fn visit<'a>(nodes: &'a [Node], out: &mut Vec<&'a str>) {
            for node in nodes {
                if let Node::Section(section) = node {
                    if section.name == "include"
                        && let Some(id) = section.arguments.first().and_then(Argument::static_text)
                        && !out.contains(&id)
                    {
                        out.push(id);
                    }
                    for block in &section.blocks {
                        visit(&block.nodes, out);
                    }
                }
            }
        }
        let mut result = Vec::new();
        visit(&self.nodes, &mut result);
        result
    }

    /// Returns all fragment sections, including nested declarations.
    #[must_use]
    pub fn fragments(&self) -> Vec<&Section> {
        fn visit<'a>(nodes: &'a [Node], out: &mut Vec<&'a Section>) {
            for node in nodes {
                if let Node::Section(section) = node {
                    if matches!(section.name.as_str(), "fragment" | "capture") {
                        out.push(section);
                    }
                    for block in &section.blocks {
                        visit(&block.nodes, out);
                    }
                }
            }
        }
        let mut result = Vec::new();
        visit(&self.nodes, &mut result);
        result
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    Text { value: String, span: Span },
    Unparsed { value: String, span: Span },
    Comment { value: String, span: Span },
    Output { expression: Expr, span: Span },
    Parameter(Parameter),
    Section(Section),
}

impl Node {
    #[must_use]
    pub const fn span(&self) -> Span {
        match self {
            Self::Text { span, .. }
            | Self::Unparsed { span, .. }
            | Self::Comment { span, .. }
            | Self::Output { span, .. } => *span,
            Self::Parameter(value) => value.span,
            Self::Section(value) => value.span,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Parameter {
    pub type_name: String,
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub name: String,
    pub arguments: Vec<Argument>,
    /// The first block is the section body and has the section's name.
    pub blocks: Vec<Block>,
    pub self_closing: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub name: String,
    pub arguments: Vec<Argument>,
    pub nodes: Vec<Node>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Argument {
    pub name: Option<String>,
    pub value: ArgumentValue,
    pub span: Span,
}

impl Argument {
    #[must_use]
    pub fn static_text(&self) -> Option<&str> {
        match &self.value {
            ArgumentValue::String(value) | ArgumentValue::Raw(value) => Some(value),
            ArgumentValue::Expression(Expr::Identifier { name, .. }) => Some(name),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ArgumentValue {
    Expression(Expr),
    String(String),
    Raw(String),
}
