//! HTTP SaaS transport.
//!
//! # rmcp API surface (verified 2026-10-02 against `rmcp 3.5.0`)
//!
//! The following types and methods are confirmed against the version resolved
//! by `Cargo.lock` (currently `rmcp 3.5.0`; the workspace requirement starts
//! at `3.4.0`). Line numbers are intentionally not treated as an API contract.
//!
//! - `rmcp::transport::streamable_http_server::StreamableHttpServerConfig`.
//! - `rmcp::transport::streamable_http_server::StreamableHttpService<S, M>`.
//! - `rmcp::transport::streamable_http_server::session::never::NeverSessionManager`.
//!
//! `StreamableHttpServerConfig` exposes the builder methods used below for
//! host/origin policy, SSE framing, stateless metadata, cancellation, and
//! bounded request bodies.
//!
//! # Dual-era serving
//!
//! `ServerHandler::supported_protocol_versions` has a default impl that
//! returns `Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)`, and this profile
//! keeps that default. The returned list is **not** only advertisement: rmcp
//! uses it as a hard membership check against every per-request
//! `_meta.protocolVersion` before dispatch, with no fallback. Narrowing it
//! would reject modern requests rather than legacy ones. Version narrowing for
//! the `initialize` handshake happens inside rmcp's
//! `negotiate_protocol_version`, which never answers a handshake with a
//! revision that has no handshake.
//!
//! # The two transport flags are not what they look like
//!
//! `with_legacy_session_mode` selects **sessions, not legacy support**.
//! `is_legacy_request` returns `true` for any `initialize` regardless of the
//! version it names, and `use_session = legacy_session_mode && is_legacy_request`.
//! Setting this flag to `true` therefore routes every legacy handshake into the
//! session path, which `NeverSessionManager` cannot serve. We keep it `false`,
//! which serves `initialize` through the same stateless path as modern
//! requests and emits no `Mcp-Session-Id`.
//!
//! `with_stateless_protocol_metadata_required` is `false` so that legacy
//! requests, which carry no per-request `_meta`, are not rejected. It does not
//! affect routing.
//!
//! `rmcp::model::ProtocolVersion` is a newtype struct with associated
//! constants `V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`,
//! `V_2025_11_25`, `V_2026_07_28`. `LATEST == V_2026_07_28`, while
//! `LATEST_WITH_INITIALIZE == V_2025_11_25` is the newest revision that still
//! has an `initialize` handshake.

use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio_util::sync::CancellationToken;

use super::config::HttpConfig;
use super::validation::with_body_deadline;

/// The modern protocol revision, and the revision named in the per-request
/// `_meta` a modern client must send. The profile is dual-era and does not
/// restrict clients to it: `supported_protocol_versions` advertises every known
/// revision, and rmcp refuses anything outside that set. Aliased to the
/// crate-level constant so the literal is declared in exactly one place.
pub use crate::mcp::handlers::PROTOCOL_VERSION_2026_07_28 as PROTOCOL_VERSION;

/// Subscriptions listen methods that produce a long-lived SSE
/// stream and must not be bounded by the request-deadline body wrap.
const SUBSCRIPTION_METHODS: &[&str] = &["subscriptions/listen"];

/// Single production config builder. Every construction of the rmcp
/// service goes through this function.
pub fn build_server_config(
    http: &HttpConfig,
    cancellation_token: CancellationToken,
) -> StreamableHttpServerConfig {
    StreamableHttpServerConfig::default()
        .with_allowed_hosts(http.allowed_hosts.iter().cloned())
        .with_allowed_origins(http.allowed_origins.iter().cloned())
        .with_legacy_session_mode(false)
        .with_stateless_protocol_metadata_required(false)
        .with_max_request_body_bytes(http.body_limit_bytes)
        .with_cancellation_token(cancellation_token)
        .with_sse_keep_alive(Some(Duration::from_secs(15)))
        .with_sse_retry(Some(Duration::from_secs(3)))
        .with_json_response(false) // SSE for everything; spec §4.1
}

