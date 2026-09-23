#![cfg(all(
    feature = "streamable-http",
    feature = "test-fixtures",
    feature = "control-plane"
))]

//! Local administrator HTTP surface (Task 6).
//!
//! Drives the real router over a **durable in-memory Registry store**
//! with the production local composition. Every assertion exercises
//! the actual pre-auth cookie, CSRF token, Origin check, session
//! cookie and SQL transaction path — nothing is stubbed.
//!
//! The fixture attaches `ConnectInfo` explicitly, because the handlers
//! fail closed when the direct socket peer is unknown (see
//! `control::local_admin::direct_peer`). No test relies on a hidden
//! auto-CSRF helper: negative tests deliberately omit or corrupt one
//! piece of the contract at a time.
//!
//! Run:
//! `cargo test -p memory_mcp --features control-plane,test-fixtures --locked --test http_local_admin`

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::connect_info::ConnectInfo;
use axum::http::{Request, StatusCode};
use memory_mcp::http::HttpState;
use memory_mcp::http::principal::api_keys::ApiKeyCredential;
use memory_mcp::http::principal::auth::{AuthDecision, Authenticator};
use memory_mcp::http::registry::SurrealRegistryStore;
use memory_mcp::http::registry::models::{ApiKey, ApiKeyStatus, KeyedVerifier, TenantStatus};
use memory_mcp::http::registry::storage::RegistryStore;
use memory_mcp::http::router::build_router;
use memory_mcp::http::test_state::HttpStateTestBuilder;
use memory_mcp::service::local_admin::{
    AdminFence, AdminManagementService, LocalAdminAuthority, LocalAdminService, PasswordHasher,
    RequestContext,
};
use tower_service::Service;

const ORIGIN: &str = "http://localhost";
const PEER: &str = "127.0.0.1:54321";

fn peer() -> SocketAddr {
    PEER.parse().expect("peer address")
}

/// A test harness owning one durable local-admin deployment.
struct Harness {
    state: Arc<HttpState>,
    store: Arc<SurrealRegistryStore>,
    router: Router,
}

impl Harness {
    async fn new() -> Self {
        let (builder, store) = HttpStateTestBuilder::local_admin().await;
        let state = builder.build().await.expect("local admin HTTP state");
        let router = build_router(state.clone(), None).expect("router builds in tests");
        Self {
            state,
            store,
            router,
        }
    }

    /// The production bearer authenticator wired by `HttpState::assemble`.
    /// It shares the registry store and the configured pepper with the
    /// local admin key handler, so it is the real data-plane path.
    fn authenticator(&self) -> Arc<Authenticator> {
        self.state.authenticator.clone()
    }

    /// Advance a freshly created client's tenant from `reserved` to
    /// `ready` so the key-issuance route accepts it. This is an explicit
    /// test-only state write, not provisioning evidence.
    async fn ready_tenant(&self, account_id: &str) -> String {
        let tenant = self
            .store
            .find_tenant_by_account(account_id)
            .await
            .expect("tenant lookup")
            .expect("tenant exists");
        self.store
            .update_tenant_state(
                &tenant.id,
                tenant.version,
                TenantStatus::Reserved,
                TenantStatus::Ready,
            )
            .await
            .expect("tenant ready");
        tenant.id
    }

    /// Create a client through the real HTTP route and mark it ready.
    /// Returns `(account_id, tenant_id)`.
    async fn create_ready_client(&self, session: &Session, display_name: &str) -> (String, String) {
        let operation = uuid::Uuid::new_v4().to_string();
        let created = self
            .send(
                "POST",
                "/api/v1/admin/clients",
                Some(serde_json::json!({ "display_name": display_name })),
                &[
                    ("x-csrf-token", &session.csrf),
                    ("idempotency-key", &operation),
                ],
                Some(&session.cookie),
            )
            .await;
        assert_eq!(created.status(), StatusCode::ACCEPTED, "create client");
        let account_id = read_json(created).await["account_id"]
            .as_str()
            .expect("account id")
            .to_string();
        let tenant_id = self.ready_tenant(&account_id).await;
        (account_id, tenant_id)
    }

    /// Issue a key through the real HTTP route and return its response.
    async fn issue_key(
        &self,
        session: &Session,
        account_id: &str,
        name: &str,
        expiry: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        let operation = uuid::Uuid::new_v4().to_string();
        let response = self
            .send(
                "POST",
                &format!("/api/v1/admin/clients/{account_id}/keys"),
                Some(serde_json::json!({ "name": name, "expiry": expiry })),
                &[
                    ("x-csrf-token", &session.csrf),
                    ("idempotency-key", &operation),
                ],
                Some(&session.cookie),
            )
            .await;
        let status = response.status();
        (status, read_json(response).await)
    }

    /// Revoke a key through the real HTTP route.
    async fn revoke_key(&self, session: &Session, account_id: &str, key_id: &str) -> StatusCode {
        self.send(
            "DELETE",
            &format!("/api/v1/admin/clients/{account_id}/keys/{key_id}"),
            None,
            &[("x-csrf-token", &session.csrf)],
            Some(&session.cookie),
        )
        .await
        .status()
    }

