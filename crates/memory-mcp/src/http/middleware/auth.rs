//! Authentication and CSRF middleware.
//!
//! Three independent authenticators cover the three trust paths:
//! - `authenticate` is the Bearer-API-key path used by `/mcp`.
//! - `authenticate_control_plane_session` is the Secure-cookie path
//!   used by `/api/v1/account/*` and `/api/v1/operator/*`.
//! - `authenticate_control_plane_operator` is the operator allowlist
//!   path layered on top of the session authenticator.
//!
//! `require_control_plane_csrf` is the cross-cutting CSRF check that
//! sits behind the session authenticator on state-changing API calls.

use axum::http::StatusCode;
#[allow(unused_imports)]
use axum::http::{Method, header};
use axum::middleware::Next;
#[allow(unused_imports)]
use axum::response::IntoResponse;
use axum::response::Response;
use std::sync::Arc;

use crate::http::HttpState;
use crate::http::principal::auth::AuthDecision;

/// Build a 401 response with the `WWW-Authenticate: Bearer`
/// challenge. Used by `authenticate` (and any other auth
/// middleware that needs the same shape). The body is empty so
/// the response carries no information about why the request
/// was rejected.
fn unauthorized_response() -> Response {
    let mut response = Response::new(axum::body::Body::empty());
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    response.headers_mut().insert(
        axum::http::header::WWW_AUTHENTICATE,
        axum::http::HeaderValue::from_static("Bearer realm=\"memory-mcp\""),
    );
    response
}

/// Bearer-token authenticator. Wired only
/// on `/mcp`. Returns 401 (with `WWW-Authenticate: Bearer
/// realm="memory-mcp"`) without distinguishing missing,
/// unknown, expired, revoked, or malformed keys. The raw
/// `Authorization` value is never logged or surfaced.
pub async fn authenticate(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let header = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned);
    let request_id = req
        .extensions()
        .get::<crate::http::logging::RequestId>()
        .map(|id| id.as_uuid().to_string());
    let decision = match header.as_deref() {
        Some(value) => {
            let mut parts = value.split_ascii_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some(scheme), Some(credentials), None)
                    if scheme.eq_ignore_ascii_case("Bearer") =>
                {
                    let api_key = state.authenticator.authenticate_bearer(credentials).await;
                    match api_key {
                        AuthDecision::Allow(principal) => AuthDecision::Allow(principal),
                        // Not a valid API key. An access token issued by the
                        // deployment's OIDC provider may still be valid, so the
                        // refusal is not final until the OAuth path also
                        // declines. `Deny` logged and counted its bounded reason
                        // inside the authenticator; `NotApplicable` did not, so
                        // it is logged and counted here — once — on the final
                        // denial, never on a request OAuth goes on to accept.
                        other => {
                            let not_an_api_key = matches!(other, AuthDecision::NotApplicable);
                            match oauth_bearer(&state, credentials).await {
                                Some(principal) => AuthDecision::Allow(principal),
                                None => {
                                    if not_an_api_key {
                                        crate::http::logging::log_auth_rejection(
                                            crate::http::logging::AuthRejection::Parse,
                                            request_id.as_deref(),
                                        );
                                    }
                                    AuthDecision::Deny
                                }
                            }
                        }
                    }
                }
                _ => {
                    crate::http::logging::log_auth_rejection(
                        crate::http::logging::AuthRejection::BadScheme,
                        request_id.as_deref(),
                    );
                    AuthDecision::Deny
                }
            }
        }
        None => {
            crate::http::logging::log_auth_rejection(
                crate::http::logging::AuthRejection::Missing,
                request_id.as_deref(),
            );
            AuthDecision::Deny
        }
    };
    let principal = match decision {
        AuthDecision::Allow(principal) => principal,
        _ => return unauthorized_response(),
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Authenticate an OAuth 2.0 access token as a fallback for a credential that
/// is not an API key.
///
/// Only meaningful where the deployment serves an OIDC authorization server
/// (`MEMORY_MCP_HTTP_AUTH_METHODS` includes `oidc`) and therefore publishes
/// protected-resource metadata; a deployment without one accepts no access
/// tokens, so the fallback is inert rather than rejecting what the API-key
/// path already denied.
#[cfg(feature = "control-plane")]
async fn oauth_bearer(
    state: &HttpState,
    token: &str,
) -> Option<crate::http::principal::AuthenticatedPrincipal> {
    if !state
        .config
        .has_method(crate::http::config::BrowserAuthMethod::Oidc)
    {
        return None;
    }
    let client = state.oidc_client.as_ref()?;
    crate::http::oauth::resolve_bearer_principal(
        &state.config,
        &*state.registry.accounts(),
        client.jwks(),
        token,
    )
    .await
}

/// Without a control plane there is no OIDC provider, so no access token can
/// be resolved and the fallback stays inert.
#[cfg(not(feature = "control-plane"))]
async fn oauth_bearer(
    _state: &HttpState,
    _token: &str,
) -> Option<crate::http::principal::AuthenticatedPrincipal> {
    None
}

/// Authenticate the control-plane Secure cookie and attach the server-side
/// session. This middleware is mounted only on `/api/v1/account/*` and never
/// participates in MCP Bearer authentication.
#[cfg(feature = "control-plane")]
pub async fn authenticate_control_plane_session(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: Next,
) -> Response {
    let cookie = req
        .headers()
        .get(header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|cookies| {
            crate::control::session::parse_session_cookie(cookies, &state.config.base_path)
                .map(str::to_owned)
        });
    let Some(cookie) = cookie else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let session = match crate::control::session::resolve_session_record(&state, &cookie).await {
        Ok(session) => session,
        Err(error) => return error.into_response(),
    };
    req.extensions_mut().insert(session);
    next.run(req).await
}

/// Authenticate a control-plane session as an operator by matching one of its
/// durable external identity blind indexes against the immutable deployment
/// allowlist. Account data cannot grant itself this role.
#[cfg(feature = "control-plane")]
pub async fn authenticate_control_plane_operator(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let Some(session) = req
        .extensions()
        .get::<crate::control::session::ControlPlaneSession>()
        .cloned()
    else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let identities = match state
        .registry
        .identities()
        .find_external_identities(&session.account_id)
        .await
    {
        Ok(identities) => identities,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "operator registry unavailable",
            )
                .into_response();
        }
    };
    let is_operator = identities.iter().any(|identity| {
        let entry = format!(
            "{}|{}",
            identity.issuer,
            hex::encode(identity.subject_verifier.0)
        );
        state
            .config
            .operator_identity_allowlist
            .iter()
            .any(|allowed| allowed == &entry)
    });
    if !is_operator {
        return (StatusCode::FORBIDDEN, "operator access required").into_response();
    }
    let mut req = req;
    req.extensions_mut()
        .insert(crate::control::operator::OperatorPrincipal {
            authenticated_at: session.auth_time,
        });
    next.run(req).await
}

