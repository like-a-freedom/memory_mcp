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

/// Resolve an OAuth 2.0 access token to a tenant Account.
///
/// Validates the token against the OIDC provider's JWKS, derives the
/// `(issuer, subject_verifier)` blind index exactly as the login flow does,
/// and returns the Account that index names — but only while it is Active.
/// Every failure mode (bad signature, wrong issuer/audience, expired, unknown
/// identity, suspended Account) collapses to `None`, so a caller cannot tell
/// them apart.
pub async fn resolve_bearer_principal(
    config: &crate::http::config::HttpConfig,
    accounts: &dyn crate::http::registry::storage::AccountStore,
    jwks: &crate::control::oidc::JwksCache,
    token: &str,
) -> Option<crate::http::principal::AuthenticatedPrincipal> {
    let claims = validate_token(token, config, jwks).await.ok()?;
    let verifier = crate::control::oidc::identity_subject_verifier(
        &config.keys.identity_index,
        &claims.iss,
        &claims.sub,
    )
    .ok()?;
    let account = accounts
        .find_account_by_identity(
            &claims.iss,
            &crate::http::registry::models::SubjectVerifier(verifier),
        )
        .await
        .ok()
        .flatten()?;
    if account.status != crate::http::registry::models::AccountStatus::Active {
        return None;
    }
    Some(crate::http::principal::AuthenticatedPrincipal::Oidc {
        account: Arc::new(account),
        issuer: claims.iss,
        subject: claims.sub,
    })
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
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::time::Duration;

    use jsonwebtoken::{EncodingKey, Header, encode};

    /// A real 2048-bit RSA key pair, so signatures and JWKS decoding are
    /// exercised for real rather than against a stubbed verifier.
    const RSA_PRIVATE_PEM: &str = include_str!("../../tests/fixtures/oauth_test_rsa_key.pem");
    /// The matching public modulus, base64url-encoded as JWKS requires.
    const RSA_N: &str = "sWmwOq_tLQb96bfMzWSNEjJRQhd2ZNcZGVBHdGfBu9LJg36Y4tJ62GNLbnMDtQBjiumnN8owIeIOZjypySedttnX6MfvQh9cFkfLxa6d2A2ADuJXkyQNpte0QC1qGKGBjTdqYNX_cfjrwRIlk7HlK2VINjlFpvrcYYJYArVqCMjvCq3L7srhDNeT4L7Ec9ch-cUBhZQFRexdSGAWorBRlMazlQnIJLYpZSydOnnwgX_P1I3W9KXiew5bO3SmV2mo73vf95TayWnc4CWbtOEp3Q_JFgCG8Q0PZYUAx7X7atP26-6dBXeyXBdJ2dNP6BOAzRpiNiuhKT3Fkdslxm8pIQ";
    const RSA_E: &str = "AQAB";
    const KEY_ID: &str = "key-1";
    const ISSUER: &str = "https://issuer.example";
    const AUDIENCE: &str = "memory-mcp";

    /// A loopback JWKS endpoint serving one RSA key, which is the only
    /// boundary `validate_token` reaches out to.
    fn jwks_cache() -> crate::control::oidc::JwksCache {
        let body = format!(
            r#"{{"keys":[{{"kid":"{KEY_ID}","kty":"RSA","n":"{RSA_N}","e":"{RSA_E}","alg":"RS256"}}]}}"#
        );
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let addr = listener.local_addr().expect("loopback address");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while let Ok(1) = stream.read(&mut byte) {
                    head.push(byte[0]);
                    if head.ends_with(b"\r\n\r\n") {
                        break;
                    }
                }
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        crate::control::oidc::JwksCache::from_parts(
            reqwest::Client::new(),
            format!("http://{addr}/jwks"),
            Duration::from_secs(300),
        )
    }

    /// An `HttpConfig` with the audience and issuer the tokens below claim.
    fn config() -> crate::http::config::HttpConfig {
        let mut config = crate::http::config::HttpConfig::default_for_test();
        config.oidc_audience = AUDIENCE.to_string();
        config.oidc_issuer = ISSUER.to_string();
        config.oidc_allowed_alg = "RS256".to_string();
        config
    }

    /// Sign `claims` with the test key under `kid`.
    fn sign(claims: &serde_json::Value, kid: Option<&str>) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = kid.map(str::to_string);
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(RSA_PRIVATE_PEM.as_bytes()).expect("private key parses"),
        )
        .expect("token signs")
    }

    /// A valid, unexpired token for `sub`.
    fn valid_claims(sub: &str, exp: u64) -> serde_json::Value {
        serde_json::json!({
            "sub": sub,
            "iss": ISSUER,
            "aud": AUDIENCE,
            "exp": exp,
        })
    }

    /// A far-future expiry, so the assertions never race the wall clock.
    const FAR_FUTURE: u64 = 4_102_444_800; // 2100-01-01T00:00:00Z

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

    #[tokio::test]
    async fn accepts_a_validly_signed_token() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(observed.is_ok(), "a correctly signed token must validate");
    }

    #[tokio::test]
    async fn returns_the_subject_of_a_valid_token() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache())
            .await
            .expect("token validates");

        assert_eq!(observed.sub, "user-1");
    }

    #[tokio::test]
    async fn rejects_a_token_that_is_not_a_jwt() {
        let observed = validate_token("not-a-jwt", &config(), &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::MalformedToken)));
    }

    #[tokio::test]
    async fn rejects_a_token_with_no_key_id() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), None);

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(
            matches!(observed, Err(TokenError::MissingKeyId)),
            "an unsigned-by-kid token cannot select a verification key"
        );
    }

    #[tokio::test]
    async fn rejects_a_token_whose_key_id_is_unknown() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some("key-unknown"));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::KeyNotFound(_))));
    }

    #[tokio::test]
    async fn rejects_a_token_that_does_not_match_the_configured_algorithm() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));
        let mut config = config();
        config.oidc_allowed_alg = "ES256".to_string();

        let observed = validate_token(&token, &config, &jwks_cache()).await;

        // Algorithm confusion must be refused: a token signed with a different
        // algorithm than the deployment allows never reaches verification.
        assert!(matches!(observed, Err(TokenError::DisallowedAlgorithm)));
    }

    #[tokio::test]
    async fn rejects_an_algorithm_the_deployment_does_not_allow() {
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));
        let mut config = config();
        config.oidc_allowed_alg = "HS256".to_string();

        let observed = validate_token(&token, &config, &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::DisallowedAlgorithm)));
    }

    #[tokio::test]
    async fn rejects_an_expired_token() {
        let token = sign(&valid_claims("user-1", 1), Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::ExpiredToken)));
    }

    #[tokio::test]
    async fn rejects_a_token_for_another_audience() {
        let claims = serde_json::json!({
            "sub": "user-1",
            "iss": ISSUER,
            "aud": "some-other-service",
            "exp": FAR_FUTURE,
        });
        let token = sign(&claims, Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::InvalidAudience)));
    }

    #[tokio::test]
    async fn rejects_a_token_from_another_issuer() {
        let claims = serde_json::json!({
            "sub": "user-1",
            "iss": "https://evil.example",
            "aud": AUDIENCE,
            "exp": FAR_FUTURE,
        });
        let token = sign(&claims, Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(matches!(observed, Err(TokenError::InvalidIssuer)));
    }

    #[tokio::test]
    async fn rejects_a_token_without_an_expiry() {
        let claims = serde_json::json!({
            "sub": "user-1",
            "iss": ISSUER,
            "aud": AUDIENCE,
        });
        let token = sign(&claims, Some(KEY_ID));

        let observed = validate_token(&token, &config(), &jwks_cache()).await;

        assert!(
            observed.is_err(),
            "`exp` is a required spec claim for a bearer token"
        );
    }

    #[test]
    fn header_defaults_to_rs256_for_the_signer() {
        let observed = Header::new(Algorithm::RS256);

        assert_eq!(observed.alg, Algorithm::RS256);
    }

    // --- resource-server principal resolution -----------------------------

    use crate::control::oidc::identity_subject_verifier;
    use crate::http::principal::AuthenticatedPrincipal;
    use crate::http::registry::models::{
        Account, AccountStatus, ExternalIdentity, NamespaceBinding, SubjectVerifier, Tenant,
        TenantStatus,
    };
    use crate::http::registry::storage::{AccountStore, InMemoryStore};

    /// A registry holding one Account reachable through `(issuer, sub)` — the
    /// same blind index the login flow writes — so the resource-server path
    /// proves it resolves identities the way login does, not some parallel one.
    async fn registry_with_identity(sub: &str, status: AccountStatus) -> Arc<dyn AccountStore> {
        let stores =
            crate::http::registry::RegistryStores::from_backend(Arc::new(InMemoryStore::default()));
        let accounts = stores.accounts();
        let now = chrono::Utc::now();
        let verifier = identity_subject_verifier(&config().keys.identity_index, ISSUER, sub)
            .expect("identity index");
        accounts
            .create_account_bundle(
                &Account {
                    id: "acct_1".into(),
                    status,
                    tenant_id: "ten_1".into(),
                    created_at: now,
                    display_name: None,
                },
                &Tenant {
                    id: "ten_1".into(),
                    status: TenantStatus::Ready,
                    namespace_binding: NamespaceBinding {
                        namespace: "tns_1".into(),
                        database: "memory".into(),
                    },
                    plan_version: 1,
                    schema_version: 0,
                    retry_stage: None,
                    provisioning_lease: None,
                    created_at: now,
                    version: 0,
                },
                Some(&ExternalIdentity {
                    id: "idn_1".into(),
                    issuer: ISSUER.into(),
                    subject_verifier: SubjectVerifier(verifier),
                    account_id: "acct_1".into(),
                    created_at: now,
                }),
            )
            .await
            .expect("seed account with identity");
        accounts
    }

    #[tokio::test]
    async fn resolves_an_active_account_from_a_valid_access_token() {
        let accounts = registry_with_identity("user-1", AccountStatus::Active).await;
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), &token).await;

        match principal {
            Some(AuthenticatedPrincipal::Oidc {
                account,
                issuer,
                subject,
            }) => {
                assert_eq!(account.id, "acct_1");
                assert_eq!(issuer, ISSUER);
                assert_eq!(subject, "user-1");
            }
            other => panic!("expected an Oidc principal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_suspended_account_is_not_resolved() {
        let accounts = registry_with_identity("user-1", AccountStatus::Suspended).await;
        let token = sign(&valid_claims("user-1", FAR_FUTURE), Some(KEY_ID));

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), &token).await;

        assert!(
            principal.is_none(),
            "a token for an Account that is not Active must not authenticate"
        );
    }

    #[tokio::test]
    async fn an_identity_no_account_owns_is_not_resolved() {
        let accounts = registry_with_identity("user-1", AccountStatus::Active).await;
        // Signed by the provider's real key and naming the configured issuer,
        // so only the identity lookup can reject it.
        let token = sign(&valid_claims("someone-else", FAR_FUTURE), Some(KEY_ID));

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), &token).await;

        assert!(principal.is_none(), "an unlinked subject must not resolve");
    }

    #[tokio::test]
    async fn an_invalid_access_token_is_not_resolved() {
        let accounts = registry_with_identity("user-1", AccountStatus::Active).await;

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), "not-a-jwt").await;

        assert!(principal.is_none(), "a malformed token must not resolve");
    }

    #[tokio::test]
    async fn an_array_audience_naming_this_resource_resolves() {
        // RFC 7519 lets `aud` be a single string or an array, and the login-flow
        // decoder accepts both. The resource-server path must accept the array
        // form too, or an access token minted for several resources would work
        // for browser sign-in but not for MCP.
        let accounts = registry_with_identity("user-1", AccountStatus::Active).await;
        let mut claims = valid_claims("user-1", FAR_FUTURE);
        claims["aud"] = serde_json::json!([AUDIENCE, "another-resource"]);
        let token = sign(&claims, Some(KEY_ID));

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), &token).await;

        assert!(
            principal.is_some(),
            "an array audience naming this resource must resolve the Account"
        );
    }

    #[tokio::test]
    async fn an_array_audience_without_this_resource_is_not_resolved() {
        let accounts = registry_with_identity("user-1", AccountStatus::Active).await;
        let mut claims = valid_claims("user-1", FAR_FUTURE);
        claims["aud"] = serde_json::json!(["another-resource", "a-third"]);
        let token = sign(&claims, Some(KEY_ID));

        let principal =
            resolve_bearer_principal(&config(), &*accounts, &jwks_cache(), &token).await;

        assert!(
            principal.is_none(),
            "an array audience that omits this resource must not resolve"
        );
    }
}
