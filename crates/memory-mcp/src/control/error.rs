//! Control-plane API error.
//!
//! The HTTP mapping stays in this one place. The Internal(MemoryError)
//! variant's body is generic
//! to avoid leaking storage/error shape; the original is logged
//! server-side by the surrounding middleware.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

use crate::error::MemoryError;

pub enum ApiError {
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Unavailable,
    ReauthRequired,
    Internal(MemoryError),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized", "unauthorized"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden", "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not_found", "not found"),
            ApiError::Conflict => (StatusCode::CONFLICT, "conflict", "conflict"),
            ApiError::Unavailable => (
                StatusCode::SERVICE_UNAVAILABLE,
                "temporarily_unavailable",
                "temporarily unavailable",
            ),
            ApiError::ReauthRequired => (
                StatusCode::UNAUTHORIZED,
                "recent_auth_required",
                "recent authentication required",
            ),
            // Internal details are logged server-side; the response body stays
            // generic and carries only a correlation id for support.
            ApiError::Internal(error) => {
                eprintln!("memory_mcp::control: internal API error: {error}");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error",
                )
            }
        };
        let body = serde_json::json!({
            "error": {"code": code, "message": message},
            "correlation_id": uuid::Uuid::new_v4().to_string(),
        });
        (
            status,
            [(axum::http::header::CONTENT_TYPE, "application/json")],
            body.to_string(),
        )
            .into_response()
    }
}

impl From<MemoryError> for ApiError {
    fn from(err: MemoryError) -> Self {
        match err {
            MemoryError::NotFound(_) => ApiError::NotFound,
            MemoryError::Conflict(_) => ApiError::Conflict,
            MemoryError::Unavailable(_) | MemoryError::Transient(_) => ApiError::Unavailable,
            MemoryError::Validation(_) | MemoryError::ConfigInvalid(_) => ApiError::Internal(err),
            other => ApiError::Internal(other),
        }
    }
}

#[cfg(feature = "control-plane")]
impl From<super::oidc::AuthError> for ApiError {
    fn from(err: super::oidc::AuthError) -> Self {
        match err {
            super::oidc::AuthError::MalformedToken
            | super::oidc::AuthError::MissingKeyId
            | super::oidc::AuthError::DisallowedAlgorithm
            | super::oidc::AuthError::Jwt(_) => ApiError::Unauthorized,
            super::oidc::AuthError::Jwks(_) | super::oidc::AuthError::Provider(_) => {
                ApiError::Unavailable
            }
            super::oidc::AuthError::Sealing => {
                ApiError::Internal(MemoryError::ConfigInvalid(err.to_string()))
            }
        }
    }
}
