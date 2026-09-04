//! A strict, async implementation of Qute-style templates.
//!
//! [`Template`] provides checked, embedded application templates while
//! [`Engine::template`] supports templates registered at runtime. Both paths
//! use the same parser and evaluator.

mod engine;
mod error;
mod escape;
mod messages;
mod template;
mod value;

pub use engine::{
    BoxFuture, Engine, EngineBuilder, EvalContext, FileLoader, NamespaceContext, NamespaceResolver,
    RenderBuilder, RenderOptions, Resolution, TemplateLoader, ValueResolver,
};
pub use error::{ErrorCode, RenderError};
pub use escape::{SafeHtml, SafeJsonString, SafeXml};
pub use messages::{MessageBundle, MessageBundleBuilder};
pub use template::{
    DynamicTemplate, EmbeddedSource, MediaType, Rendered, Template, TemplateInstance,
};
pub use value::{IntoValue, TemplateValue, Value};

pub use radiant_macros::{Template, TemplateValue};

/// Compiler types are exposed for tooling without making the compiler an
/// implementation detail that tools need to duplicate.
pub mod compiler {
    pub use radiant_compiler::*;
}