pub fn build_mcp_service<H, F>(
    factory: F,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<H, NeverSessionManager>
where
    H: rmcp::ServerHandler + Clone + Send + 'static,
    F: Fn() -> Result<H, std::io::Error> + Send + Sync + 'static,
{
    StreamableHttpService::new(factory, Arc::new(NeverSessionManager::default()), config)
}

/// Production handler. The runtime guard and admission
/// permit were inserted by `acquire_runtime`; this handler
/// extracts them and moves the guard into a `ResponseLease`
/// wrapped around the body so the pin and permit are not
/// released until the body is fully consumed.
pub async fn mcp_handler(
    State(state): State<std::sync::Arc<super::HttpState>>,
    mut req: Request,
) -> Response {
    // Remove the wrapped (Arc-cloneable) permits. The
    // inner `OperationGuard` is then moved into the
    // `ResponseLease`; the `Arc<AdmissionPermit>` is dropped
    // by the response body when the stream ends.
    let Some(guard_ref) = req
        .extensions_mut()
        .remove::<super::runtime::guard::OperationGuardRef>()
    else {
        return with_body_deadline(
            axum::response::IntoResponse::into_response((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "runtime guard missing",
            )),
            Some(state.config.request_deadline),
        );
    };
    let Some(permit_ref) = req
        .extensions_mut()
        .remove::<super::runtime::guard::AdmissionPermitRef>()
    else {
        return with_body_deadline(
            axum::response::IntoResponse::into_response((
                axum::http::StatusCode::SERVICE_UNAVAILABLE,
                "admission permit missing",
            )),
            Some(state.config.request_deadline),
        );
    };
    let is_subscription = is_subscription_request(&req);
    let runtime = guard_ref.runtime().clone();
    let mut request_handler = runtime.mcp_service.clone();
    // Attach subscription authorization when the principal
    // is available. The auth middleware inserts it into
    // request extensions before this handler runs.
    if let Some(principal) = req
        .extensions()
        .get::<super::principal::AuthenticatedPrincipal>()
        .cloned()
    {
        request_handler = request_handler
            .with_subscription_authorization(principal, state.authenticator.clone())
            .with_subscription_limits(
                state.config.subscription_queue_capacity,
                state.config.subscription_auth_recheck,
            );
    }
    let svc = build_mcp_service(
        move || Ok(request_handler.clone()),
        build_server_config(&state.config, state.shutdown.token()),
    );
    let response = forward(svc, req).await;
    let (parts, body) = response.into_parts();
    // Shared ownership is intentional: tower/axum layers may clone
    // request extensions. The resources are released when the final
    // owner drops, without relying on a panic-prone uniqueness invariant.
    let operation = (!is_subscription).then_some(guard_ref.0);
    let lease = super::runtime::guard::ResponseLease::new(operation, permit_ref.0);
    let timeout = (!is_subscription).then_some(state.config.request_deadline);
    let body = super::validation::DeadlineBody::new(body, timeout);
    axum::response::Response::from_parts(
        parts,
        axum::body::Body::new(super::runtime::guard::LeasedBody::new(body, lease)),
    )
}

fn is_subscription_request(req: &Request) -> bool {
    req.extensions()
        .get::<super::middleware::ValidatedMcpRequest>()
        .is_some_and(|request| {
            request.subscription && SUBSCRIPTION_METHODS.contains(&request.method.as_str())
        })
}

/// Forward path shared by `mcp_handler` and the future pool-aware
/// variant. Type-erases the axum body, calls the rmcp service,
/// re-wraps the box body. `StreamableHttpService::Error = Infallible`.
pub async fn forward(
    mut svc: StreamableHttpService<crate::mcp::handlers::MemoryMcp, NeverSessionManager>,
    req: Request,
) -> Response {
    let (parts, body) = req.into_parts();
    let http_req: http::Request<axum::body::Body> = http::Request::from_parts(parts, body);
    match <_ as tower_service::Service<http::Request<axum::body::Body>>>::call(&mut svc, http_req)
        .await
    {
        Ok(resp) => resp.map(Body::new),
        Err(infallible) => match infallible {},
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ProtocolVersion;

    // Verifies the rmcp StreamableHttpServerConfig shape we hand to the
    // service builder: stateless, no legacy session mode, and no requirement
    // for per-request protocol metadata, which is what lets a legacy request
    // through. The profile is dual-era, so no version is pinned here.
    #[test]
    fn build_server_config_sets_required_knobs() {
        let cfg = HttpConfig::default_for_test();
        let token = CancellationToken::new();
        let out = build_server_config(&cfg, token);
        // Touch the field through Debug so the test fails if the
        // rmcp API renames anything we depend on.
        assert!(!format!("{out:?}").is_empty());
        // Dual-era serving depends on exactly these two: a legacy request is
        // admitted because per-request metadata is not required, and is
        // served without a session because legacy session mode stays off.
        // Flipping either one silently breaks every legacy client.
        assert!(
            !out.stateless_protocol_metadata_required,
            "legacy requests carry no per-request _meta and must not be required to"
        );
        assert!(
            !out.legacy_session_mode,
            "legacy session mode would route every initialize into a session \
             path that NeverSessionManager cannot serve"
        );
    }

    #[test]
    fn subscription_request_detection_uses_validated_body_metadata() {
        let mut req = axum::http::Request::builder()
            .header("mcp-method", "subscriptions/listen")
            .body(Body::empty())
            .unwrap();
        req.extensions_mut()
            .insert(super::super::middleware::ValidatedMcpRequest {
                method: "subscriptions/listen".to_owned(),
                subscription: true,
                ingest_source_bytes: None,
            });
        assert!(is_subscription_request(&req));

        let forged = axum::http::Request::builder()
            .header("mcp-method", "subscriptions/listen")
            .body(Body::empty())
            .unwrap();
        assert!(!is_subscription_request(&forged));

        let mut ordinary = axum::http::Request::builder()
            .header("mcp-method", "tools/call")
            .body(Body::empty())
            .unwrap();
        ordinary
            .extensions_mut()
            .insert(super::super::middleware::ValidatedMcpRequest {
                method: "tools/call".to_owned(),
                subscription: false,
                ingest_source_bytes: None,
            });
        assert!(!is_subscription_request(&ordinary));
    }

    #[test]
    fn protocol_version_alias_is_2026_07_28() {
        // Re-export alias; the canonical assertion lives in
        // mcp::handlers::tests.
        assert_eq!(PROTOCOL_VERSION, ProtocolVersion::V_2026_07_28);
    }
}
