//! Owned AST, parser, and validation for a Qute-style template language.

mod ast;
mod diagnostic;
mod expr;
mod parser;
mod span;

pub use ast::{Argument, ArgumentValue, Block, Node, Parameter, Section, Template};
pub use diagnostic::Diagnostic;
pub use expr::{BinaryOp, Expr, Literal, UnaryOp};
pub use span::Span;

/// Section names implemented directly by Radiant.
pub const BUILT_IN_SECTIONS: &[&str] = &[
    "if",
    "for",
    "each",
    "let",
    "set",
    "with",
    "when",
    "switch",
    "include",
    "insert",
    "nested-content",
    "fragment",
    "capture",
];

/// Returns the fixed block names accepted by a built-in section.
///
/// Include block names are template-defined and therefore are not returned.
#[must_use]
pub const fn built_in_block_names(section: &str) -> &'static [&'static str] {
    match section.as_bytes() {
        b"if" | b"for" => &["else"],
        b"when" => &["is", "case", "else"],
        _ => &[],
    }
}

/// The best-effort syntax tree and all diagnostics produced while analyzing a template.
#[derive(Debug, Clone, PartialEq)]
pub struct Analysis {
    pub template: Template,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses and validates a template while retaining the syntax tree when errors are present.
pub fn analyze(name: impl AsRef<str>, source: impl AsRef<str>) -> Analysis {
    parser::analyze_template(name.as_ref(), source.as_ref())
}

/// Parses and validates a template. All returned data owns its strings.
pub fn parse(name: impl AsRef<str>, source: impl AsRef<str>) -> Result<Template, Vec<Diagnostic>> {
    let analysis = analyze(name, source);
    if analysis.diagnostics.is_empty() {
        Ok(analysis.template)
    } else {
        Err(analysis.diagnostics)
    }
}

/// Validates a caller-created or modified AST using the compiler's structural rules.
#[must_use]
pub fn validate(template: &Template) -> Vec<Diagnostic> {
    parser::validate_template(template)
}
