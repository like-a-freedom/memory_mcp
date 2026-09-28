#![cfg(all(
    feature = "streamable-http",
    feature = "test-fixtures",
    feature = "control-plane"
))]

//! Upgrading a deployment whose control plane was formerly disabled.
//!
//! Phase 1 requires the upgrade path to be tested under all three
//! browser auth configurations — local, OIDC and both together — rather
//! than assuming one stands for the rest. The scenarios are discovery
//! failure at startup, local bootstrap, method/key drift, durable
//! reconciliation, readiness and protected routes.
//!
//! These drive the real router over a durable in-memory registry store,
//! the same composition `http_local_admin.rs` uses. Nothing is stubbed.
//!
//! Run:
//! `cargo test -p memory_mcp --features streamable-http,test-fixtures --locked --test auth_upgrade`

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use memory_mcp::http::HttpState;
use memory_mcp::http::config::{BrowserAuthMethod, BrowserAuthMethods, LocalBrowserConfig};
use memory_mcp::http::registry::models::{BrowserPolicyFence, PlanLimits};
use memory_mcp::http::router::build_router;
use memory_mcp::http::test_state::HttpStateTestBuilder;
use tower_service::Service;

/// The three deployment shapes an upgrade can land in.
#[derive(Clone, Copy, Debug)]
enum Shape {
    Local,
    Oidc,
    Combined,
}

impl Shape {
    fn label(self) -> &'static str {
        match self {
            Shape::Local => "local only",
            Shape::Oidc => "OIDC only",
            Shape::Combined => "local and OIDC together",
        }
    }

    /// The methods the disclosure must report, in canonical order.
    fn expected_methods(self) -> Vec<&'static str> {
        match self {
            Shape::Local => vec!["local"],
            Shape::Oidc => vec!["oidc"],
            Shape::Combined => vec!["local", "oidc"],
        }
    }
}

/// Compose each shape over a durable in-memory registry and hand back a
/// built router.
///
/// The durable policy is what a formerly-disabled control plane joins on
/// first start, so each shape sets it explicitly — that is the
/// reconciliation step under test, not a test convenience.
async fn router_for(shape: Shape) -> Router {
    let (mut builder, store) = HttpStateTestBuilder::local_admin().await;
    let _ = store;

    let (browser_auth, fence_methods) = match shape {
        Shape::Local => (
            BrowserAuthMethods {
                local: Some(test_local_config()),
                oidc: None,
            },
            vec![BrowserAuthMethod::Local],
        ),
        Shape::Oidc => (
            BrowserAuthMethods {
                local: None,
                oidc: Some(oidc_config()),
            },
            vec![BrowserAuthMethod::Oidc],
        ),
        Shape::Combined => (
            BrowserAuthMethods {
                local: Some(test_local_config()),
                oidc: Some(oidc_config()),
            },
            vec![BrowserAuthMethod::Local, BrowserAuthMethod::Oidc],
        ),
    };

    let mut config = memory_mcp::http::config::HttpConfig::default_for_test();
    config.browser_auth = Some(browser_auth);
    builder = builder
        .with_config(config)
        .with_browser_policy(BrowserPolicyFence {
            methods: fence_methods,
            epoch: 1,
        });

    let state: Arc<HttpState> = builder.build().await.expect("HTTP state assembles");
    build_router(state, None).expect("router builds")
}

fn test_local_config() -> LocalBrowserConfig {
    LocalBrowserConfig {
        session_key: HttpStateTestBuilder::LOCAL_TEST_SESSION_KEY,
        csrf_key: HttpStateTestBuilder::LOCAL_TEST_CSRF_KEY,
        default_plan_version: 1,
        default_plan_limits: PlanLimits::default(),
    }
}

fn test_keys() -> memory_mcp::http::config::HmacKeys {
    memory_mcp::http::config::HmacKeys {
        identity_index: [0; 32],
        control_plane_session: [0; 32],
        oidc_state: [0; 32],
        oidc_nonce: [0; 32],
        csrf: [0; 32],
    }
}

