//! Local admin HTTP handlers.
//!
//! Placeholder handlers for local admin authentication and client management.
//! These will be fully implemented when the durable store is wired up.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

fn not_implemented(message: &str) -> Response {
    let body = serde_json::json!({
        "error": {"code": "not_implemented", "message": message},
        "correlation_id": uuid::Uuid::new_v4().to_string(),
    });
    (
        StatusCode::NOT_IMPLEMENTED,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

/// Placeholder: challenge inspection/creation.
pub async fn challenge_handler() -> Response {
    not_implemented("local admin challenge handler not yet wired")
}

/// Placeholder: login/reauth/logout.
pub async fn auth_handler() -> Response {
    not_implemented("local admin auth handler not yet wired")
}

/// Placeholder: list/create clients.
pub async fn clients_handler() -> Response {
    not_implemented("local admin clients handler not yet wired")
}

/// Placeholder: get client.
pub async fn client_handler() -> Response {
    not_implemented("local admin client handler not yet wired")
}

/// Placeholder: list/create keys.
pub async fn keys_handler() -> Response {
    not_implemented("local admin keys handler not yet wired")
}

/// Placeholder: revoke key.
pub async fn revoke_key_handler() -> Response {
    not_implemented("local admin revoke key handler not yet wired")
}

/// Placeholder: suspend/resume client.
pub async fn client_state_handler() -> Response {
    not_implemented("local admin client state handler not yet wired")
}