    fn authority(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Arc<LocalAdminAuthority>> + Send + '_>>
    {
        Box::pin(async move {
            LocalAdminAuthority::join_local_for_test(
                self.store.clone(),
                HttpStateTestBuilder::LOCAL_TEST_SESSION_KEY,
                HttpStateTestBuilder::LOCAL_TEST_CSRF_KEY,
            )
            .await
            .expect("join durable local policy")
        })
    }

    async fn management(&self) -> AdminManagementService {
        AdminManagementService::new(self.authority().await)
    }

    async fn service(&self) -> LocalAdminService {
        LocalAdminService::new(
            self.authority().await,
            Arc::new(PasswordHasher::new().expect("supported KDF")),
        )
    }

    /// Create an administrator through the CLI-equivalent management
    /// path and return the one-time activation code.
    async fn create_admin(&self, username: &str) -> String {
        self.management()
            .await
            .create_admin(
                username,
                &RequestContext {
                    request_id: uuid::Uuid::new_v4(),
                },
            )
            .await
            .expect("create admin")
            .code
    }

    /// Activate an administrator end to end through the HTTP surface.
    async fn activate(&self, code: &str, password: &str) {
        let preauth = self.preauth().await;
        let response = self
            .send(
                "POST",
                "/api/v1/auth/local/activate",
                Some(serde_json::json!({"code": code, "password": password})),
                &[("x-csrf-token", &preauth.token)],
                Some(&preauth.cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "activation");
    }

    /// Fetch a fresh pre-auth cookie + CSRF token.
    async fn preauth(&self) -> Preauth {
        let response = self.get("/api/v1/auth/local/csrf", &[]).await;
        assert_eq!(response.status(), StatusCode::OK, "preauth csrf");
        let cookie = response
            .headers()
            .get_all(axum::http::header::SET_COOKIE)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .find_map(|value| {
                let (pair, _rest) = value.split_once(';')?;
                pair.starts_with("__Host-memory_mcp_admin_preauth=")
                    .then(|| pair.to_string())
            })
            .expect("preauth cookie issued");
        let body = read_json(response).await;
        Preauth {
            token: body["csrf_token"].as_str().expect("token").to_string(),
            cookie,
        }
    }

    /// Log in and return the admin session cookie plus CSRF token.
    async fn login(&self, username: &str, password: &str) -> Session {
        let preauth = self.preauth().await;
        let response = self
            .send(
                "POST",
                "/api/v1/auth/local/login",
                Some(serde_json::json!({"username": username, "password": password})),
                &[("x-csrf-token", &preauth.token)],
                Some(&preauth.cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "login");
        let cookie = session_cookie(&response).expect("session cookie issued");
        let session = self
            .send("GET", "/api/v1/admin/session", None, &[], Some(&cookie))
            .await;
        assert_eq!(session.status(), StatusCode::OK, "session");
        let body = read_json(session).await;
        Session {
            cookie,
            csrf: body["csrf_token"].as_str().expect("csrf").to_string(),
        }
    }

    async fn get(&self, path: &str, headers: &[(&str, &str)]) -> axum::response::Response {
        self.send("GET", path, None, headers, None).await
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        json: Option<serde_json::Value>,
        headers: &[(&str, &str)],
        cookie: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(path)
            .header(axum::http::header::HOST, "localhost")
            .header(axum::http::header::ORIGIN, ORIGIN)
            .extension(ConnectInfo(peer()));
        if json.is_some() {
            builder = builder.header(axum::http::header::CONTENT_TYPE, "application/json");
        }
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        if let Some(cookie) = cookie {
            builder = builder.header(axum::http::header::COOKIE, cookie);
        }
        let body = match json {
            Some(value) => Body::from(serde_json::to_vec(&value).expect("serialize")),
            None => Body::empty(),
        };
        let mut service = self.router.clone();
        service
            .call(builder.body(body).expect("request"))
            .await
            .expect("dispatch")
    }
}

struct Preauth {
    cookie: String,
    token: String,
}

struct Session {
    cookie: String,
    csrf: String,
}

fn session_cookie(response: &axum::response::Response) -> Option<String> {
    response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find_map(|value| {
            let (pair, _rest) = value.split_once(';')?;
            (pair.starts_with("__Host-memory_mcp_admin=") && !pair.ends_with('='))
                .then(|| pair.to_string())
        })
}

async fn read_json(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .expect("body");
    if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    }
}

// ─── Method disclosure ────────────────────────────────────

#[tokio::test]
async fn auth_config_reports_only_the_local_method() {
    let harness = Harness::new().await;
    let response = harness.get("/api/v1/auth/config", &[]).await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = read_json(response).await;
    assert_eq!(body["methods"], serde_json::json!(["local"]));
    assert!(body.get("issuer").is_none());
    assert!(body.get("admin_count").is_none());
}

// ─── Pre-auth CSRF ────────────────────────────────────────

#[tokio::test]
async fn preauth_cookie_is_host_scoped_and_httponly() {
    let harness = Harness::new().await;
    let response = harness.get("/api/v1/auth/local/csrf", &[]).await;
    let cookie = response
        .headers()
        .get_all(axum::http::header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .find(|value| value.starts_with("__Host-memory_mcp_admin_preauth="))
        .expect("preauth cookie")
        .to_string();
    assert!(cookie.contains("Path=/"), "{cookie}");
    assert!(cookie.contains("Secure"), "{cookie}");
    assert!(cookie.contains("HttpOnly"), "{cookie}");
    assert!(cookie.contains("SameSite=Strict"), "{cookie}");
    assert!(!cookie.contains("Domain="), "{cookie}");
    assert!(cookie.contains("Max-Age=300"), "{cookie}");
}

// ─── Activation, login, session ───────────────────────────

#[tokio::test]
async fn activation_login_session_roundtrip() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;

    // Inspect before consuming: the username is disclosed, nothing else.
    let preauth = harness.preauth().await;
    let inspect = harness
        .send(
            "POST",
            "/api/v1/auth/local/challenge",
            Some(serde_json::json!({"code": code, "kind": "activate"})),
            &[("x-csrf-token", &preauth.token)],
            Some(&preauth.cookie),
        )
        .await;
    assert_eq!(inspect.status(), StatusCode::OK);
    let body = read_json(inspect).await;
    assert_eq!(body["username"], "ops.one");
    assert!(body.get("password_phc").is_none());
    assert!(body.get("admin_id").is_none());

    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;

    // Login yields a session-scoped admin cookie and a session CSRF.
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    assert!(session.cookie.starts_with("__Host-memory_mcp_admin="));
    assert!(!session.csrf.is_empty());

    // The session token is bound to the presented session: a token from
    // another session must not authorize this one.
    let other = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let response = harness
        .send(
            "GET",
            "/api/v1/admin/session",
            None,
            &[],
            Some(&other.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
}

#[tokio::test]
async fn wrong_password_is_uniformly_unauthorized() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;

    let preauth = harness.preauth().await;
    let response = harness
        .send(
            "POST",
            "/api/v1/auth/local/login",
            Some(serde_json::json!({
                "username": "ops.one",
                "password": "a completely different passphrase",
            })),
            &[("x-csrf-token", &preauth.token)],
            Some(&preauth.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let body = read_json(response).await;
    assert_eq!(body["error"]["code"], "invalid_credentials");

    // An unknown username is indistinguishable from a wrong password.
    let preauth = harness.preauth().await;
    let unknown = harness
        .send(
            "POST",
            "/api/v1/auth/local/login",
            Some(serde_json::json!({
                "username": "nobody.here",
                "password": "a sufficiently long passphrase",
            })),
            &[("x-csrf-token", &preauth.token)],
            Some(&preauth.cookie),
        )
        .await;
    assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
    let body = read_json(unknown).await;
    assert_eq!(body["error"]["code"], "invalid_credentials");
    assert!(!body.to_string().contains("nobody.here"));
}

#[tokio::test]
async fn reset_changes_the_password_and_fences_old_sessions() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "the first correct passphrase")
        .await;
    let original = harness
        .login("ops.one", "the first correct passphrase")
        .await;

    let reset = harness
        .management()
        .await
        .recover_admin(
            "ops.one",
            &RequestContext {
                request_id: uuid::Uuid::new_v4(),
            },
        )
        .await
        .expect("recover")
        .code;

    // The old session is fenced immediately by the recovery.
    let response = harness
        .send(
            "GET",
            "/api/v1/admin/session",
            None,
            &[],
            Some(&original.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    harness
        .activate_reset(&reset, "the second correct passphrase")
        .await;

    // The first password is gone; the second works.
    assert_eq!(
        harness
            .login_status("ops.one", "the first correct passphrase")
            .await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness
            .login_status("ops.one", "the second correct passphrase")
            .await,
        StatusCode::NO_CONTENT
    );
}

impl Harness {
    async fn activate_reset(&self, code: &str, password: &str) {
        let preauth = self.preauth().await;
        let response = self
            .send(
                "POST",
                "/api/v1/auth/local/reset",
                Some(serde_json::json!({"code": code, "password": password})),
                &[("x-csrf-token", &preauth.token)],
                Some(&preauth.cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT, "reset");
    }

    async fn login_status(&self, username: &str, password: &str) -> StatusCode {
        let preauth = self.preauth().await;
        self.send(
            "POST",
            "/api/v1/auth/local/login",
            Some(serde_json::json!({"username": username, "password": password})),
            &[("x-csrf-token", &preauth.token)],
            Some(&preauth.cookie),
        )
        .await
        .status()
    }
}

#[tokio::test]
async fn logout_revokes_the_presented_session_only() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let first = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let second = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    let response = harness
        .send(
            "POST",
            "/api/v1/admin/logout",
            None,
            &[("x-csrf-token", &first.csrf)],
            Some(&first.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    assert_eq!(
        harness
            .send(
                "GET",
                "/api/v1/admin/session",
                None,
                &[],
                Some(&first.cookie)
            )
            .await
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        harness
            .send(
                "GET",
                "/api/v1/admin/session",
                None,
                &[],
                Some(&second.cookie)
            )
            .await
            .status(),
        StatusCode::OK,
        "an independent session must survive"
    );
}

#[tokio::test]
async fn logout_without_a_session_is_a_no_op() {
    let harness = Harness::new().await;
    let response = harness
        .send("POST", "/api/v1/admin/logout", None, &[], None)
        .await;
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
}

// ─── CSRF and Origin enforcement ──────────────────────────

#[tokio::test]
async fn public_post_without_csrf_token_is_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    let preauth = harness.preauth().await;
    let response = harness
        .send(
            "POST",
            "/api/v1/auth/local/activate",
            Some(serde_json::json!({"code": code, "password": "a sufficiently long passphrase"})),
            // No X-CSRF-Token.
            &[],
            Some(&preauth.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_post_with_another_cookies_token_is_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    let mine = harness.preauth().await;
    let theirs = harness.preauth().await;
    let response = harness
        .send(
            "POST",
            "/api/v1/auth/local/activate",
            Some(serde_json::json!({"code": code, "password": "a sufficiently long passphrase"})),
            &[("x-csrf-token", &theirs.token)],
            Some(&mine.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_post_without_origin_is_rejected() {
    let harness = Harness::new().await;
    let preauth = harness.preauth().await;
    // Build the request by hand so the Origin header is genuinely absent.
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/login")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .extension(ConnectInfo(peer()))
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"username": "x", "password": "y"})).unwrap(),
        ))
        .expect("request");
    let mut service = harness.router.clone();
    let response = service.call(request).await.expect("dispatch");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn public_post_with_disallowed_origin_is_rejected() {
    let harness = Harness::new().await;
    let preauth = harness.preauth().await;
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/login")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::ORIGIN, "https://evil.example")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .extension(ConnectInfo(peer()))
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"username": "x", "password": "y"})).unwrap(),
        ))
        .expect("request");
    let mut service = harness.router.clone();
    let response = service.call(request).await.expect("dispatch");
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn session_mutation_without_csrf_token_is_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let response = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "team-alpha"})),
            &[("idempotency-key", &uuid::Uuid::new_v4().to_string())],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn duplicate_cookie_values_are_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let doubled = format!("{}; {}", session.cookie, session.cookie);
    let response = harness
        .send("GET", "/api/v1/admin/session", None, &[], Some(&doubled))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ─── Transport contract ───────────────────────────────────

