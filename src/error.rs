use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Gemini API error: {0}")]
    GeminiApi(String),

    #[error("Gemini returned no candidates")]
    NoCandidates,

    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Rate limited: {0}")]
    RateLimited(String),

    #[error(transparent)]
    Reqwest(#[from] reqwest::Error),

    #[error(transparent)]
    SerdeJson(#[from] serde_json::Error),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            ProxyError::GeminiApi(msg) => {
                let sanitized = if msg.len() > 200 { &msg[..200] } else { msg };
                (StatusCode::BAD_GATEWAY, format!("Upstream error: {sanitized}"))
            }
            ProxyError::NoCandidates => (StatusCode::BAD_GATEWAY, "Gemini returned no candidates".into()),
            ProxyError::Config(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ProxyError::Auth(msg) => (StatusCode::UNAUTHORIZED, msg.clone()),
            ProxyError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            ProxyError::ModelNotFound(model) => (StatusCode::NOT_FOUND, format!("Model not found: {model}")),
            ProxyError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            ProxyError::RateLimited(msg) => (StatusCode::TOO_MANY_REQUESTS, format!("Upstream rate limit: {msg}")),
            ProxyError::Reqwest(e) => (StatusCode::BAD_GATEWAY, format!("Upstream error: {e}")),
            ProxyError::SerdeJson(e) => (StatusCode::BAD_REQUEST, format!("JSON error: {e}")),
        };

        let body = json!({
            "error": {
                "message": message,
                "type": "proxy_error",
                "code": status.as_u16(),
            }
        });

        (status, axum::Json(body)).into_response()
    }
}

pub type Result<T> = std::result::Result<T, ProxyError>;
