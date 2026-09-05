//! MCP OAuth Resource Server.
//!
//! Publishes Protected Resource Metadata per RFC 9728 and validates
//! OAuth 2.0 access tokens presented by MCP clients.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::http::HttpState;

/// Claims from an OAuth 2.0 access token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessClaims {
    pub sub: String,
    pub iss: String,
    pub aud: serde_json::Value,
    pub exp: Option<u64>,
}

/// Errors during JWT validation.
#[derive(Debug)]
pub enum TokenError {
    MalformedToken,
    MissingKeyId,
    DisallowedAlgorithm,
    ExpiredToken,
    InvalidAudience,
    InvalidIssuer,
    KeyNotFound(String),
    Validation(jsonwebtoken::errors::Error),
}

/// Validate an OAuth 2.0 access token against the OIDC provider's JWKS.
pub async fn validate_token(
    token: &str,
    cfg: &crate::http::config::HttpConfig,
    jwks: &crate::control::oidc::JwksCache,
) -> Result<AccessClaims, TokenError> {
    let header = decode_header(token).map_err(|_| TokenError::MalformedToken)?;

    let algorithm = match cfg.oidc_allowed_alg.as_str() {
        "RS256" => Algorithm::RS256,
        "ES256" => Algorithm::ES256,
        "EdDSA" => Algorithm::EdDSA,
        _ => return Err(TokenError::DisallowedAlgorithm),
    };

    if header.alg != algorithm {
        return Err(TokenError::DisallowedAlgorithm);
    }

    let kid = header.kid.ok_or(TokenError::MissingKeyId)?;
    let key: DecodingKey = jwks
        .key_for(&kid)
        .await
        .map_err(|e| TokenError::KeyNotFound(format!("JWKS key lookup failed: {e}")))?;

    let mut validator = Validation::new(algorithm);
    if !cfg.oidc_audience.is_empty() {
        validator.set_audience(&[cfg.oidc_audience.as_str()]);
    }
    if !cfg.oidc_issuer.is_empty() {
        validator.set_issuer(&[cfg.oidc_issuer.as_str()]);
    }
    validator.set_required_spec_claims(&["exp", "iss"]);

    let claims = decode::<AccessClaims>(token, &key, &validator)
        .map_err(|e| match e.kind() {
            jsonwebtoken::errors::ErrorKind::ExpiredSignature => TokenError::ExpiredToken,
            jsonwebtoken::errors::ErrorKind::InvalidAudience => TokenError::InvalidAudience,
            jsonwebtoken::errors::ErrorKind::InvalidIssuer => TokenError::InvalidIssuer,
            _ => TokenError::Validation(e),
        })?
        .claims;

    Ok(claims)
}

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