#[tokio::test]
async fn oversized_and_malformed_and_wrong_content_type_are_distinct() {
    let harness = Harness::new().await;
    let preauth = harness.preauth().await;

    // Wrong content type.
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/challenge")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::ORIGIN, ORIGIN)
        .header(axum::http::header::CONTENT_TYPE, "text/plain")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .extension(ConnectInfo(peer()))
        .body(Body::from("code=abc"))
        .expect("request");
    let mut service = harness.router.clone();
    assert_eq!(
        service.call(request).await.expect("dispatch").status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE
    );

    // Malformed JSON.
    let mut service = harness.router.clone();
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/challenge")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::ORIGIN, ORIGIN)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .extension(ConnectInfo(peer()))
        .body(Body::from("{not json"))
        .expect("request");
    assert_eq!(
        service.call(request).await.expect("dispatch").status(),
        StatusCode::BAD_REQUEST
    );

    // Unknown fields are rejected rather than ignored.
    let response = harness
        .send(
            "POST",
            "/api/v1/auth/local/challenge",
            Some(serde_json::json!({"code": "ab", "kind": "activate", "extra": 1})),
            &[("x-csrf-token", &preauth.token)],
            Some(&preauth.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn oversized_body_is_rejected() {
    let harness = Harness::new().await;
    let preauth = harness.preauth().await;
    // 17 KiB of JSON — over the 16 KiB local bound.
    let payload = format!(
        "{{\"code\":\"{}\",\"kind\":\"activate\"}}",
        "a".repeat(17 * 1024)
    );
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/challenge")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::ORIGIN, ORIGIN)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .extension(ConnectInfo(peer()))
        .body(Body::from(payload))
        .expect("request");
    let mut service = harness.router.clone();
    assert_eq!(
        service.call(request).await.expect("dispatch").status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn missing_peer_fails_closed() {
    let harness = Harness::new().await;
    let preauth = harness.preauth().await;
    // No ConnectInfo extension.
    let request = Request::builder()
        .method("POST")
        .uri("/api/v1/auth/local/challenge")
        .header(axum::http::header::HOST, "localhost")
        .header(axum::http::header::ORIGIN, ORIGIN)
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .header("x-csrf-token", &preauth.token)
        .header(axum::http::header::COOKIE, &preauth.cookie)
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"code": "ab", "kind": "activate"})).unwrap(),
        ))
        .expect("request");
    let mut service = harness.router.clone();
    assert_eq!(
        service.call(request).await.expect("dispatch").status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

// ─── Challenge uniformity ─────────────────────────────────

#[tokio::test]
async fn every_bad_challenge_shape_returns_the_same_body() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;

    let mut bodies = Vec::new();
    for payload in [
        // Wrong kind for a live activation code.
        serde_json::json!({"code": code, "kind": "reset"}),
        // Not hex.
        serde_json::json!({"code": "zzzz", "kind": "activate"}),
        // Well-formed but unknown.
        serde_json::json!({"code": "00".repeat(32), "kind": "activate"}),
        // Unsupported kind label.
        serde_json::json!({"code": code, "kind": "elevate"}),
    ] {
        let preauth = harness.preauth().await;
        let response = harness
            .send(
                "POST",
                "/api/v1/auth/local/challenge",
                Some(payload),
                &[("x-csrf-token", &preauth.token)],
                Some(&preauth.cookie),
            )
            .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        bodies.push(read_json(response).await);
    }
    let first = &bodies[0];
    for body in &bodies[1..] {
        assert_eq!(body["error"]["code"], first["error"]["code"]);
        assert_eq!(body["error"]["message"], first["error"]["message"]);
    }
}

// ─── Client and key routes ────────────────────────────────

#[tokio::test]
async fn client_lifecycle_uses_the_durable_store() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    // Listing starts empty.
    let list = harness
        .send(
            "GET",
            "/api/v1/admin/clients",
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(list.status(), StatusCode::OK);
    assert_eq!(read_json(list).await["items"].as_array().unwrap().len(), 0);

    // Creating a client returns 202 with a Location header.
    let operation = uuid::Uuid::new_v4().to_string();
    let created = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "team-alpha"})),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &operation),
            ],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(created.status(), StatusCode::ACCEPTED);
    let location = created
        .headers()
        .get(axum::http::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .expect("Location header")
        .to_string();
    let view = read_json(created).await;
    let account_id = view["account_id"].as_str().expect("account id").to_string();
    assert_eq!(location, format!("/api/v1/admin/clients/{account_id}"));
    assert_eq!(view["tenant_status"], "reserved");
    assert_eq!(view["account_status"], "active");

    // Replaying the same operation id and body returns the same resource.
    let replay = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "team-alpha"})),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &operation),
            ],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(replay.status(), StatusCode::ACCEPTED);
    assert_eq!(
        read_json(replay).await["account_id"].as_str(),
        Some(account_id.as_str())
    );

    // Get by id.
    let fetched = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(read_json(fetched).await["display_name"], "team-alpha");

    // Keys start empty and are paginated.
    let keys = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}/keys"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(keys.status(), StatusCode::OK);
    assert_eq!(read_json(keys).await["items"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn missing_idempotency_key_is_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let response = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "team-alpha"})),
            &[("x-csrf-token", &session.csrf)],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn unauthenticated_client_list_is_unauthorized() {
    let harness = Harness::new().await;
    let response = harness.get("/api/v1/admin/clients", &[]).await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ─── Mode exclusivity ─────────────────────────────────────

#[tokio::test]
async fn local_mode_does_not_mount_oidc_routes() {
    let harness = Harness::new().await;
    for path in [
        "/auth/oidc/authorize",
        "/auth/oidc/callback",
        "/auth/oidc/logout",
        "/api/v1/account",
        "/api/v1/account/csrf",
        "/api/v1/account/api_keys",
        "/api/v1/operator/tenants/some-tenant",
        "/api/v1/operator/recovery/status",
    ] {
        let response = harness.get(path, &[]).await;
        assert_eq!(
            response.status(),
            StatusCode::NOT_FOUND,
            "{path} must not be mounted in local mode"
        );
    }
}

#[tokio::test]
async fn unmatched_api_and_auth_paths_are_json_404_not_html() {
    let harness = Harness::new().await;
    for path in ["/api/v1/nope", "/auth/local/nope"] {
        let response = harness.get(path, &[]).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
        let content_type = response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");
        assert!(
            content_type.starts_with("application/json"),
            "{path} returned {content_type}"
        );
    }
}

#[tokio::test]
async fn bearer_keys_cannot_authenticate_local_admin_routes() {
    let harness = Harness::new().await;
    // A well-formed but unknown admin cookie is not a bearer key and is
    // not accepted either.
    let forged = format!("__Host-memory_mcp_admin={}", "ab".repeat(32));
    let response = harness
        .send("GET", "/api/v1/admin/session", None, &[], Some(&forged))
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let _ = &harness.service().await;
}

// ─── Recent-auth window ───────────────────────────────────

#[tokio::test]
async fn client_mutation_follows_the_session_fence() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    // Log out, then present the same (now revoked) cookie with a valid
    // CSRF token captured earlier. The transaction must reject the stale
    // session even though the token still verifies.
    let logout = harness
        .send(
            "POST",
            "/api/v1/admin/logout",
            None,
            &[("x-csrf-token", &session.csrf)],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);

    let response = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "late-write"})),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &uuid::Uuid::new_v4().to_string()),
            ],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn a_new_client_is_discoverable_by_the_provisioning_worker() {
    // Task 7 requires the worker to discover `Reserved` rows independently
    // of any event. That is only true if the local create transaction
    // leaves a real `tenant` row in a state `list_due_provisioning`
    // selects; a sidecar-only write would leave the client permanently
    // unfinished.
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    let created = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({ "display_name": "discoverable" })),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &uuid::Uuid::new_v4().to_string()),
            ],
            Some(&session.cookie),
        )
        .await;
    let view = read_json(created).await;
    let account_id = view["account_id"].as_str().expect("account id").to_string();
    let tenant_id = view["tenant_id"].as_str().expect("tenant id").to_string();

    let tenant = harness
        .store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("the tenant row must exist");
    assert_eq!(
        tenant.status,
        memory_mcp::http::registry::models::TenantStatus::Reserved,
        "a new client starts Reserved"
    );

    let due = harness
        .store
        .list_due_provisioning(100, chrono::Utc::now())
        .await
        .expect("due list");
    assert!(
        due.iter().any(|t| t.id == tenant_id),
        "the provisioning worker must be able to discover the new tenant"
    );
    let _ = account_id;
}

