use std::{error::Error, fmt};

use crate::Span;

/// A source-located parser or validation error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub code: &'static str,
    pub message: String,
    pub source_name: String,
    pub span: Span,
    /// One-based line number.
    pub line: usize,
    /// One-based Unicode-scalar column number.
    pub column: usize,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}:{}:{}: {}: {}",
            self.source_name, self.line, self.column, self.code, self.message
        )
    }
}

impl Error for Diagnostic {}