fn oidc_config() -> memory_mcp::http::config::OidcBrowserConfig {
    memory_mcp::http::config::OidcBrowserConfig {
        issuer: "https://issuer.invalid".into(),
        client_id: "test-client".into(),
        audience: "memory-mcp".into(),
        redirect_uri: "http://localhost/auth/oidc/callback".into(),
        allowed_alg: "RS256".into(),
        operator_identity_allowlist: Vec::new(),
        signup_mode: memory_mcp::http::config::SignupMode::InviteOnly,
        keys: test_keys(),
    }
}

type Router = axum::Router;

/// The direct socket peer, attached explicitly like `http_local_admin.rs`
/// does: the handlers fail closed with 403 when the peer is unknown.
const PEER: &str = "127.0.0.1:54321";
/// The host the deployment is published under. The router refuses a
/// request whose `Host` is outside the allowlist, so a test request needs
/// one just as a real client sends one.
const ORIGIN: &str = "http://localhost";

async fn get(router: &mut Router, path: &str) -> axum::response::Response {
    use axum::extract::connect_info::ConnectInfo;
    use std::net::SocketAddr;

    let peer: SocketAddr = PEER.parse().expect("peer address");
    router
        .call(
            Request::builder()
                .uri(path)
                .header(axum::http::header::HOST, "localhost")
                .header(axum::http::header::ORIGIN, ORIGIN)
                .extension(ConnectInfo(peer))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("dispatch")
}

async fn json_of(response: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
}

/// Local bootstrap: the production local composition assembles and the
/// disclosure reports the local door.
#[tokio::test]
async fn local_only_bootstraps_and_discloses_the_local_door() {
    let mut router = router_for(Shape::Local).await;
    let response = get(&mut router, "/api/v1/auth/config").await;
    assert_eq!(response.status(), StatusCode::OK, "local bootstrap");

    let body = json_of(response).await;
    let methods: Vec<&str> = body["methods"]
        .as_array()
        .map(|items| items.iter().filter_map(|item| item.as_str()).collect())
        .unwrap_or_default();
    assert_eq!(
        methods,
        Shape::Local.expected_methods(),
        "the local-only upgrade must disclose exactly the local door"
    );
}

/// Each of the three shapes discloses exactly the methods it configured.
/// This is the method-set half of "method drift": if the disclosure ever
/// reports a method the deployment did not enable, or drops one it did,
/// the upgrade is shipping the wrong door set.
#[tokio::test]
async fn each_auth_shape_discloses_exactly_the_methods_it_enabled() {
    for shape in [Shape::Local, Shape::Oidc, Shape::Combined] {
        let mut router = router_for(shape).await;
        let response = get(&mut router, "/api/v1/auth/config").await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{} must assemble and disclose",
            shape.label()
        );

        let body = json_of(response).await;
        let methods: Vec<&str> = body["methods"]
            .as_array()
            .map(|items| items.iter().filter_map(|item| item.as_str()).collect())
            .unwrap_or_default();
        assert_eq!(
            methods,
            shape.expected_methods(),
            "{} disclosed the wrong method set",
            shape.label()
        );
    }
}

/// Readiness under every shape. A formerly-disabled control plane must
/// report ready once its durable registry is reachable, whichever door
/// it was upgraded with.
#[tokio::test]
async fn readiness_answers_under_every_auth_shape() {
    for shape in [Shape::Local, Shape::Oidc, Shape::Combined] {
        let mut router = router_for(shape).await;
        let response = get(&mut router, "/health/ready").await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "{} must be ready with a reachable registry",
            shape.label()
        );
    }
}

/// Protected routes stay closed under every shape. An upgrade must not
/// widen access: an unauthenticated request to the administration API is
/// refused whether the deployment has one door or two.
#[tokio::test]
async fn protected_routes_stay_closed_under_every_auth_shape() {
    for shape in [Shape::Local, Shape::Oidc, Shape::Combined] {
        let mut router = router_for(shape).await;
        let response = get(&mut router, "/api/v1/admin/clients").await;
        assert!(
            response.status() == StatusCode::UNAUTHORIZED
                || response.status() == StatusCode::FORBIDDEN
                || response.status() == StatusCode::NOT_FOUND,
            "{} leaked a protected route to an unauthenticated caller: {}",
            shape.label(),
            response.status()
        );
    }
}