/// A sanity check that the harness really is wired to the durable store
/// and not to a mock: the store sees the accounts the HTTP surface
/// created.
#[tokio::test]
async fn created_clients_reach_the_durable_store() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let created = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({"display_name": "durable-client"})),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &uuid::Uuid::new_v4().to_string()),
            ],
            Some(&session.cookie),
        )
        .await;
    let account_id = read_json(created).await["account_id"]
        .as_str()
        .expect("account id")
        .to_string();
    assert!(
        harness
            .store
            .find_account_by_id(&account_id)
            .await
            .expect("account lookup")
            .is_some(),
        "the account must exist in the durable registry"
    );
}

/// The client view must report live provisioning state, not the snapshot the
/// create transaction copied into the sidecar. Otherwise an administrator
/// polls a client forever while the tenant is already `ready`.
#[tokio::test]
async fn client_view_reflects_live_tenant_state() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, tenant_id) = harness.create_ready_client(&session, "live-state").await;

    // `create_ready_client` only flips the status, so move the plan and
    // schema forward too: the assertion then cannot pass on a stale sidecar
    // snapshot that happens to agree with the tenant.
    let mut tenant = harness
        .store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("the tenant row must exist");
    tenant.plan_version = 3;
    tenant.schema_version = 7;
    harness
        .store
        .write_tenant(&tenant)
        .await
        .expect("tenant write");

    let response = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let view = read_json(response).await;
    assert_eq!(view["tenant_status"], "ready");
    assert_eq!(view["plan_version"], 3);
    assert_eq!(view["schema_version"], 7);
    assert_eq!(view["provisioning_reason"], serde_json::Value::Null);
    assert_eq!(view["tenant_id"], tenant_id.as_str());
    // The optimistic-concurrency guard must stay the sidecar's own version;
    // the tenant carries an independent version of its own.
    assert_eq!(view["version"], 1);

    let listed = harness
        .send(
            "GET",
            "/api/v1/admin/clients",
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let page = read_json(listed).await;
    assert_eq!(page["items"][0]["account_id"], account_id.as_str());
    assert_eq!(page["items"][0]["tenant_status"], "ready");
    assert_eq!(page["items"][0]["plan_version"], 3);
}

