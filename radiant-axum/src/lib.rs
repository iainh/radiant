//! Axum state extraction, negotiation, and response conversion for Radiant.

use std::{convert::Infallible, io, time::Duration};

use axum::{
    body::Body,
    extract::{FromRef, FromRequestParts},
    http::{
        HeaderValue, StatusCode,
        header::{ACCEPT, ACCEPT_LANGUAGE, CONTENT_LANGUAGE, CONTENT_TYPE},
        request::Parts,
    },
    response::{IntoResponse, Response},
};
use futures_util::stream;
use radiant::{Engine, MediaType, RenderError, Rendered, Template};

#[derive(Debug, Clone, Copy)]
pub struct RenderDeadline(pub Duration);

#[derive(Clone)]
pub struct Renderer {
    engine: Engine,
    media_type: Option<MediaType>,
    acceptable: bool,
    language: Option<String>,
    deadline: Option<Duration>,
}

impl<S> FromRequestParts<S> for Renderer
where
    Engine: FromRef<S>,
    S: Send + Sync,
{
    type Rejection = Infallible;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let (media_type, acceptable) = parts.headers.get(ACCEPT).map_or((None, true), |value| {
            negotiate_media_type(value).map_or((None, false), |media_type| (media_type, true))
        });
        let language = parts
            .headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok())
            .and_then(negotiate_language);
        let deadline = parts
            .extensions
            .get::<RenderDeadline>()
            .map(|deadline| deadline.0);
        Ok(Self {
            engine: Engine::from_ref(state),
            media_type,
            acceptable,
            language,
            deadline,
        })
    }
}

impl Renderer {
    pub async fn render<T: Template>(
        self,
        template: T,
    ) -> Result<TemplateResponse, RenderRejection> {
        if !self.acceptable {
            return Err(RenderError::new(
                radiant::ErrorCode::NotAcceptable,
                "the Accept header has no supported media type",
            )
            .into());
        }
        let mut render = self.engine.render_with(template);
        if let Some(media_type) = self.media_type {
            render = render.variant(media_type);
        }
        if let Some(language) = self.language {
            render = render.locale(language);
        }
        let rendered = if let Some(deadline) = self.deadline {
            tokio::time::timeout(deadline, render.render())
                .await
                .map_err(|_| RenderRejection::deadline(deadline))??
        } else {
            render.render().await?
        };
        Ok(TemplateResponse(rendered))
    }

    /// Starts rendering after Axum converts the value into a response body.
    ///
    /// A rendering error after headers are sent terminates the body; use
    /// [`Renderer::render`] when a clean error response is required.
    #[must_use]
    pub fn stream<T>(self, template: T) -> StreamResponse<T>
    where
        T: Template + 'static,
    {
        StreamResponse {
            renderer: self,
            template,
        }
    }
}

pub struct TemplateResponse(pub Rendered);

impl IntoResponse for TemplateResponse {
    fn into_response(self) -> Response {
        let content_type = self.0.content_type();
        let language = self.0.language().map(str::to_owned);
        let mut response = self.0.into_bytes().into_response();
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        if let Some(language) = language
            && let Ok(value) = HeaderValue::from_str(&language)
        {
            response.headers_mut().insert(CONTENT_LANGUAGE, value);
        }
        response
    }
}

pub struct StreamResponse<T> {
    renderer: Renderer,
    template: T,
}

impl<T> IntoResponse for StreamResponse<T>
where
    T: Template + 'static,
{
    fn into_response(self) -> Response {
        if !self.renderer.acceptable {
            return RenderRejection::from(RenderError::new(
                radiant::ErrorCode::NotAcceptable,
                "the Accept header has no supported media type",
            ))
            .into_response();
        }
        let content_type = self
            .renderer
            .media_type
            .unwrap_or_else(|| MediaType::from_template_id(T::ID))
            .content_type();
        let future = async move {
            self.renderer
                .render(self.template)
                .await
                .map(|response| response.0.into_bytes())
                .map_err(|error| io::Error::other(error.to_string()))
        };
        let body = Body::from_stream(stream::once(future));
        let mut response = Response::new(body);
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(content_type));
        response
    }
}

#[derive(Debug)]
pub struct RenderRejection {
    error: Option<Box<RenderError>>,
    deadline: Option<Duration>,
}

impl RenderRejection {
    fn deadline(deadline: Duration) -> Self {
        Self {
            error: None,
            deadline: Some(deadline),
        }
    }

    #[must_use]
    pub fn error(&self) -> Option<&RenderError> {
        self.error.as_deref()
    }
}

impl From<RenderError> for RenderRejection {
    fn from(error: RenderError) -> Self {
        Self {
            error: Some(Box::new(error)),
            deadline: None,
        }
    }
}

