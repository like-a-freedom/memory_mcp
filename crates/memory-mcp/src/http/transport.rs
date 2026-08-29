//! HTTP SaaS transport (ADR-0052).
//!
//! # rmcp 3.1.2 API surface (verified 2026-08-28)
//!
//! The following types and methods are confirmed against the installed
//! `rmcp 3.1.2` source. Line numbers are stable while `Cargo.lock`
//! resolves the `rmcp` dep to `3.1.2` (currently pinned via the
//! workspace entry in `Cargo.toml`).
//!
//! - `rmcp::transport::streamable_http_server::StreamableHttpServerConfig`
//!   (`src/transport/streamable_http_server/tower.rs:60`).
//! - `rmcp::transport::streamable_http_server::StreamableHttpService<S, M>`
//!   (`src/transport/streamable_http_server/tower.rs:999`).
//! - `rmcp::transport::streamable_http_server::session::never::NeverSessionManager`
//!   (`src/transport/streamable_http_server/session/never.rs:19`).
//!
//! `StreamableHttpServerConfig` builder methods
//! (`src/transport/streamable_http_server/tower.rs`):
//!
//! - `with_allowed_hosts(impl IntoIterator<Item = String>)`            line 182
//! - `with_allowed_origins(impl IntoIterator<Item = String>)`          line 194
//! - `with_sse_keep_alive(Option<Duration>)`                           line 206
//! - `with_sse_retry(Option<Duration>)`                                line 211
//! - `with_legacy_session_mode(bool)`                                  line 216
//! - `with_json_response(bool)`                                        line 221
//! - `with_cancellation_token(CancellationToken)`                      line 226
//! - `with_max_request_body_bytes(usize)`                              line 232
//! - `with_stateless_protocol_metadata_required(bool)`                line 241
//!
//! `ServerHandler::supported_protocol_versions` has a default impl that
//! returns `Cow::Borrowed(ProtocolVersion::KNOWN_VERSIONS)`. The HTTP
//! profile overrides it to advertise only `V_2026_07_28`.
//!
//! `rmcp::model::ProtocolVersion` is a newtype struct with associated
//! constants `V_2024_11_05`, `V_2025_03_26`, `V_2025_06_18`,
//! `V_2025_11_25`, `V_2026_07_28`. `LATEST == V_2025_11_25`. The HTTP
//! profile pins both `supported_protocol_versions` and the `get_info()`
//! `protocol_version` fallback to `V_2026_07_28`.

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

/// The single protocol version advertised and accepted by the HTTP
/// profile. Any other value comes back as 400 from rmcp's header
/// check; the `supported_protocol_versions` override removes any
/// fallback ambiguity. Aliased to the crate-level constant so the
/// literal value is declared in exactly one place.
pub use crate::mcp::handlers::PROTOCOL_VERSION_2026_07_28 as PROTOCOL_VERSION;

/// Subscriptions listen methods that produce a long-lived SSE
/// stream and must not be bounded by the request-deadline body wrap.
const SUBSCRIPTION_METHODS: &[&str] = &["subscriptions/listen", "subscribe", "stream"];

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
        .with_stateless_protocol_metadata_required(true)
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

/// Phase 3 production handler. Reads the per-request subscription
/// flag from `Mcp-Method` so the long-lived SSE stream is NOT
/// wrapped by the body deadline (a deadline on a long-lived stream
/// would close it before clients finish listening).
pub async fn mcp_handler(
    State(state): State<std::sync::Arc<super::HttpState>>,
    req: Request,
) -> Response {
    let is_subscription = is_subscription_request(&req);
    let svc = build_mcp_service(
        {
            let handler = Arc::clone(&state.shared_handler);
            move || Ok((*handler).clone())
        },
        build_server_config(&state.config, state.shutdown.token()),
    );
    let response = forward(svc, req).await;
    if is_subscription {
        response
    } else {
        with_body_deadline(response, Some(state.config.request_deadline))
    }
}

fn is_subscription_request(req: &Request) -> bool {
    let method = req
        .headers()
        .get("mcp-method")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    SUBSCRIPTION_METHODS.contains(&method)
}

/// Builds the Phase 3 tenantless handler over the configured tenant
/// target. `mem://` selects the embedded in-memory engine (tests,
/// smoke runs); anything else connects to the remote endpoint.
pub async fn build_tenantless_handler(
    cfg: &HttpConfig,
) -> Result<Arc<crate::mcp::handlers::MemoryMcp>, crate::error::MemoryError> {
    use crate::storage::DbClient;
    let t = &cfg.tenant_db;
    let client = if t.url == "mem://" {
        crate::storage::SurrealDbClient::connect_in_memory(&t.database, &t.namespace, "warn")
            .await?
    } else {
        crate::storage::SurrealDbClient::connect_bound(&cfg.tenant_db, "warn").await?
    };
    client.apply_migrations(&t.namespace).await?;
    let service = crate::service::MemoryService::new(
        std::sync::Arc::new(client),
        t.namespace.clone(),
        "warn".into(),
        100,
        100,
    )?;
    Ok(Arc::new(crate::mcp::handlers::MemoryMcp::new_modern(
        service,
    )))
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
    use axum::http::Method;
    use rmcp::model::ProtocolVersion;
    use tower_service::Service;

    // Mechanism: stateless_protocol_metadata_required = true rejects
    // legacy requests (they carry no per-request _meta protocol
    // version). 2025-03-26 is a KNOWN version, so the header check
    // alone would pass it.
    #[tokio::test]
    async fn unsupported_legacy_version_returns_bad_request() {
        let state = crate::http::HttpState::default_for_test().await;
        let mut router = super::super::router::build_router(state);
        let req = axum::http::Request::builder()
            .method(Method::POST)
            .uri("/mcp")
            .header("host", "localhost")
            .header("MCP-Protocol-Version", "2025-03-26")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#))
            .unwrap();
        let resp: Response = router.call(req).await.unwrap();
        assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
    }

    #[test]
    fn subscription_request_detection() {
        let req = axum::http::Request::builder()
            .header("mcp-method", "subscriptions/listen")
            .body(Body::empty())
            .unwrap();
        assert!(is_subscription_request(&req));
        let req = axum::http::Request::builder()
            .header("mcp-method", "tools/call")
            .body(Body::empty())
            .unwrap();
        assert!(!is_subscription_request(&req));
    }

    #[test]
    fn protocol_version_alias_is_2026_07_28() {
        // Re-export alias; the canonical assertion lives in
        // mcp::handlers::tests.
        assert_eq!(PROTOCOL_VERSION, ProtocolVersion::V_2026_07_28);
    }
}