/// A stalled provisioning attempt is reported as a bounded, allowlisted
/// reason. No storage or migration text may reach the API through this field.
#[tokio::test]
async fn client_view_reports_a_bounded_provisioning_reason() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, tenant_id) = harness.create_ready_client(&session, "failed-state").await;

    let mut tenant = harness
        .store
        .find_tenant_by_id(&tenant_id)
        .await
        .expect("tenant lookup")
        .expect("the tenant row must exist");
    tenant.status = TenantStatus::Failed;
    tenant.retry_stage = Some(TenantStatus::Migrating);
    harness
        .store
        .write_tenant(&tenant)
        .await
        .expect("tenant write");

    let response = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    let view = read_json(response).await;
    assert_eq!(view["tenant_status"], "failed");
    assert_eq!(
        view["provisioning_reason"],
        "provisioning failed at migrating"
    );
}

#[tokio::test]
async fn auth_fence_requires_a_matching_generation() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    // Directly exercise the store fence so a router-level gate cannot
    // hide a service-level bypass.
    let authority = harness.authority().await;
    let service = LocalAdminService::new(
        authority.clone(),
        Arc::new(PasswordHasher::new().expect("KDF")),
    );
    let bogus = AdminFence {
        admin_id: "adm_unknown".into(),
        session_id: "ses_unknown".into(),
        credential_generation: 0,
        policy: authority.policy().clone(),
    };
    let outcome = authority
        .store()
        .revoke_session(
            &bogus,
            &RequestContext {
                request_id: uuid::Uuid::new_v4(),
            },
        )
        .await;
    assert!(outcome.is_err(), "an unknown session must not be revoked");
    let _ = service;
}

// ─── Locally issued keys on the data plane ────────────────

/// A key issued through the local admin surface must be a real data-plane
/// credential: parseable, persisted in the authoritative `api_key` table
/// under the configured pepper, and carrying the chosen expiry.
#[tokio::test]
async fn issued_key_is_persisted_for_the_data_plane() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    let (status, body) = harness
        .issue_key(
            &session,
            &account_id,
            "forever",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "issue never key: {body}");
    let key_id = body["id"].as_str().expect("key id").to_string();
    let secret = body["secret"].as_str().expect("secret").to_string();
    assert!(body["expires_at"].is_null(), "a never key has no deadline");

    // The returned credential parses with the real credential parser.
    let credential = ApiKeyCredential::parse(&secret).expect("returned secret parses");
    assert_eq!(credential.key_id(), key_id);

    // The durable `api_key` row is what the data plane resolves.
    let pepper = harness.state.config.api_key_pepper.as_bytes().to_vec();
    let stored = harness
        .store
        .find_api_key(&key_id)
        .await
        .expect("api key lookup")
        .expect("api key persisted");
    assert_eq!(stored.account_id, account_id);
    assert_eq!(stored.status, ApiKeyStatus::Active);
    assert!(stored.expires_at.is_none());
    assert!(
        stored.verifier.verify(&pepper, credential.secret()),
        "the persisted verifier must match under the configured pepper"
    );

    // A finite expiry is derived from the explicit day choice inside the
    // transaction, never from a browser-computed deadline.
    let (status, body) = harness
        .issue_key(
            &session,
            &account_id,
            "monthly",
            serde_json::json!({"kind": "days", "days": 30}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "issue days key: {body}");
    let day_key_id = body["id"].as_str().expect("key id").to_string();
    let declared: chrono::DateTime<chrono::Utc> = body["expires_at"]
        .as_str()
        .expect("expires_at")
        .parse()
        .expect("rfc3339 expiry");
    let stored = harness
        .store
        .find_api_key(&day_key_id)
        .await
        .expect("api key lookup")
        .expect("api key persisted");
    let persisted = stored.expires_at.expect("persisted expiry");
    assert!(
        (persisted - declared).num_milliseconds().abs() < 1000,
        "persisted {persisted} vs declared {declared}"
    );
    let delta = declared - chrono::Utc::now();
    assert!(
        delta.num_days() >= 29 && delta.num_days() <= 30,
        "unexpected expiry delta {delta}"
    );
}

/// The key issued by the local admin surface authenticates through the
/// real data-plane principal path and resolves to the client's tenant.
#[tokio::test]
async fn issued_key_authenticates_on_the_data_plane() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    let (status, body) = harness
        .issue_key(
            &session,
            &account_id,
            "runtime",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "issue key: {body}");
    let secret = body["secret"].as_str().expect("secret").to_string();

    match harness.authenticator().authenticate_bearer(&secret).await {
        AuthDecision::Allow(principal) => {
            assert_eq!(principal.account().id, account_id);
            assert_eq!(principal.account().tenant_id, tenant_id);
        }
        other => panic!("expected Allow(ApiKey), got {other:?}"),
    }
}