impl std::fmt::Display for RenderRejection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(error) = &self.error {
            error.fmt(formatter)
        } else if let Some(deadline) = self.deadline {
            write!(formatter, "template rendering exceeded {deadline:?}")
        } else {
            formatter.write_str("template rendering failed")
        }
    }
}

impl std::error::Error for RenderRejection {}

impl IntoResponse for RenderRejection {
    fn into_response(self) -> Response {
        tracing::error!(error = %self, "template rendering failed");
        let status = if self
            .error
            .as_deref()
            .is_some_and(|error| error.code == radiant::ErrorCode::NotAcceptable)
        {
            StatusCode::NOT_ACCEPTABLE
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let message = if status == StatusCode::NOT_ACCEPTABLE {
            "Not Acceptable"
        } else {
            "Internal Server Error"
        };
        (status, message).into_response()
    }
}

fn negotiate_media_type(value: &HeaderValue) -> Result<Option<MediaType>, ()> {
    let value = value.to_str().map_err(|_| ())?;
    let mut wildcard = false;
    let mut choices = value.split(',').filter_map(|choice| {
        let mut parts = choice.trim().split(';');
        let media = parts.next()?.trim();
        let quality = parts
            .find_map(|parameter| parameter.trim().strip_prefix("q="))
            .and_then(|value| value.parse::<f32>().ok())
            .unwrap_or(1.0);
        if quality <= 0.0 {
            return None;
        }
        let media_type = match media {
            "text/html" | "application/xhtml+xml" => MediaType::Html,
            "application/json" => MediaType::Json,
            "application/xml" | "text/xml" => MediaType::Xml,
            "text/plain" => MediaType::Text,
            "*/*" | "text/*" => {
                wildcard = true;
                return None;
            }
            _ => return None,
        };
        Some((quality, media_type))
    });
    let selected = choices
        .next()
        .into_iter()
        .chain(choices)
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, media_type)| media_type);
    if selected.is_some() || wildcard {
        Ok(selected)
    } else {
        Err(())
    }
}

fn negotiate_language(value: &str) -> Option<String> {
    value
        .split(',')
        .filter_map(|choice| {
            let mut parts = choice.trim().split(';');
            let language = parts.next()?.trim();
            let quality = parts
                .find_map(|parameter| parameter.trim().strip_prefix("q="))
                .and_then(|quality| quality.parse::<f32>().ok())
                .unwrap_or(1.0);
            (!language.is_empty() && language != "*" && quality > 0.0)
                .then_some((quality, language))
        })
        .max_by(|left, right| left.0.total_cmp(&right.0))
        .map(|(_, language)| language.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, http::Request, routing::get};
    use http_body_util::BodyExt;
    use radiant::{EmbeddedSource, Value};
    use tower::ServiceExt;

    struct Page;

    impl Template for Page {
        const ID: &'static str = "page";

        fn data(&self) -> Value {
            Value::Map(Default::default())
        }

        fn sources() -> &'static [EmbeddedSource] {
            &[
                EmbeddedSource {
                    id: "page.html",
                    source: "<p>HTML</p>",
                },
                EmbeddedSource {
                    id: "page.txt",
                    source: "plain",
                },
            ]
        }
    }

    async fn page(renderer: Renderer) -> Result<TemplateResponse, RenderRejection> {
        renderer.render(Page).await
    }

    #[test]
    fn negotiation_honours_quality() {
        let value = HeaderValue::from_static("text/plain;q=0.2, text/html;q=0.9");
        assert_eq!(negotiate_media_type(&value), Ok(Some(MediaType::Html)));
        assert_eq!(
            negotiate_media_type(&HeaderValue::from_static("*/*")),
            Ok(None)
        );
        assert_eq!(
            negotiate_media_type(&HeaderValue::from_static("image/png")),
            Err(())
        );
        assert_eq!(
            negotiate_language("en;q=0.4, fr-CA;q=0.9"),
            Some("fr-CA".into())
        );
    }

    #[tokio::test]
    async fn renderer_negotiates_and_sets_response_metadata() {
        let app = Router::new()
            .route("/", get(page))
            .with_state(Engine::new().unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "text/html")
                    .header(ACCEPT_LANGUAGE, "fr-CA, en;q=0.8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[CONTENT_TYPE], "text/html; charset=utf-8");
        assert_eq!(response.headers()[CONTENT_LANGUAGE], "fr-CA");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(body, "<p>HTML</p>");
    }

    #[tokio::test]
    async fn unavailable_variant_returns_not_acceptable() {
        let app = Router::new()
            .route("/", get(page))
            .with_state(Engine::new().unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "application/json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    }

    #[tokio::test]
    async fn unsupported_accept_header_returns_not_acceptable() {
        let app = Router::new()
            .route("/", get(page))
            .with_state(Engine::new().unwrap());
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/")
                    .header(ACCEPT, "image/png")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    }
}
