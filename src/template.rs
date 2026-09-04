use std::collections::BTreeMap;

use bytes::Bytes;

use crate::{Engine, IntoValue, RenderError, Value};

#[derive(Debug, Clone, Copy)]
pub struct EmbeddedSource {
    pub id: &'static str,
    pub source: &'static str,
}

/// Implemented by the `Template` derive for checked template models.
pub trait Template: Send {
    const ID: &'static str;
    fn data(&self) -> Value;
    fn sources() -> &'static [EmbeddedSource];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaType {
    Html,
    Xml,
    Json,
    Text,
}

impl MediaType {
    #[must_use]
    pub fn from_template_id(id: &str) -> Self {
        if id.ends_with(".html") || id.ends_with(".htm") {
            Self::Html
        } else if id.ends_with(".xml") {
            Self::Xml
        } else if id.ends_with(".json") {
            Self::Json
        } else {
            Self::Text
        }
    }

    #[must_use]
    pub const fn content_type(self) -> &'static str {
        match self {
            Self::Html => "text/html; charset=utf-8",
            Self::Xml => "application/xml; charset=utf-8",
            Self::Json => "application/json; charset=utf-8",
            Self::Text => "text/plain; charset=utf-8",
        }
    }

    #[must_use]
    pub const fn suffix(self) -> &'static str {
        match self {
            Self::Html => ".html",
            Self::Xml => ".xml",
            Self::Json => ".json",
            Self::Text => ".txt",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Rendered {
    bytes: Bytes,
    media_type: MediaType,
    language: Option<String>,
}

impl Rendered {
    pub(crate) fn new(value: String, media_type: MediaType, language: Option<String>) -> Self {
        Self {
            bytes: Bytes::from(value),
            media_type,
            language,
        }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[must_use]
    pub fn into_bytes(self) -> Bytes {
        self.bytes
    }

    #[must_use]
    pub const fn media_type(&self) -> MediaType {
        self.media_type
    }

    #[must_use]
    pub fn content_type(&self) -> &'static str {
        self.media_type.content_type()
    }

    #[must_use]
    pub fn language(&self) -> Option<&str> {
        self.language.as_deref()
    }
}

impl std::fmt::Display for Rendered {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&String::from_utf8_lossy(&self.bytes))
    }
}

#[derive(Clone)]
pub struct DynamicTemplate {
    pub(crate) engine: Engine,
    pub(crate) id: String,
}

impl DynamicTemplate {
    pub fn fragment(mut self, name: &str) -> Result<Self, RenderError> {
        if name.is_empty()
            || !name
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '_')
        {
            return Err(RenderError::new(
                crate::ErrorCode::MissingTemplate,
                format!("invalid fragment name `{name}`"),
            ));
        }
        self.id.push('$');
        self.id.push_str(name);
        Ok(self)
    }

    #[must_use]
    pub fn instance(self) -> TemplateInstance {
        TemplateInstance {
            engine: self.engine,
            id: self.id,
            data: BTreeMap::new(),
            language: None,
            media_type: None,
        }
    }

    pub fn data(self, name: impl Into<String>, value: impl IntoValue) -> TemplateInstance {
        self.instance().data(name, value)
    }
}

pub struct TemplateInstance {
    pub(crate) engine: Engine,
    pub(crate) id: String,
    pub(crate) data: BTreeMap<String, Value>,
    pub(crate) language: Option<String>,
    pub(crate) media_type: Option<MediaType>,
}

impl TemplateInstance {
    #[must_use]
    pub fn data(mut self, name: impl Into<String>, value: impl IntoValue) -> Self {
        self.data.insert(name.into(), value.into_value());
        self
    }

    #[must_use]
    pub fn locale(mut self, language: impl Into<String>) -> Self {
        self.language = Some(language.into());
        self
    }

    #[must_use]
    pub const fn variant(mut self, media_type: MediaType) -> Self {
        self.media_type = Some(media_type);
        self
    }

    pub async fn render(self) -> Result<Rendered, RenderError> {
        let value = Value::Map(self.data);
        self.engine
            .render_dynamic(&self.id, value, self.media_type, self.language)
            .await
    }
}