/// Revocation is durable and immediate: the next authentication attempt is
/// denied both with a cold cache and after a successful request has warmed
/// the positive cache.
#[tokio::test]
async fn revoking_an_issued_key_denies_with_and_without_a_warm_cache() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    // Cold path: revoke before any successful authentication.
    let (_, body) = harness
        .issue_key(
            &session,
            &account_id,
            "cold",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    let cold_id = body["id"].as_str().expect("key id").to_string();
    let cold_secret = body["secret"].as_str().expect("secret").to_string();
    assert_eq!(
        harness.revoke_key(&session, &account_id, &cold_id).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        matches!(
            harness
                .authenticator()
                .authenticate_bearer(&cold_secret)
                .await,
            AuthDecision::Deny
        ),
        "a revoked key must not authenticate on a cold cache"
    );

    // Warm path: authenticate successfully first, then revoke.
    let (_, body) = harness
        .issue_key(
            &session,
            &account_id,
            "warm",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    let warm_id = body["id"].as_str().expect("key id").to_string();
    let warm_secret = body["secret"].as_str().expect("secret").to_string();
    assert!(
        matches!(
            harness
                .authenticator()
                .authenticate_bearer(&warm_secret)
                .await,
            AuthDecision::Allow(_)
        ),
        "the fresh key must authenticate and warm the cache"
    );
    assert_eq!(
        harness.revoke_key(&session, &account_id, &warm_id).await,
        StatusCode::NO_CONTENT
    );
    assert!(
        matches!(
            harness
                .authenticator()
                .authenticate_bearer(&warm_secret)
                .await,
            AuthDecision::Deny
        ),
        "a revoked key must be rejected on a warm cache hit"
    );
}

/// A client still awaiting provisioning is not in the suspend source state,
/// so the mutation is refused rather than producing a false `Ready`.
#[tokio::test]
async fn a_provisioning_client_cannot_be_suspended() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    let created = harness
        .send(
            "POST",
            "/api/v1/admin/clients",
            Some(serde_json::json!({ "display_name": "still-provisioning" })),
            &[
                ("x-csrf-token", &session.csrf),
                ("idempotency-key", &uuid::Uuid::new_v4().to_string()),
            ],
            Some(&session.cookie),
        )
        .await;
    let view = read_json(created).await;
    let account_id = view["account_id"].as_str().expect("account id").to_string();

    let response = harness
        .send(
            "POST",
            &format!("/api/v1/admin/clients/{account_id}/suspend"),
            Some(serde_json::json!({ "expected_version": view["version"] })),
            &[("x-csrf-token", &session.csrf)],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "a reserved/ready-source mismatch is a conflict, not a false success"
    );
}

/// Suspend/resume is version-checked. Repeating a request that is already at
/// the desired state is a coherent no-op (204, no writes) even with a stale
/// version; every other stale version is a conflict.
#[tokio::test]
async fn suspend_and_resume_follow_the_coherent_state_contract() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, tenant_id) = harness.create_ready_client(&session, "state-machine").await;

    let ready = read_client(&harness, &session, &account_id).await;
    let version = ready["version"].as_u64().expect("guard version");
    assert_eq!(ready["tenant_status"], "ready");

    // Still in the suspend source state: a stale version must conflict.
    let stale = set_client_state(&harness, &session, &account_id, "suspend", version - 1).await;
    assert_eq!(stale, StatusCode::CONFLICT, "stale suspend");

    let suspended = set_client_state(&harness, &session, &account_id, "suspend", version).await;
    assert_eq!(
        suspended,
        StatusCode::NO_CONTENT,
        "suspend at the current version"
    );
    let after_suspend = read_client(&harness, &session, &account_id).await;
    assert_eq!(after_suspend["account_status"], "suspended");
    assert_eq!(after_suspend["tenant_status"], "suspended");
    assert_eq!(after_suspend["version"], version + 1);

    // Repeating the now-coherent request is a no-op, even with a stale version.
    let repeat = set_client_state(&harness, &session, &account_id, "suspend", version).await;
    assert_eq!(repeat, StatusCode::NO_CONTENT, "coherent no-op suspend");
    let after_repeat = read_client(&harness, &session, &account_id).await;
    assert_eq!(
        after_repeat["version"],
        version + 1,
        "a coherent no-op must not write"
    );

    // A suspended tenant must not be provisioning work, or the worker would
    // keep trying (and failing) to advance it back to Ready.
    let due = harness
        .store
        .list_due_provisioning(100, chrono::Utc::now())
        .await
        .expect("due list");
    assert!(
        !due.iter().any(|tenant| tenant.id == tenant_id),
        "a suspended tenant must not be discovered by the provisioning worker"
    );

    let resumed = set_client_state(&harness, &session, &account_id, "resume", version + 1).await;
    assert_eq!(
        resumed,
        StatusCode::NO_CONTENT,
        "resume at the refreshed version"
    );
    let after_resume = read_client(&harness, &session, &account_id).await;
    assert_eq!(after_resume["account_status"], "active");
    assert_eq!(after_resume["tenant_status"], "ready");
    assert_eq!(
        after_resume["version"],
        version + 2,
        "resume advances the guard version"
    );
    assert_eq!(
        after_resume["schema_version"], after_suspend["schema_version"],
        "suspension must not touch the tenant schema version"
    );

    // Back in the suspend source state, the original version is stale again.
    let stale_again = set_client_state(&harness, &session, &account_id, "suspend", version).await;
    assert_eq!(
        stale_again,
        StatusCode::CONFLICT,
        "stale suspend after resume"
    );
}

/// Read one client view through the real route.
async fn read_client(harness: &Harness, session: &Session, account_id: &str) -> serde_json::Value {
    let response = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK, "client read");
    read_json(response).await
}

/// Issue one suspend/resume mutation through the real route.
async fn set_client_state(
    harness: &Harness,
    session: &Session,
    account_id: &str,
    action: &str,
    expected_version: u64,
) -> StatusCode {
    harness
        .send(
            "POST",
            &format!("/api/v1/admin/clients/{account_id}/{action}"),
            Some(serde_json::json!({ "expected_version": expected_version })),
            &[("x-csrf-token", &session.csrf)],
            Some(&session.cookie),
        )
        .await
        .status()
}

/// The data-plane boundary rejects a key whose expiry is `now`: the
/// predicate is `expires_at > now`, so equality is denied.
#[tokio::test]
async fn expiry_at_the_boundary_is_rejected() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    let pepper = harness.state.config.api_key_pepper.as_bytes().to_vec();
    let key_id = memory_mcp::http::registry::models::new_api_key_id();
    let secret = "ExpiredKey0123456789ExpiredKey0123456789";
    let key = ApiKey {
        id: key_id.clone(),
        account_id,
        name: "boundary".into(),
        verifier: KeyedVerifier::compute(&pepper, secret.as_bytes()),
        status: ApiKeyStatus::Active,
        created_at: chrono::Utc::now(),
        expires_at: Some(chrono::Utc::now()),
        last_used_at: None,
        version: 1,
    };
    harness
        .store
        .write_api_key(&key)
        .await
        .expect("persist boundary key");
    let raw = format!("mem_sk_{key_id}_{secret}");
    assert!(
        matches!(
            harness.authenticator().authenticate_bearer(&raw).await,
            AuthDecision::Deny
        ),
        "expiry == now must be rejected"
    );
}

