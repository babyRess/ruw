//! Framework error types and sanitized response conversion.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Serialize;

/// Convenient result type for ruw handlers and framework internals.
pub type ApiResult<T> = Result<T, ApiError>;

/// Framework-owned API errors that can be converted into public responses.
///
/// The taxonomy stays intentionally small: malformed framework-owned request
/// extraction and client-visible not-found responses are safe to expose, while
/// framework and application failures continue to use the sanitized internal
/// error seam.
#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    /// A framework-owned extractor rejected malformed request input.
    #[error("bad request")]
    BadRequest,
    /// A semantically invalid controller DTO was rejected before mutation.
    #[error("validation failed")]
    ValidationFailed,
    /// A requested resource was not found.
    #[error("resource not found")]
    NotFound,
    /// A framework or application failure that must not leak internal details.
    #[error("internal server error")]
    Internal,
}

impl ApiError {
    /// Convert a framework-owned extractor rejection into the public bad-request seam.
    pub(crate) fn bad_request(extractor: &'static str, category: &'static str) -> Self {
        tracing::debug!(
            error_kind = "bad_request",
            extractor,
            rejection_category = category,
            "request extractor rejected malformed input"
        );
        Self::BadRequest
    }

    /// Convert a missing controller resource into the public not-found seam.
    pub(crate) fn resource_not_found(operation: &'static str) -> Self {
        tracing::debug!(
            error_kind = "not_found",
            operation,
            "resource controller returned no resource for requested id"
        );
        Self::NotFound
    }

    /// Convert missing framework-managed state into the sanitized error seam.
    pub(crate) fn missing_framework_state(resource: &'static str) -> Self {
        tracing::error!(
            error_kind = "framework_state",
            resource,
            "required framework state is not configured"
        );
        Self::Internal
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(error: sea_orm::DbErr) -> Self {
        tracing::error!(
            error_kind = "database",
            error = %error,
            error_debug = ?error,
            "database error mapped to sanitized API problem"
        );
        Self::Internal
    }
}

/// Stable JSON problem response shape returned by framework errors.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct Problem {
    /// Numeric HTTP status code.
    pub status: u16,
    /// Stable machine-readable error code.
    pub error: &'static str,
    /// Stable human-readable, sanitized message.
    pub message: &'static str,
}

impl Problem {
    /// Build the public body for malformed request errors.
    pub const fn bad_request() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST.as_u16(),
            error: "bad_request",
            message: "bad request",
        }
    }

    /// Build the public body for semantic validation failures.
    pub const fn validation_failed() -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY.as_u16(),
            error: "validation_failed",
            message: "validation failed",
        }
    }

    /// Build the public body for not-found errors.
    pub const fn not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND.as_u16(),
            error: "not_found",
            message: "resource not found",
        }
    }

    /// Build the public body for sanitized internal server errors.
    pub const fn internal_server_error() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR.as_u16(),
            error: "internal_server_error",
            message: "internal server error",
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self {
            Self::BadRequest => {
                (StatusCode::BAD_REQUEST, Json(Problem::bad_request())).into_response()
            }
            Self::ValidationFailed => {
                tracing::debug!(
                    error_kind = "validation_failed",
                    "resource controller validation rejected request input"
                );
                (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(Problem::validation_failed()),
                )
                    .into_response()
            }
            Self::NotFound => (StatusCode::NOT_FOUND, Json(Problem::not_found())).into_response(),
            Self::Internal => {
                tracing::error!(error = %self, "internal framework error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(Problem::internal_server_error()),
                )
                    .into_response()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ApiError, Problem};
    use axum::{
        body::to_bytes,
        http::{header::CONTENT_TYPE, StatusCode},
        response::IntoResponse,
    };

    #[test]
    fn problem_builders_use_stable_public_fields() {
        assert_eq!(
            Problem::bad_request(),
            Problem {
                status: 400,
                error: "bad_request",
                message: "bad request",
            }
        );
        assert_eq!(
            Problem::validation_failed(),
            Problem {
                status: 422,
                error: "validation_failed",
                message: "validation failed",
            }
        );
        assert_eq!(
            Problem::not_found(),
            Problem {
                status: 404,
                error: "not_found",
                message: "resource not found",
            }
        );
        assert_eq!(
            Problem::internal_server_error(),
            Problem {
                status: 500,
                error: "internal_server_error",
                message: "internal server error",
            }
        );
    }

    #[tokio::test]
    async fn bad_request_error_response_is_sanitized_json() {
        let response = ApiError::BadRequest.into_response();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            serde_json::json!({
                "status": 400,
                "error": "bad_request",
                "message": "bad request"
            })
        );
    }

    #[tokio::test]
    async fn validation_failed_error_response_is_sanitized_json() {
        let response = ApiError::ValidationFailed.into_response();

        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            serde_json::json!({
                "status": 422,
                "error": "validation_failed",
                "message": "validation failed"
            })
        );
    }

    #[tokio::test]
    async fn not_found_error_response_is_sanitized_json() {
        let response = ApiError::NotFound.into_response();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            serde_json::json!({
                "status": 404,
                "error": "not_found",
                "message": "resource not found"
            })
        );
    }

    #[tokio::test]
    async fn internal_error_response_is_sanitized_json() {
        let response = ApiError::Internal.into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();

        assert_eq!(
            body,
            serde_json::json!({
                "status": 500,
                "error": "internal_server_error",
                "message": "internal server error"
            })
        );
    }

    #[tokio::test]
    async fn database_error_response_redacts_internal_details() {
        let response = ApiError::from(sea_orm::DbErr::Custom(
            "SELECT * FROM secret_table at /tmp/private.sqlite".to_owned(),
        ))
        .into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(
            response
                .headers()
                .get(CONTENT_TYPE)
                .and_then(|value| value.to_str().ok()),
            Some("application/json")
        );

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body_text = String::from_utf8(body.to_vec()).unwrap();

        assert!(!body_text.contains("secret_table"));
        assert!(!body_text.contains("/tmp/private.sqlite"));
        assert!(!body_text.contains("SELECT"));

        let body: serde_json::Value = serde_json::from_str(&body_text).unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "status": 500,
                "error": "internal_server_error",
                "message": "internal server error"
            })
        );
    }
}