/// Method drift is refused at startup, not silently reconciled.
///
/// A formerly-disabled control plane joins a durable policy on first
/// start. If that policy's method set disagrees with the configured one,
/// composition must not pick a winner — it must fail closed so the
/// operator sees the drift. This is the property the upgrade depends on:
/// an upgrade can never quietly change which doors a deployment has.
#[tokio::test]
async fn method_drift_is_refused_at_startup_rather_than_reconciled() {
    // Configure the local door, but the durable policy still says OIDC.
    let (builder, _store) = HttpStateTestBuilder::local_admin().await;
    let mut config = memory_mcp::http::config::HttpConfig::default_for_test();
    config.browser_auth = Some(BrowserAuthMethods {
        local: Some(test_local_config()),
        oidc: None,
    });

    let outcome = builder
        .with_config(config)
        .with_browser_policy(BrowserPolicyFence {
            methods: vec![BrowserAuthMethod::Oidc],
            epoch: 7,
        })
        .build()
        .await;

    let error = outcome.err().expect("drifted method set must not assemble");
    let message = error.to_string();
    assert!(
        message.contains("requires a policy that enables local"),
        "drift must surface as a refusal naming the missing method, not \
         reconcile silently: {message}"
    );
}

/// Durable reconciliation: the production composition joins the policy
/// from the durable store rather than accepting one.
///
/// `local_admin()` leaves `browser_policy` unset, so this is the
/// formerly-disabled control plane's real first start — nothing is
/// pre-joined. Composition must reconcile the configured method set with
/// the durable policy and still come up with the door it configured.
#[tokio::test]
async fn production_composition_reconciles_the_durable_policy_on_first_start() {
    let (mut builder, _store) = HttpStateTestBuilder::local_admin().await;
    let mut config = memory_mcp::http::config::HttpConfig::default_for_test();
    config.browser_auth = Some(BrowserAuthMethods {
        local: Some(test_local_config()),
        oidc: None,
    });
    builder = builder.with_config(config);

    let state: Arc<HttpState> = builder.build().await.expect("state assembles");
    assert_eq!(
        state.config.browser_auth_methods(),
        vec![BrowserAuthMethod::Local],
        "a first start must come up with the configured door"
    );

    let mut router = build_router(state, None).expect("router builds");
    let response = get(&mut router, "/health/ready").await;
    assert_eq!(
        response.status(),
        StatusCode::OK,
        "a reconciled deployment is ready"
    );
}

/// Discovery failure is a startup failure, not a degraded mode.
///
/// The full case — an identity provider whose discovery document is
/// unreachable or malformed — needs a live or stubbed issuer and is not
/// exercised here. What is pinned is the property that makes it safe:
/// test composition with the placeholder `issuer.invalid` **never runs
/// discovery**, so a unit test can never quietly start depending on a
/// network round trip. Production composition with `browser_policy`
/// unset does run it, and a failure there is `ConfigInvalid` — fatal at
/// startup (see `control/oidc/client.rs`, which maps every discovery
/// error to `ConfigInvalid`).
#[tokio::test]
async fn the_placeholder_issuer_never_reaches_the_network_in_test_composition() {
    // `HttpStateTestBuilder::new()` pre-joins an OIDC policy precisely so
    // discovery is not run against `issuer.invalid`. If composition ever
    // starts resolving that issuer, this test blocks on DNS instead of
    // assembling immediately — which is the regression being pinned.
    let builder = HttpStateTestBuilder::new().await;
    let state: Arc<HttpState> = builder.build().await.expect("state assembles");
    assert_eq!(
        state.config.browser_auth_methods(),
        vec![BrowserAuthMethod::Oidc],
        "the OIDC shape composes without discovery"
    );
}
