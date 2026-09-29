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
    /// The id of the request this error answers, taken from the request's
    /// extensions so the envelope a client reads and the access-log line
    /// describe one operation.
    ///
    /// Carried on the error rather than looked up at render time: `into_response`
    /// has no request to read, and generating a second id here — as this
    /// module used to — produced a `correlation_id` that named nothing in the
    /// logs. `None` means the error was built outside a request, and rendering
    /// mints an id so the field is never empty.
    At {
        error: Box<ApiError>,
        request_id: uuid::Uuid,
    },
}

impl ApiError {
    /// Bind this error to the request it answers.
    ///
    /// A no-op when it is already bound, so a handler that stamps an error
    /// twice does not wrap twice and `From` conversions still work through an
    /// already-stamped error.
    #[must_use]
    pub fn at_request(self, request_id: uuid::Uuid) -> Self {
        match self {
            ApiError::At { error, .. } => ApiError::At { error, request_id },
            error => ApiError::At {
                error: Box::new(error),
                request_id,
            },
        }
    }

    /// The request this error answers, if it was bound to one.
    #[must_use]
    pub fn request_id(&self) -> Option<uuid::Uuid> {
        match self {
            ApiError::At { request_id, .. } => Some(*request_id),
            _ => None,
        }
    }
}

/// Record an internal error through the deployment's logger.
///
/// At `error` level and never below: this is the class of failure a support
/// conversation starts from, and `RUST_LOG=error` is exactly the setting an
/// operator reaches for when something is broken. It used to be an
/// `eprintln!`, which that setting could not suppress and which carried no
/// level, no timestamp and no request id.
fn log_internal_error(detail: &str, request_id: Option<uuid::Uuid>) {
    let mut event = std::collections::HashMap::new();
    event.insert("op".into(), "control.internal_error".into());
    event.insert("error".into(), detail.to_string().into());
    if let Some(id) = request_id {
        event.insert("request_id".into(), id.to_string().into());
    }
    crate::logging::StdoutLogger::from_env().log(event, crate::logging::LogLevel::Error);
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Unwrap first: the `At` wrapper carries the id and nothing else, so
        // every branch below stays a statement about one error.
        let (this, request_id) = match self {
            ApiError::At { error, request_id } => (*error, Some(request_id)),
            other => (other, None),
        };
        let (status, code, message) = match this {
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
            // generic and carries only a correlation id for support. The log
            // goes through the deployment's logger, so it carries the level,
            // the timestamp and the request id — an internal error is the one
            // event a support conversation starts from, and it used to be the
            // one line that could not be joined to the request that caused it.
            ApiError::Internal(error) => {
                log_internal_error(&error.to_string(), request_id);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error",
                )
            }
            // Unreachable: the wrapper was unwrapped above. Rendered as a 500
            // rather than panicking in a request path.
            ApiError::At { .. } => {
                log_internal_error("nested request binding", request_id);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "internal error",
                )
            }
        };
        let body = serde_json::json!({
            "error": {"code": code, "message": message},
            // The request's own id when the error was bound to one, so the
            // value a client quotes matches the access-log line; a fresh id
            // only when the error was built outside a request, where there is
            // nothing to match against.
            "correlation_id": request_id.unwrap_or_else(uuid::Uuid::new_v4).to_string(),
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
            | super::oidc::AuthError::DisallowedAlgorithm { .. }
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

    /// The `correlation_id` a client reads must be the one the server logged
    /// under. It is minted once per request and reaches both places from the
    /// request's extensions; a second, independent id in the error renderer
    /// would make every support request — "what do you see in the logs for
    /// this id?" — unanswerable.
    #[test]
    fn the_error_envelope_carries_the_requests_own_id() {
        let request_id = uuid::Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();
        let response = ApiError::Unauthorized
            .at_request(request_id)
            .into_response();
        let bytes = runtime()
            .block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
            .expect("body reads");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        assert_eq!(value["correlation_id"], request_id.to_string());
    }

    /// Every branch carries an id, including the ones with no request behind
    /// them: an error built outside a request still has to name itself.
    #[test]
    fn an_unstamped_error_still_reports_an_id() {
        let response = ApiError::Forbidden.into_response();
        let bytes = runtime()
            .block_on(axum::body::to_bytes(response.into_body(), usize::MAX))
            .expect("body reads");
        let value: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON");
        let id = value["correlation_id"].as_str().expect("an id is present");
        assert!(!id.is_empty(), "an unstamped error must still be traceable");
        uuid::Uuid::parse_str(id).expect("the id is a uuid");
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