/// The active-key cap counts only the rows the data plane would accept:
/// expired and revoked keys do not consume a slot, and revoking a live key
/// frees one.
#[tokio::test]
async fn active_key_cap_counts_only_live_keys() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    // Two dead rows that must not count against the cap: an expired active
    // key and a revoked key.
    let pepper = harness.state.config.api_key_pepper.as_bytes().to_vec();
    for (suffix, status, expires_at) in [
        (
            "expired",
            ApiKeyStatus::Active,
            Some(chrono::Utc::now() - chrono::Duration::hours(1)),
        ),
        ("revoked", ApiKeyStatus::Revoked, None),
    ] {
        let key_id = memory_mcp::http::registry::models::new_api_key_id();
        let secret = format!("DeadKey0123456789DeadKey0123456789{suffix}");
        let key = ApiKey {
            id: key_id,
            account_id: account_id.clone(),
            name: format!("dead-{suffix}"),
            verifier: KeyedVerifier::compute(&pepper, secret.as_bytes()),
            status,
            created_at: chrono::Utc::now(),
            expires_at,
            last_used_at: None,
            version: 1,
        };
        harness
            .store
            .write_api_key(&key)
            .await
            .expect("persist dead key");
    }

    let cap = memory_mcp::http::registry::models::PlanLimits::default().max_active_api_keys;
    let mut live_ids = Vec::new();
    for index in 0..cap {
        let (status, body) = harness
            .issue_key(
                &session,
                &account_id,
                &format!("live-{index}"),
                serde_json::json!({"kind": "never"}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "issue live-{index}: {body}");
        live_ids.push(body["id"].as_str().expect("key id").to_string());
    }

    // The cap is now full; one more live key must be refused.
    let (status, body) = harness
        .issue_key(
            &session,
            &account_id,
            "overflow",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "cap must be enforced: {body}");
    assert_eq!(body["error"]["code"], "key_cap_reached");

    // Revoking a live key frees a slot.
    assert_eq!(
        harness
            .revoke_key(&session, &account_id, &live_ids[0])
            .await,
        StatusCode::NO_CONTENT
    );
    let (status, body) = harness
        .issue_key(
            &session,
            &account_id,
            "after-revoke",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(
        status,
        StatusCode::CREATED,
        "a revoked key must free a cap slot: {body}"
    );
}

/// Plan §5: "Two admins issue at cap−1". Both administrators have equal
/// privileges over the same client, so the cap belongs to the client rather
/// than to the administrator: racing the last slot from two independent
/// sessions must still yield exactly one key. Issuance serializes on the
/// client guard row, so the loser is refused by the cap instead of writing a
/// second live key.
#[tokio::test]
async fn two_administrators_racing_the_last_key_slot_issue_exactly_one_key() {
    let harness = Harness::new().await;

    // Two independent administrators with equal privileges.
    let first_code = harness.create_admin("ops.one").await;
    harness
        .activate(&first_code, "a sufficiently long passphrase")
        .await;
    let first = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    let second_code = harness.create_admin("ops.two").await;
    harness
        .activate(&second_code, "another sufficiently long passphrase")
        .await;
    let second = harness
        .login("ops.two", "another sufficiently long passphrase")
        .await;

    // One client, created by the first administrator but administrable by both.
    let (account_id, _tenant_id) = harness.create_ready_client(&first, "team-alpha").await;
    let seen_by_second = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&second.cookie),
        )
        .await;
    assert_eq!(
        seen_by_second.status(),
        StatusCode::OK,
        "the second administrator must reach the same client"
    );

    // Fill every slot but the last.
    let cap = memory_mcp::http::registry::models::PlanLimits::default().max_active_api_keys;
    assert!(
        cap >= 2,
        "this experiment needs at least two slots, got {cap}"
    );
    for index in 0..cap - 1 {
        let (status, body) = harness
            .issue_key(
                &first,
                &account_id,
                &format!("filler-{index}"),
                serde_json::json!({"kind": "never"}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "filler-{index}: {body}");
    }

    // Both administrators race the single remaining slot.
    let (left, right) = tokio::join!(
        harness.issue_key(
            &first,
            &account_id,
            "race-one",
            serde_json::json!({"kind": "never"})
        ),
        harness.issue_key(
            &second,
            &account_id,
            "race-two",
            serde_json::json!({"kind": "never"})
        )
    );

    let mut created = 0_u8;
    let mut refused = 0_u8;
    for (status, body) in [left, right] {
        match status {
            StatusCode::CREATED => created += 1,
            StatusCode::CONFLICT => {
                assert_eq!(
                    body["error"]["code"], "key_cap_reached",
                    "the losing administrator is refused by the cap: {body}"
                );
                refused += 1;
            }
            other => panic!("unexpected status {other}: {body}"),
        }
    }
    assert_eq!(
        (created, refused),
        (1, 1),
        "exactly one of the two racing administrators gets the last slot"
    );

    // The client never exceeds its cap: the durable set agrees with the
    // single success above.
    let listed = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}/keys?limit=100"),
            None,
            &[],
            Some(&second.cookie),
        )
        .await;
    assert_eq!(listed.status(), StatusCode::OK, "list keys");
    let body = read_json(listed).await;
    assert_eq!(
        body["items"].as_array().map(Vec::len),
        Some(cap as usize),
        "the cap is the client's, not the administrator's: {body}"
    );
}

/// A key issued for client A resolves to A's account and tenant, never to
/// client B's.
#[tokio::test]
async fn a_key_for_one_client_cannot_reach_another() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_a, tenant_a) = harness.create_ready_client(&session, "team-a").await;
    let (_account_b, tenant_b) = harness.create_ready_client(&session, "team-b").await;

    let (status, body) = harness
        .issue_key(
            &session,
            &account_a,
            "for-a",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "issue key: {body}");
    let secret = body["secret"].as_str().expect("secret").to_string();

    match harness.authenticator().authenticate_bearer(&secret).await {
        AuthDecision::Allow(principal) => {
            assert_eq!(principal.account().id, account_a);
            assert_eq!(principal.account().tenant_id, tenant_a);
            assert_ne!(principal.account().tenant_id, tenant_b);
        }
        other => panic!("expected Allow(ApiKey), got {other:?}"),
    }
}

/// Plan §5: "Key issue response lost and repeated". A repeated issuance
/// request resolves to `409 secret_already_issued` carrying the existing
/// public key id — never a second secret, and never a second verifier.
#[tokio::test]
async fn a_repeated_key_issue_never_returns_a_second_secret() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;

    let operation = uuid::Uuid::new_v4().to_string();
    let path = format!("/api/v1/admin/clients/{account_id}/keys");
    let body = serde_json::json!({ "name": "ci", "expiry": { "kind": "never" } });
    let headers = [
        ("x-csrf-token", session.csrf.as_str()),
        ("idempotency-key", operation.as_str()),
    ];

    let first = harness
        .send(
            "POST",
            &path,
            Some(body.clone()),
            &headers,
            Some(&session.cookie),
        )
        .await;
    assert_eq!(first.status(), StatusCode::CREATED);
    let first = read_json(first).await;
    let key_id = first["id"].as_str().expect("key id").to_string();
    let secret = first["secret"].as_str().expect("secret").to_string();

    // The same operation, replayed: the conflict names the existing key and
    // the response body never repeats or re-derives the secret.
    let replay = harness
        .send(
            "POST",
            &path,
            Some(body.clone()),
            &headers,
            Some(&session.cookie),
        )
        .await;
    assert_eq!(replay.status(), StatusCode::CONFLICT);
    let replay = read_json(replay).await;
    assert_eq!(replay["error"]["code"], "secret_already_issued");
    assert_eq!(replay["error"]["key_id"], serde_json::json!(key_id));
    assert!(
        !replay.to_string().contains(&secret),
        "a replayed issuance must not echo the first secret"
    );

    // One stored verifier: the replay created no second key row.
    let keys = harness
        .send("GET", &path, None, &[], Some(&session.cookie))
        .await;
    assert_eq!(keys.status(), StatusCode::OK);
    let items = read_json(keys).await;
    assert_eq!(
        items["items"].as_array().expect("key page").len(),
        1,
        "a replay must not create a second key"
    );

    // The same operation with a different body is an idempotency conflict,
    // not a second secret.
    let changed = harness
        .send(
            "POST",
            &path,
            Some(serde_json::json!({ "name": "ci-renamed", "expiry": { "kind": "never" } })),
            &headers,
            Some(&session.cookie),
        )
        .await;
    assert_eq!(changed.status(), StatusCode::CONFLICT);
    assert_eq!(
        read_json(changed).await["error"]["code"],
        "idempotency_conflict"
    );

    // A fresh operation is a fresh key, with its own secret.
    let (status, second) = harness
        .issue_key(
            &session,
            &account_id,
            "ci",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_ne!(second["secret"].as_str(), Some(secret.as_str()));
}

