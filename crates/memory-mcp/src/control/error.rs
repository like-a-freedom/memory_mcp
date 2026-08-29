//! Control-plane API error (ADR-0052, plan §4.7).
//!
//! Phase 10 extends the variants; the HTTP mapping stays in this
//! one place. The Internal(MemoryError) variant's body is generic
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
        let (status, message) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::NotFound => (StatusCode::NOT_FOUND, "not found"),
            ApiError::Conflict => (StatusCode::CONFLICT, "conflict"),
            ApiError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, "temporarily unavailable"),
            ApiError::ReauthRequired => {
                (StatusCode::UNAUTHORIZED, "recent authentication required")
            }
            // Internal details are logged server-side; the
            // response body stays generic (no storage/error-shape
            // leak).
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal error"),
        };
        (status, message).into_response()
    }
}

impl From<MemoryError> for ApiError {
    fn from(err: MemoryError) -> Self {
        ApiError::Internal(err)
    }
}
