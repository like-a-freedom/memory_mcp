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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    /// A current-thread runtime; none of these response bodies block.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("current-thread runtime")
    }

    /// Render an `ApiError` through `IntoResponse` and read back its status.
    async fn status_of(error: ApiError) -> StatusCode {
        error.into_response().status()
    }

    /// Render an `ApiError` and return its JSON `error.code` string.
    async fn code_of(error: ApiError) -> String {
        let body = error.into_response().into_body();
        let bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .expect("error body reads");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("error body is JSON");
        value["error"]["code"]
            .as_str()
            .expect("error code is a string")
            .to_string()
    }

    #[test]
    fn unauthorized_maps_to_401() {
        let observed = runtime().block_on(status_of(ApiError::Unauthorized));

        assert_eq!(observed, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn forbidden_maps_to_403() {
        let observed = runtime().block_on(status_of(ApiError::Forbidden));

        assert_eq!(observed, StatusCode::FORBIDDEN);
    }

    #[test]
    fn not_found_maps_to_404() {
        let observed = runtime().block_on(status_of(ApiError::NotFound));

        assert_eq!(observed, StatusCode::NOT_FOUND);
    }

    #[test]
    fn conflict_maps_to_409() {
        let observed = runtime().block_on(status_of(ApiError::Conflict));

        assert_eq!(observed, StatusCode::CONFLICT);
    }

    #[test]
    fn unavailable_maps_to_503() {
        let observed = runtime().block_on(status_of(ApiError::Unavailable));

        assert_eq!(observed, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn reauth_required_maps_to_401() {
        let observed = runtime().block_on(status_of(ApiError::ReauthRequired));

        assert_eq!(observed, StatusCode::UNAUTHORIZED);
    }

    #[test]
    fn reauth_required_is_distinguishable_from_plain_unauthorized_by_code() {
        let observed = runtime().block_on(code_of(ApiError::ReauthRequired));

        assert_eq!(observed, "recent_auth_required");
    }

    #[test]
    fn unavailable_reports_the_temporarily_unavailable_code() {
        let observed = runtime().block_on(code_of(ApiError::Unavailable));

        assert_eq!(observed, "temporarily_unavailable");
    }

    #[test]
    fn internal_maps_to_500() {
        let observed = runtime().block_on(status_of(ApiError::Internal(MemoryError::Storage(
            "disk on fire".into(),
        ))));

        assert_eq!(observed, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn internal_body_never_echoes_the_underlying_error() {
        let observed = runtime().block_on(code_of(ApiError::Internal(MemoryError::Storage(
            "postgres://user:hunter2@internal-db/memory".into(),
        ))));

        assert_eq!(
            observed, "internal_error",
            "a storage error must not leak its shape into the response"
        );
    }

    #[test]
    fn not_found_error_converts_to_not_found_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::NotFound("tenant".into())),
            ApiError::NotFound
        );

        assert!(observed, "a missing record must surface as 404");
    }

    #[test]
    fn conflict_error_converts_to_conflict_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::Conflict("generation".into())),
            ApiError::Conflict
        );

        assert!(observed, "a state conflict must surface as 409");
    }

    #[test]
    fn unavailable_error_converts_to_unavailable_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::Unavailable("draining".into())),
            ApiError::Unavailable
        );

        assert!(observed, "a draining tenant must surface as 503");
    }

    #[test]
    fn transient_error_converts_to_unavailable_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::Transient("write conflict".into())),
            ApiError::Unavailable
        );

        assert!(observed, "a transient error is retryable, so 503 not 500");
    }

    #[test]
    fn validation_error_converts_to_internal_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::Validation("bad payload".into())),
            ApiError::Internal(_)
        );

        assert!(
            observed,
            "a validation error is a server-side contract breach"
        );
    }

    #[test]
    fn config_invalid_error_converts_to_internal_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::ConfigInvalid("bad key length".into())),
            ApiError::Internal(_)
        );

        assert!(observed, "a config error is a server-side contract breach");
    }

    #[test]
    fn config_missing_error_converts_to_internal_api_error() {
        let observed = matches!(
            ApiError::from(MemoryError::ConfigMissing("SESSION_KEY".into())),
            ApiError::Internal(_)
        );

        assert!(observed, "the fallthrough arm must still reach Internal");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn auth_error_malformed_token_converts_to_unauthorized() {
        let observed = matches!(
            ApiError::from(super::super::oidc::AuthError::MalformedToken),
            ApiError::Unauthorized
        );

        assert!(observed, "a malformed token is a caller credential fault");
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn auth_error_jwks_failure_converts_to_unavailable() {
        let observed = matches!(
            ApiError::from(super::super::oidc::AuthError::Jwks("timeout".into())),
            ApiError::Unavailable
        );

        assert!(
            observed,
            "a JWKS outage is the server's fault, not the caller's"
        );
    }

    #[cfg(feature = "control-plane")]
    #[test]
    fn auth_error_sealing_converts_to_internal_api_error() {
        let observed = matches!(
            ApiError::from(super::super::oidc::AuthError::Sealing),
            ApiError::Internal(_)
        );

        assert!(
            observed,
            "a sealing failure is a server-side contract breach"
        );
    }
}
