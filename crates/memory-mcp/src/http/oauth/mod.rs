//! MCP OAuth Resource Server.
//!
//! Publishes Protected Resource Metadata per RFC 9728 and validates
//! OAuth 2.0 access tokens presented by MCP clients.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use serde_json::json;

use crate::http::HttpState;

/// GET /.well-known/oauth-protected-resource
pub async fn protected_resource_metadata(
    State(state): State<Arc<HttpState>>,
) -> Result<
    (
        StatusCode,
        [(axum::http::header::HeaderName, axum::http::HeaderValue); 1],
        String,
    ),
    StatusCode,
> {
    let body = serde_json::to_string(&json!({
        "resource": state.config.public_base_url,
        "authorization_servers": [state.config.oidc_issuer],
        "bearer_methods_supported": ["header"],
        "scopes_supported": ["memory:read", "memory:write"],
    }))
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    Ok((
        StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_static("application/json"),
        )],
        body,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn prm_publishes_resource_and_issuer() {
        let state = crate::http::HttpState::default_for_test().await;
        let (status, _headers, body) = protected_resource_metadata(State(state)).await.unwrap();
        assert_eq!(status, StatusCode::OK);
        let val: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(val["resource"].is_string());
        assert!(val["authorization_servers"].is_array());
        assert_eq!(val["bearer_methods_supported"], json!(["header"]));
        assert!(val["scopes_supported"].is_array());
    }
}
