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

/// Parses and validates a template. All returned data owns its strings.
pub fn parse(name: impl AsRef<str>, source: impl AsRef<str>) -> Result<Template, Vec<Diagnostic>> {
    parser::parse_template(name.as_ref(), source.as_ref())
}

/// Validates a caller-created or modified AST using the compiler's structural rules.
#[must_use]
pub fn validate(template: &Template) -> Vec<Diagnostic> {
    parser::validate_template(template)
}