/// Require a valid CSRF header for a cookie-authenticated state-changing API
/// request. The token is bound to the Account and server-side session id.
#[cfg(feature = "control-plane")]
pub async fn require_control_plane_csrf(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    if matches!(
        req.method(),
        &Method::GET | &Method::HEAD | &Method::OPTIONS
    ) {
        return next.run(req).await;
    }
    let Some(session) = req
        .extensions()
        .get::<crate::control::session::ControlPlaneSession>()
    else {
        return (StatusCode::UNAUTHORIZED, "control-plane session required").into_response();
    };
    let token = req
        .headers()
        .get("x-csrf-token")
        .and_then(|value| value.to_str().ok());
    let valid = token.is_some_and(|token| {
        crate::control::csrf::verify_csrf(
            &state.config.keys.csrf,
            &session.account_id,
            &session.id,
            token,
        )
        .unwrap_or(false)
    });
    if !valid {
        return (StatusCode::FORBIDDEN, "csrf validation failed").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::routing::post;
    use tower_service::Service;

    async fn accept_any(_: axum::extract::Request) -> Response {
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn missing_bearer_returns_401_with_www_authenticate() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut svc = Router::new()
            .route("/", post(accept_any))
            .layer(axum::middleware::from_fn_with_state(state, authenticate));
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers()
                .get(axum::http::header::WWW_AUTHENTICATE)
                .map(|v| v.to_str().unwrap()),
            Some("Bearer realm=\"memory-mcp\""),
        );
    }

    #[tokio::test]
    async fn non_bearer_scheme_returns_401_with_www_authenticate() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut svc = Router::new()
            .route("/", post(accept_any))
            .layer(axum::middleware::from_fn_with_state(state, authenticate));
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("authorization", "Basic abc")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );
    }

    /// A missing or malformed bearer is logged with a bounded reason, so an
    /// operator can see refusals as a rate instead of reading the raw header.
    #[tokio::test]
    async fn a_missing_bearer_is_logged_with_reason_missing() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut svc = Router::new()
            .route("/", post(accept_any))
            .layer(axum::middleware::from_fn_with_state(state, authenticate));
        let req = axum::http::Request::builder()
            .method("POST")
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let sink = crate::logging::capture::install();

        let resp = svc.call(req).await.unwrap();

        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.auth.rejected")
                    && line.contains("reason=missing")
                    && line.contains("WARN")),
            "the refusal must be recorded with its reason: {recorded:?}"
        );
    }

    #[tokio::test]
    async fn router_level_wiring_validates_before_auth_and_keeps_health_public() {
        // Proves the spec-mandated ordering: malformed MCP requests fail
        // before auth, while a valid modern envelope reaches auth. The
        // health endpoint remains unauthenticated.
        use crate::http::router as build_router;
        let state = crate::http::HttpState::default_for_test().await;
        let router = build_router::build_router(state, None).expect("router builds in tests");
        let mut svc = router;

        let malformed = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(malformed).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);

        let valid_body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": {
                        "name": "middleware-test",
                        "version": "0.0.0"
                    },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let valid = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover")
            .body(axum::body::Body::from(valid_body.to_string()))
            .unwrap();
        let resp = svc.call(valid).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(
            resp.headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE)
        );

        // /health/live is unauthenticated; rebuild a fresh
        // router to drive the second request.
        let state2 = crate::http::HttpState::default_for_test().await;
        let router2 = build_router::build_router(state2, None).expect("router builds in tests");
        let req = axum::http::Request::builder()
            .method("GET")
            .uri("/health/live")
            .header("host", "localhost")
            .body(axum::body::Body::empty())
            .unwrap();
        let mut svc = router2;
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // --- OAuth resource-server fallback -----------------------------------

    #[cfg(feature = "control-plane")]
    use crate::control::oidc::test_provider::{MockProvider, MockProviderConfig, sign_id_token};

    /// The audience the token fixtures below carry, and a far-future expiry so
    /// the assertions never race the wall clock.
    #[cfg(feature = "control-plane")]
    const OAUTH_AUDIENCE: &str = "memory-mcp";
    #[cfg(feature = "control-plane")]
    const OAUTH_EXPIRY: u64 = 4_102_444_800; // 2100-01-01T00:00:00Z

    /// Compose a state whose resource-server path trusts `provider`, and seed
    /// the Account its `(issuer, subject)` blind index names.
    #[cfg(feature = "control-plane")]
    async fn state_with_oauth_account(
        provider: &MockProvider,
        sub: &str,
    ) -> std::sync::Arc<HttpState> {
        use crate::http::registry::models::{
            Account, AccountStatus, ExternalIdentity, NamespaceBinding, SubjectVerifier, Tenant,
            TenantStatus,
        };
        use crate::http::test_state::HttpStateTestBuilder;

        let client = crate::control::oidc::OidcClient::new(
            provider.base_url(),
            "memory-mcp",
            OAUTH_AUDIENCE,
            "https://app.example/callback",
            "auto",
        )
        .await
        .expect("OIDC discovery against the mock provider");

        let mut config = crate::http::config::HttpConfig::default_for_test();
        config.oidc_issuer = provider.base_url().to_string();
        config.oidc_audience = OAUTH_AUDIENCE.to_string();
        // The resource-server path maps an explicit algorithm pin, so the test
        // names the one the provider advertises rather than the `auto` default.
        config.oidc_allowed_alg = "RS256".to_string();

        let state = HttpStateTestBuilder::new()
            .await
            .with_config(config)
            .with_oidc_client(std::sync::Arc::new(client))
            .build()
            .await
            .expect("composed OAuth-shaped state");

        let now = chrono::Utc::now();
        let verifier = crate::control::oidc::identity_subject_verifier(
            &state.config.keys.identity_index,
            provider.base_url(),
            sub,
        )
        .expect("identity index");
        state
            .registry
            .accounts()
            .create_account_bundle(
                &Account {
                    id: "acct_oauth".into(),
                    status: AccountStatus::Active,
                    tenant_id: "ten_oauth".into(),
                    created_at: now,
                    display_name: None,
                },
                &Tenant {
                    id: "ten_oauth".into(),
                    status: TenantStatus::Ready,
                    namespace_binding: NamespaceBinding {
                        namespace: "tns_oauth".into(),
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
                    id: "idn_oauth".into(),
                    issuer: provider.base_url().into(),
                    subject_verifier: SubjectVerifier(verifier),
                    account_id: "acct_oauth".into(),
                    created_at: now,
                }),
            )
            .await
            .expect("seed Account with identity");

        state
    }

    /// POST a valid MCP envelope to `/mcp` with `token` as the bearer, exactly
    /// as a client sends it (host, origin and peer included).
    #[cfg(feature = "control-plane")]
    async fn post_mcp_with_bearer(router: axum::Router, token: &str) -> Response {
        use axum::extract::connect_info::ConnectInfo;
        use std::net::SocketAddr;
        use tower_service::Service;

        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "server/discover",
            "params": {
                "_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientInfo": { "name": "oauth-test", "version": "0.0.0" },
                    "io.modelcontextprotocol/clientCapabilities": {}
                }
            }
        });
        let peer: SocketAddr = "127.0.0.1:54321".parse().expect("peer address");
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/mcp")
            .header("host", "localhost")
            .header("origin", "http://localhost")
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", "2026-07-28")
            .header("mcp-method", "server/discover")
            .extension(ConnectInfo(peer))
            .body(axum::body::Body::from(body.to_string()))
            .expect("request");
        let mut router = router;
        router.call(request).await.expect("dispatch")
    }

    /// An access token the OIDC provider issued for a linked Account clears the
    /// `/mcp` authenticator, exactly where an API key does.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn an_oidc_access_token_authenticates_on_mcp() {
        let provider = MockProvider::spawn(MockProviderConfig::default()).await;
        let state = state_with_oauth_account(&provider, "user-1").await;
        let token = sign_id_token(&serde_json::json!({
            "sub": "user-1",
            "iss": provider.base_url(),
            "aud": OAUTH_AUDIENCE,
            "exp": OAUTH_EXPIRY,
        }));
        let router = crate::http::router::build_router(state, None).expect("router builds");

        let sink = crate::logging::capture::install();
        let response = post_mcp_with_bearer(router, &token).await;

        assert_ne!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "an OIDC access token must clear the `/mcp` authenticator"
        );
        assert!(
            !response
                .headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE),
            "an authenticated request must not carry the bearer challenge"
        );
        // A credential the API-key path cannot parse is not a refusal: it may
        // be an access token, and this one is. Counting it as a refusal would
        // make every successful OAuth request look like an authentication
        // failure. The check is tied to this request's own id, so a parallel
        // test's refusal line cannot satisfy or break it.
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .expect("the response carries the request id")
            .to_string();
        assert!(
            !sink.lines().iter().any(|line| {
                line.contains("op=http.auth.rejected")
                    && line.contains(&format!("req={request_id}"))
            }),
            "an accepted OAuth token must not be logged as a refusal for its own \
             request {request_id}: {:?}",
            sink.lines()
        );
    }

    /// A well-formed access token whose subject no Account owns is refused: the
    /// fallback must not become an open door.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn an_access_token_for_an_unlinked_subject_is_denied_on_mcp() {
        let provider = MockProvider::spawn(MockProviderConfig::default()).await;
        let state = state_with_oauth_account(&provider, "user-1").await;
        let token = sign_id_token(&serde_json::json!({
            "sub": "nobody",
            "iss": provider.base_url(),
            "aud": OAUTH_AUDIENCE,
            "exp": OAUTH_EXPIRY,
        }));
        let router = crate::http::router::build_router(state, None).expect("router builds");

        let response = post_mcp_with_bearer(router, &token).await;

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            response
                .headers()
                .contains_key(axum::http::header::WWW_AUTHENTICATE),
            "a refused request must carry the bearer challenge"
        );
    }

    /// Where no OIDC method is configured the fallback is inert: a token that
    /// is not an API key is refused outright, whatever it claims.
    #[cfg(feature = "control-plane")]
    #[tokio::test]
    async fn the_access_token_fallback_is_inert_without_the_oidc_method() {
        let (builder, _store) = crate::http::test_state::HttpStateTestBuilder::local_admin().await;
        let state = builder.build().await.expect("composed local-admin state");
        let token = sign_id_token(&serde_json::json!({
            "sub": "user-1",
            "iss": "https://issuer.invalid",
            "aud": OAUTH_AUDIENCE,
            "exp": OAUTH_EXPIRY,
        }));
        let router = crate::http::router::build_router(state, None).expect("router builds");

        let sink = crate::logging::capture::install();
        let response = post_mcp_with_bearer(router, &token).await;

        assert_eq!(
            response.status(),
            StatusCode::UNAUTHORIZED,
            "a deployment with no OIDC method must accept no access token"
        );
        // With the OAuth path inert, the denial is final, so the refusal is
        // logged and counted here — once — with its bounded reason.
        assert!(
            sink.lines()
                .iter()
                .any(|line| line.contains("op=http.auth.rejected")
                    && line.contains("reason=parse")
                    && line.contains("WARN")),
            "a final denial of a non-API-key credential must be a logged refusal: {:?}",
            sink.lines()
        );
    }
}
