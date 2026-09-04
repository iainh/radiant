use std::collections::BTreeMap;

use futures_util::FutureExt;

use crate::{
    BoxFuture, ErrorCode, NamespaceContext, NamespaceResolver, RenderError, Resolution, Value,
};

/// A locale-aware, explicitly registered message namespace.
pub struct MessageBundle {
    namespace: String,
    default_locale: String,
    messages: BTreeMap<String, BTreeMap<String, String>>,
}

impl MessageBundle {
    #[must_use]
    pub fn builder(namespace: impl Into<String>) -> MessageBundleBuilder {
        MessageBundleBuilder {
            namespace: namespace.into(),
            default_locale: "en".into(),
            messages: BTreeMap::new(),
        }
    }
}

pub struct MessageBundleBuilder {
    namespace: String,
    default_locale: String,
    messages: BTreeMap<String, BTreeMap<String, String>>,
}

impl MessageBundleBuilder {
    #[must_use]
    pub fn default_locale(mut self, locale: impl Into<String>) -> Self {
        self.default_locale = locale.into();
        self
    }

    #[must_use]
    pub fn message(
        mut self,
        locale: impl Into<String>,
        key: impl Into<String>,
        template: impl Into<String>,
    ) -> Self {
        self.messages
            .entry(locale.into())
            .or_default()
            .insert(key.into(), template.into());
        self
    }

    pub fn build(self) -> Result<MessageBundle, RenderError> {
        if self.namespace.is_empty() || self.default_locale.is_empty() {
            return Err(RenderError::new(
                ErrorCode::Extension,
                "message namespace and default locale must not be empty",
            ));
        }
        for (locale, messages) in &self.messages {
            for (key, template) in messages {
                validate_message(template).map_err(|message| {
                    RenderError::new(
                        ErrorCode::Extension,
                        format!("invalid message `{key}` for locale `{locale}`: {message}"),
                    )
                })?;
            }
        }
        Ok(MessageBundle {
            namespace: self.namespace,
            default_locale: self.default_locale,
            messages: self.messages,
        })
    }
}

impl NamespaceResolver for MessageBundle {
    fn namespace(&self) -> &str {
        &self.namespace
    }

    fn resolve<'a>(
        &'a self,
        context: NamespaceContext<'a>,
    ) -> BoxFuture<'a, Result<Resolution<Value>, RenderError>> {
        async move {
            let locale = context.language.unwrap_or(&self.default_locale);
            let language = locale.split(['-', '_']).next().unwrap_or(locale);
            let template = self
                .messages
                .get(locale)
                .and_then(|messages| messages.get(context.name))
                .or_else(|| {
                    self.messages
                        .get(language)
                        .and_then(|messages| messages.get(context.name))
                })
                .or_else(|| {
                    self.messages
                        .get(&self.default_locale)
                        .and_then(|messages| messages.get(context.name))
                });
            let Some(template) = template else {
                return Ok(Resolution::NotFound);
            };
            let rendered = render_message(template, context.arguments)?;
            Ok(Resolution::Value(Value::String(rendered)))
        }
        .boxed()
    }
}

fn validate_message(template: &str) -> Result<(), &'static str> {
    let mut rest = template;
    while let Some(next) = rest.find(['{', '}']) {
        rest = &rest[next..];
        if rest.starts_with("{{") || rest.starts_with("}}") {
            rest = &rest[2..];
        } else if rest.starts_with('}') {
            return Err("unmatched closing brace; use `}}` for a literal brace");
        } else {
            rest = &rest[1..];
            let Some(close) = rest.find('}') else {
                return Err("unclosed placeholder");
            };
            if rest[..close].parse::<usize>().is_err() {
                return Err("placeholders must be zero-based integers such as `{0}`");
            }
            rest = &rest[close + 1..];
        }
    }
    Ok(())
}

fn render_message(template: &str, arguments: &[Value]) -> Result<String, RenderError> {
    let mut output = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(next) = rest.find(['{', '}']) {
        output.push_str(&rest[..next]);
        rest = &rest[next..];
        if rest.starts_with("{{") {
            output.push('{');
            rest = &rest[2..];
            continue;
        }
        if rest.starts_with("}}") {
            output.push('}');
            rest = &rest[2..];
            continue;
        }
        rest = &rest[1..];
        let close = rest.find('}').ok_or_else(|| {
            RenderError::new(ErrorCode::Extension, "unclosed message placeholder")
        })?;
        let index = rest[..close]
            .parse::<usize>()
            .map_err(|_| RenderError::new(ErrorCode::Extension, "invalid message placeholder"))?;
        let value = arguments.get(index).ok_or_else(|| {
            RenderError::new(
                ErrorCode::Extension,
                format!("message argument {index} is missing"),
            )
        })?;
        output.push_str(&value.plain_text());
        rest = &rest[close + 1..];
    }
    output.push_str(rest);
    Ok(output)
}