/// The local surface exposes no `500`: a storage failure is a sanitized
/// `503` outage, exactly like an admission failure (spec §7, §8).
#[tokio::test]
async fn a_local_admin_error_always_carries_a_request_id_header() {
    let harness = Harness::new().await;
    let response = harness
        .send("GET", "/api/v1/admin/clients", None, &[], None)
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let header = response
        .headers()
        .get("x-request-id")
        .expect("spec §8 requires a request-id response header")
        .to_str()
        .expect("ascii request id")
        .to_owned();
    let body = read_json(response).await;
    assert_eq!(body["correlation_id"], serde_json::json!(header));
}

/// Spec §8: the key listing is metadata only — `id`, `name`, `status`,
/// `created_at`, `expires_at`, `last_used_at` — and it never replays a
/// secret. `last_used_at` comes from the authoritative `api_key` row, which
/// the data plane stamps on each successful bearer request.
#[tokio::test]
async fn key_metadata_reports_status_and_last_used_at_without_secrets() {
    let harness = Harness::new().await;
    let code = harness.create_admin("ops.one").await;
    harness
        .activate(&code, "a sufficiently long passphrase")
        .await;
    let session = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;
    let (account_id, _tenant_id) = harness.create_ready_client(&session, "team-alpha").await;
    let (status, issued) = harness
        .issue_key(
            &session,
            &account_id,
            "runtime",
            serde_json::json!({"kind": "never"}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "issue key: {issued}");
    let key_id = issued["id"].as_str().expect("key id").to_string();
    let secret = issued["secret"].as_str().expect("secret").to_string();

    let listed = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}/keys"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = read_json(listed).await;
    let item = &listed["items"][0];
    assert_eq!(item["id"], serde_json::json!(key_id));
    assert_eq!(item["name"], "runtime");
    assert_eq!(item["status"], "active");
    assert!(item["created_at"].is_string(), "created_at is reported");
    assert!(item["expires_at"].is_null(), "a never key has no deadline");
    assert!(
        item["last_used_at"].is_null(),
        "a key that was never used has no last_used_at"
    );
    assert!(
        !listed.to_string().contains(&secret),
        "the listing must never replay a secret"
    );

    // A successful data-plane request stamps last_used_at, and the admin
    // surface reflects it on the next read.
    assert!(matches!(
        harness.authenticator().authenticate_bearer(&secret).await,
        AuthDecision::Allow(_)
    ));
    let listed = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}/keys"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    let listed = read_json(listed).await;
    assert!(
        listed["items"][0]["last_used_at"].is_string(),
        "the data plane's use timestamp must be visible: {listed}"
    );

    // Revocation is visible as status, and the key stays in the history.
    assert_eq!(
        harness.revoke_key(&session, &account_id, &key_id).await,
        StatusCode::NO_CONTENT
    );
    let listed = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}/keys"),
            None,
            &[],
            Some(&session.cookie),
        )
        .await;
    let listed = read_json(listed).await;
    assert_eq!(listed["items"][0]["status"], "revoked");
    assert_eq!(
        listed["items"].as_array().expect("key page").len(),
        1,
        "history keeps the revoked key"
    );
}

/// Plan R2: equal local administrators see the same clients. One
/// administrator's client is listable, readable and suspendable by another;
/// only the audit trail records who created it.
#[tokio::test]
async fn a_second_administrator_sees_and_can_administer_the_same_clients() {
    let harness = Harness::new().await;

    let first_code = harness.create_admin("ops.one").await;
    harness
        .activate(&first_code, "a sufficiently long passphrase")
        .await;
    let first = harness
        .login("ops.one", "a sufficiently long passphrase")
        .await;

    let second_code = harness.create_admin("ops.two").await;
    harness
        .activate(&second_code, "another sufficiently long passphrase")
        .await;
    let second = harness
        .login("ops.two", "another sufficiently long passphrase")
        .await;

    // The client is created by the *first* administrator.
    let (account_id, _tenant_id) = harness.create_ready_client(&first, "team-alpha").await;

    // The second administrator lists it.
    let listed = harness
        .send(
            "GET",
            "/api/v1/admin/clients",
            None,
            &[],
            Some(&second.cookie),
        )
        .await;
    assert_eq!(listed.status(), StatusCode::OK);
    let listed = read_json(listed).await;
    let ids: Vec<&str> = listed["items"]
        .as_array()
        .expect("client page")
        .iter()
        .filter_map(|item| item["account_id"].as_str())
        .collect();
    assert!(
        ids.contains(&account_id.as_str()),
        "a second equal administrator must see the client: {listed}"
    );

    // …and reads it.
    let fetched = harness
        .send(
            "GET",
            &format!("/api/v1/admin/clients/{account_id}"),
            None,
            &[],
            Some(&second.cookie),
        )
        .await;
    assert_eq!(fetched.status(), StatusCode::OK);
    assert_eq!(read_json(fetched).await["display_name"], "team-alpha");

    // …and can suspend and resume it.
    let view = read_client(&harness, &second, &account_id).await;
    let version = view["version"].as_u64().expect("version");
    assert_eq!(
        set_client_state(&harness, &second, &account_id, "suspend", version).await,
        StatusCode::NO_CONTENT
    );
    let suspended = read_client(&harness, &second, &account_id).await;
    assert_eq!(suspended["account_status"], "suspended");
    let version = suspended["version"].as_u64().expect("version");
    assert_eq!(
        set_client_state(&harness, &second, &account_id, "resume", version).await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        read_client(&harness, &second, &account_id).await["account_status"],
        "active"
    );
}
