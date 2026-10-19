use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("not found")]
    NotFound,
    #[error("{0}")]
    BadRequest(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("{0}")]
    RequestTimeout(String),
    /// An upstream dependency (object storage) failed; the message is client-safe.
    #[error("{0}")]
    BadGateway(String),
    /// A feature this deployment does not have (the search model is not installed); the message is client-safe.
    #[error("{0}")]
    Unavailable(String),
    /// A per-account limit refused the request (HTTP 413 or 429): `code` is stable for clients, `details` is merged into
    /// the body (sizes, a reset time), and `retry_after_secs` becomes `Retry-After`.
    #[error("{message}")]
    Limited {
        status: StatusCode,
        code: &'static str,
        message: String,
        details: serde_json::Value,
        retry_after_secs: Option<u64>,
    },
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl From<sqlx::Error> for ApiError {
    fn from(e: sqlx::Error) -> Self {
        Self::Internal(e.into())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match &self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden(_) => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RequestTimeout(_) => StatusCode::REQUEST_TIMEOUT,
            Self::BadGateway(_) => StatusCode::BAD_GATEWAY,
            Self::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Limited {
                status,
                code,
                message,
                details,
                retry_after_secs,
            } => {
                let mut body = json!({"error": message, "code": code});
                if let (Some(b), Some(d)) = (body.as_object_mut(), details.as_object()) {
                    b.extend(d.clone());
                }
                let mut res = (*status, Json(body)).into_response();
                if let Some(v) = retry_after_secs.and_then(|s| s.to_string().parse().ok()) {
                    res.headers_mut().insert(axum::http::header::RETRY_AFTER, v);
                }
                return res;
            }
            Self::Internal(e) => {
                tracing::error!(error = ?e, "internal error");
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({"error": "internal error"})),
                )
                    .into_response();
            }
        };
        (status, Json(json!({"error": self.to_string()}))).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
