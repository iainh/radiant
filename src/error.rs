use std::{error::Error, fmt};

use radiant_compiler::{Diagnostic, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ErrorCode {
    Parse,
    MissingTemplate,
    NotAcceptable,
    DuplicateTemplate,
    MissingValue,
    Type,
    Arithmetic,
    UnknownSection,
    IncludeCycle,
    OutputLimit,
    Loader,
    Extension,
}

#[derive(Debug)]
pub struct RenderError {
    pub code: ErrorCode,
    pub message: Box<str>,
    pub template: Option<Box<str>>,
    pub span: Option<Span>,
    pub line: Option<u32>,
    pub column: Option<u32>,
    pub render_stack: Vec<String>,
    source: Option<Box<dyn Error + Send + Sync>>,
}

impl RenderError {
    #[must_use]
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into().into_boxed_str(),
            template: None,
            span: None,
            line: None,
            column: None,
            render_stack: Vec::new(),
            source: None,
        }
    }

    #[must_use]
    pub fn at(mut self, template: &radiant_compiler::Template, span: Span) -> Self {
        let diagnostic = Diagnostic {
            code: "render",
            message: String::new(),
            source_name: template.name.clone(),
            span,
            line: template.source[..span.start.min(template.source.len())]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            column: template.source[..span.start.min(template.source.len())]
                .rsplit_once('\n')
                .map_or_else(
                    || {
                        template.source[..span.start.min(template.source.len())]
                            .chars()
                            .count()
                            + 1
                    },
                    |(_, tail)| tail.chars().count() + 1,
                ),
        };
        self.template = Some(template.name.clone().into_boxed_str());
        self.span = Some(span);
        self.line = u32::try_from(diagnostic.line).ok();
        self.column = u32::try_from(diagnostic.column).ok();
        self
    }

    #[must_use]
    pub fn with_source(mut self, source: Box<dyn Error + Send + Sync>) -> Self {
        self.source = Some(source);
        self
    }
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let (Some(template), Some(line), Some(column)) = (&self.template, self.line, self.column)
        {
            write!(formatter, "{template}:{line}:{column}: ")?;
        }
        write!(formatter, "{}", self.message)?;
        if !self.render_stack.is_empty() {
            write!(
                formatter,
                " (render stack: {})",
                self.render_stack.join(" -> ")
            )?;
        }
        Ok(())
    }
}

impl Error for RenderError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        self.source.as_deref().map(|error| error as _)
    }
}

impl From<Vec<Diagnostic>> for RenderError {
    fn from(diagnostics: Vec<Diagnostic>) -> Self {
        let message = diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let first = diagnostics.first();
        Self {
            code: ErrorCode::Parse,
            message: message.into_boxed_str(),
            template: first.map(|value| value.source_name.clone().into_boxed_str()),
            span: first.map(|value| value.span),
            line: first.and_then(|value| u32::try_from(value.line).ok()),
            column: first.and_then(|value| u32::try_from(value.column).ok()),
            render_stack: Vec::new(),
            source: None,
        }
    }
}
