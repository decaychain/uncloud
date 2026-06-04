use axum::{
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Authentication required")]
    Unauthorized,

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("{0} not found")]
    NotFound(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Database error: {0}")]
    Database(#[from] mongodb::error::Error),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Storage error: {0}")]
    Storage(String),

    #[error("External service error: {0}")]
    ExternalService(String),

    #[error("External service timeout: {0}")]
    ExternalTimeout(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Range not satisfiable")]
    RangeNotSatisfiable(i64),

    #[error("{0}")]
    MethodNotAllowed(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, self.to_string()),
            AppError::Forbidden(_) => (StatusCode::FORBIDDEN, self.to_string()),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, self.to_string()),
            AppError::BadRequest(msg) => {
                tracing::warn!("Bad request: {}", msg);
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            AppError::Conflict(_) => (StatusCode::CONFLICT, self.to_string()),
            AppError::Validation(msg) => {
                tracing::warn!("Validation: {}", msg);
                (StatusCode::BAD_REQUEST, self.to_string())
            }
            AppError::Database(e) => {
                tracing::error!(error = ?e, "Database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database error: {e}"),
                )
            }
            AppError::Internal(msg) => {
                tracing::error!("Internal error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Internal error: {msg}"),
                )
            }
            AppError::Storage(msg) => {
                tracing::error!("Storage error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Storage error: {msg}"),
                )
            }
            AppError::ExternalService(msg) => {
                tracing::warn!("External service error: {}", msg);
                (
                    StatusCode::BAD_GATEWAY,
                    format!("External service error: {msg}"),
                )
            }
            AppError::ExternalTimeout(msg) => {
                tracing::warn!("External service timeout: {}", msg);
                (
                    StatusCode::GATEWAY_TIMEOUT,
                    format!("External service timeout: {msg}"),
                )
            }
            AppError::MethodNotAllowed(_) => (StatusCode::METHOD_NOT_ALLOWED, self.to_string()),
            AppError::RangeNotSatisfiable(total) => {
                let body = Json(json!({ "error": "Range not satisfiable" }));
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{}", total))],
                    body,
                )
                    .into_response();
            }
        };

        tracing::debug!(status = status.as_u16(), error = %message, "Request failed");

        let body = Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    #[test]
    fn range_not_satisfiable_returns_416_with_content_range() {
        let err = AppError::RangeNotSatisfiable(5000);
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);

        let content_range = response
            .headers()
            .get(header::CONTENT_RANGE)
            .expect("missing Content-Range header")
            .to_str()
            .unwrap();
        assert_eq!(content_range, "bytes */5000");
    }

    #[test]
    fn external_service_error_returns_502() {
        let err = AppError::ExternalService("mail provider connection failed".into());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn external_timeout_returns_504() {
        let err = AppError::ExternalTimeout("mail provider timed out".into());
        let response = err.into_response();

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
    }
}
