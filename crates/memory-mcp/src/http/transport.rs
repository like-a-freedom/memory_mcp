//! HTTP SaaS transport (ADR-0052). Phase 3+ implementation lives here.
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

use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService,
};
use tokio_util::sync::CancellationToken;

use super::config::HttpConfig;

pub const PROTOCOL_VERSION: &str = "2026-07-28";

/// Single production config builder. Every construction of the rmcp
/// service goes through this function — no second default builder
/// exists (a second builder would be dead code under clippy -D).
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
    H: rmcp::ServerHandler + Send + 'static,
    F: Fn() -> Result<H, std::io::Error> + Send + Sync + 'static,
{
    StreamableHttpService::new(factory, Arc::new(NeverSessionManager::default()), config)
}

use axum::body::Body;
use axum::extract::{Request, State};
use axum::response::Response;

/// Phase 3 production handler: dispatches through the tenantless factory
/// in state. Task 5.6 replaces the body with runtime-pool dispatch.
pub async fn mcp_handler(
    State(state): State<std::sync::Arc<super::HttpState>>,
    req: Request,
) -> Response {
    let svc = build_mcp_service_from_arc(
        state.mcp_factory.clone(),
        build_server_config(&state.config, super::shutdown::cancellation_token()),
    );
    forward(svc, req).await
}

/// Builds the Phase 3 tenantless handler over the configured tenant
/// target. `mem://` selects the embedded in-memory engine (tests,
/// smoke runs); anything else connects to the remote endpoint.
pub async fn build_tenantless_handler(
    cfg: &HttpConfig,
) -> Result<std::sync::Arc<crate::mcp::handlers::MemoryMcp>, crate::error::MemoryError> {
    use crate::storage::DbClient;
    let t = &cfg.tenant_db;
    let client = if t.url == "mem://" {
        crate::storage::SurrealDbClient::connect_in_memory(&t.database, &t.namespace, "warn")
            .await?
    } else {
        crate::storage::SurrealDbClient::connect_bound(
            &t.url,
            &t.username,
            &t.password,
            &t.namespace,
            &t.database,
            "warn",
        )
        .await?
    };
    client.apply_migrations(&t.namespace).await?;
    let service = crate::service::MemoryService::new(
        std::sync::Arc::new(client),
        t.namespace.clone(),
        "warn".into(),
        100,
        100,
    )?;
    Ok(std::sync::Arc::new(
        crate::mcp::handlers::MemoryMcp::new_modern(service),
    ))
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
    match <_ as tower_service::Service<http::Request<axum::body::Body>>>::call(
        &mut svc, http_req,
    )
    .await
    {
        Ok(resp) => resp.map(Body::new),
        Err(infallible) => match infallible {},
    }
}

/// Helper: build a service from a closure that clones a pre-built
/// handler per request. Convenient for tests and the Phase 3
/// tenantless dispatch path.
pub fn build_mcp_service_from_arc<H>(
    factory: Arc<dyn Fn() -> Result<H, std::io::Error> + Send + Sync>,
    config: StreamableHttpServerConfig,
) -> StreamableHttpService<H, NeverSessionManager>
where
    H: rmcp::ServerHandler + Send + 'static,
{
    let f = move || factory();
    build_mcp_service(f, config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Method;

    // Mechanism: stateless_protocol_metadata_required = true rejects
    // legacy requests (they carry no per-request _meta protocol
    // version). 2025-03-26 is a KNOWN version, so the header check
    // alone would pass it.
    #[tokio::test]
    async fn unsupported_legacy_version_returns_bad_request() {
        use tower_service::Service;
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
}
