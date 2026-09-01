//! HTTP middleware.
//!
//! Each middleware is `axum::middleware::from_fn`-compatible. Layer
//! ordering matters: layers added LATER wrap layers added EARLIER on
//! the request path.

use axum::http::{Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;

const MODERN_PROTOCOL_VERSION: &str = "2026-07-28";

/// Classification produced only after the mirrored MCP headers and the
/// JSON-RPC envelope have been checked against each other. Admission and
/// response lifetime code must never classify a request from a raw header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedMcpRequest {
    pub(crate) method: String,
    pub(crate) subscription: bool,
    /// UTF-8 byte length of an inline `ingest` content argument.
    /// `None` means this request is not an inline ingest or its
    /// arguments are not structurally valid enough to reserve quota.
    pub(crate) ingest_source_bytes: Option<u64>,
}

use super::HttpState;
use super::principal::auth::AuthDecision;

/// Reject every non-POST method on `/mcp`. Runs before
/// routing; all other paths pass through untouched. Defense in depth
/// on top of axum's own method matcher.
pub async fn reject_non_post_mcp(
    method: Method,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let path = req.uri().path();
    if path == "/mcp" && method != Method::POST {
        return Err((StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    Ok(next.run(req).await)
}

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

fn bad_request(message: impl Into<String>) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": null,
        "error": {
            "code": -32600,
            "message": message.into(),
        }
    });
    (
        StatusCode::BAD_REQUEST,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn plain_error(status: StatusCode, message: &'static str) -> Response {
    (status, message).into_response()
}

fn accepts_media_type(value: &str, expected: &str) -> bool {
    value.split(',').any(|part| {
        let mut parameters = part.trim().split(';');
        let Some(media_type) = parameters.next() else {
            return false;
        };
        if !media_type.trim().eq_ignore_ascii_case(expected) {
            return false;
        }
        parameters.all(|parameter| {
            let Some((name, raw_value)) = parameter.trim().split_once('=') else {
                return true;
            };
            if !name.trim().eq_ignore_ascii_case("q") {
                return true;
            }
            raw_value
                .trim()
                .parse::<f32>()
                .is_ok_and(|quality| quality > 0.0)
        })
    })
}

fn json_params(body: &Value) -> Option<&serde_json::Map<String, Value>> {
    body.get("params")?.as_object()
}

fn inline_ingest_source_bytes(body_method: &str, body: &Value) -> Option<u64> {
    if body_method != "tools/call" {
        return None;
    }
    let params = json_params(body)?;
    if params.get("name").and_then(Value::as_str) != Some("ingest") {
        return None;
    }
    let arguments = params.get("arguments")?.as_object()?.clone();
    let parsed: crate::tools::params::IngestParams =
        serde_json::from_value(Value::Object(arguments)).ok()?;
    u64::try_from(parsed.content.len()).ok()
}

fn quota_denied_response(reason: String, retry_after_secs: u32, guidance: String) -> Response {
    let body = serde_json::json!({
        "error": {
            "code": "quota_exceeded",
            "reason": reason,
            "guidance": guidance,
        }
    });
    let mut response = (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::CONTENT_TYPE, "application/json")],
        body.to_string(),
    )
        .into_response();
    if let Ok(value) = HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

/// Validate all request data that can affect routing, auth ordering, or
/// admission before any of those decisions are made. The body is restored
/// after bounded collection so rmcp still owns protocol dispatch and framing.
pub async fn prevalidate_mcp(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let headers = req.headers().clone();
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok());
    if !content_type.is_some_and(|value| {
        value
            .split(';')
            .next()
            .is_some_and(|media_type| media_type.trim().eq_ignore_ascii_case("application/json"))
    }) {
        return plain_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "application/json required",
        );
    }
    if headers
        .get(header::CONTENT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|encoding| !encoding.trim().eq_ignore_ascii_case("identity"))
    {
        return plain_error(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "content encoding unsupported",
        );
    }

    let accept = headers
        .get(header::ACCEPT)
        .and_then(|value| value.to_str().ok());
    if !accept.is_some_and(|value| {
        accepts_media_type(value, "application/json")
            && accepts_media_type(value, "text/event-stream")
    }) {
        return plain_error(
            StatusCode::NOT_ACCEPTABLE,
            "both MCP response media types required",
        );
    }

    if headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok())
        .is_some_and(|length| length > state.config.body_limit_bytes)
    {
        return plain_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    }

    let (parts, body) = req.into_parts();
    let body = http_body_util::Limited::new(body, state.config.body_limit_bytes);
    let bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return plain_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large"),
    };
    let value: Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(_) => return bad_request("invalid JSON-RPC request"),
    };
    if value.get("jsonrpc") != Some(&Value::String("2.0".to_owned())) {
        return bad_request("JSON-RPC version 2.0 is required");
    }
    if value.get("result").is_some() || value.get("error").is_some() {
        return bad_request("JSON-RPC responses are not accepted over MCP POST");
    }
    let Some(body_method) = value.get("method").and_then(Value::as_str) else {
        return bad_request("JSON-RPC method is required");
    };
    let Some(params) = json_params(&value) else {
        return bad_request("modern MCP metadata is required");
    };
    let Some(metadata) = params.get("_meta").and_then(Value::as_object) else {
        return bad_request("modern MCP metadata is required");
    };
    let metadata_version = metadata
        .get("io.modelcontextprotocol/protocolVersion")
        .and_then(Value::as_str);
    let protocol_header = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok());
    if protocol_header != Some(MODERN_PROTOCOL_VERSION)
        || metadata_version != Some(MODERN_PROTOCOL_VERSION)
        || protocol_header != metadata_version
    {
        return bad_request("HeaderMismatch: protocol version");
    }

    let method_header = headers
        .get("mcp-method")
        .and_then(|value| value.to_str().ok());
    if method_header != Some(body_method) {
        return bad_request("HeaderMismatch: MCP method");
    }

    let expected_name = match body_method {
        "tools/call" | "prompts/get" => params.get("name").and_then(Value::as_str),
        "resources/read" => params.get("uri").and_then(Value::as_str),
        _ => None,
    };
    if let Some(expected_name) = expected_name {
        if headers
            .get("mcp-name")
            .and_then(|value| value.to_str().ok())
            != Some(expected_name)
        {
            return bad_request("HeaderMismatch: MCP name");
        }
    } else if matches!(body_method, "tools/call" | "resources/read" | "prompts/get") {
        return bad_request("HeaderMismatch: MCP name");
    }

    let validated = ValidatedMcpRequest {
        method: body_method.to_string(),
        subscription: body_method == "subscriptions/listen",
        ingest_source_bytes: inline_ingest_source_bytes(body_method, &value),
    };
    let mut request = axum::http::Request::from_parts(parts, axum::body::Body::from(bytes));
    request.extensions_mut().insert(validated);
    next.run(request).await
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
    let decision = match header.as_deref() {
        Some(value) => {
            let mut parts = value.split_ascii_whitespace();
            match (parts.next(), parts.next(), parts.next()) {
                (Some(scheme), Some(credentials), None)
                    if scheme.eq_ignore_ascii_case("Bearer") =>
                {
                    state.authenticator.authenticate_bearer(credentials).await
                }
                _ => AuthDecision::Deny,
            }
        }
        None => AuthDecision::Deny,
    };
    let principal = match decision {
        AuthDecision::Allow(principal) => principal,
        _ => return unauthorized_response(),
    };
    req.extensions_mut().insert(principal);
    next.run(req).await
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
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                (name == "__Host-memory_mcp_session").then_some(value.to_owned())
            })
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
        .store_clone()
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

/// Tenant runtime acquisition. Runs
/// after `authenticate` so the principal is available. Resolves
/// the Tenant via `account_resolver`, acquires a global
/// admission permit (or a separate subscription permit for
/// `subscriptions/listen`), and acquires a pinned runtime from
/// the pool. On any error returns a clean 4xx/5xx so the
/// caller never sees a half-acquired state.
pub async fn acquire_runtime(
    axum::extract::State(state): axum::extract::State<Arc<HttpState>>,
    mut req: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    use crate::http::registry::account::ResolvedTenant;
    use axum::http::StatusCode;
    use axum::response::IntoResponse;

    let principal = match req
        .extensions()
        .get::<super::principal::AuthenticatedPrincipal>()
        .cloned()
    {
        Some(p) => p,
        None => {
            return (StatusCode::UNAUTHORIZED, "missing authenticated principal").into_response();
        }
    };
    let tenant = match state
        .account_resolver
        .resolve_ready_tenant(principal.account_id())
        .await
    {
        Ok(ResolvedTenant::Ready(t)) => t,
        Ok(ResolvedTenant::Provisioning(_, _)) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "tenant provisioning").into_response();
        }
        Ok(ResolvedTenant::Suspended) => {
            return (StatusCode::FORBIDDEN, "tenant suspended").into_response();
        }
        Ok(ResolvedTenant::Failed(_)) => {
            return (StatusCode::SERVICE_UNAVAILABLE, "tenant failed").into_response();
        }
        Ok(ResolvedTenant::NotFound) | Err(_) => {
            return (StatusCode::NOT_FOUND, "tenant not found").into_response();
        }
    };
    let validated = req
        .extensions()
        .get::<super::middleware::ValidatedMcpRequest>()
        .cloned();
    let is_subscription = validated
        .as_ref()
        .is_some_and(|request| request.subscription);
    let source_bytes = validated.and_then(|request| request.ingest_source_bytes);
    let store = state.registry.store_clone();
    let registry_plan = match store.load_plan(tenant.plan_version).await {
        Ok(plan) => plan,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "quota registry unavailable",
            )
                .into_response();
        }
    };
    let plan = crate::http::registry::plan::Plan::from(&registry_plan);
    if let Some(source_bytes) = source_bytes {
        let decision = match store
            .reserve_ingest_usage(&tenant.id, source_bytes, &plan, chrono::Utc::now())
            .await
        {
            Ok(decision) => decision,
            Err(_) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "quota registry unavailable",
                )
                    .into_response();
            }
        };
        if let crate::http::registry::plan::QuotaDecision::Deny {
            reason,
            retry_after_secs,
            guidance,
        } = decision
        {
            return quota_denied_response(reason, retry_after_secs, guidance);
        }
    }
    let permit = match state.admission.try_acquire_for(is_subscription) {
        Ok(p) => p,
        Err(()) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "admission capacity exhausted",
            )
                .into_response();
        }
    };
    let guard = match state
        .pool
        .acquire_or_wait_with_limit(&tenant, plan.per_tenant_request_concurrency)
        .await
    {
        Ok(g) => g,
        Err(_) => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                "runtime pool capacity exhausted",
            )
                .into_response();
        }
    };
    // The handler extracts these by `remove::<T>`. Wrap in
    // `Arc` to satisfy axum's `Extension<T: Clone>` bound.
    let permit_ref = super::runtime::guard::AdmissionPermitRef(std::sync::Arc::new(permit));
    let guard_ref = super::runtime::guard::OperationGuardRef(std::sync::Arc::new(guard));
    req.extensions_mut().insert(permit_ref);
    req.extensions_mut().insert(guard_ref);
    let mut resp = next.run(req).await;
    // Log context: hex of the first 8 bytes of the SHA-256 of
    // the tenant id. Cheap and stable across processes.
    use sha2::Digest;
    let digest = sha2::Sha256::digest(tenant.id.as_bytes());
    let fingerprint = hex::encode(&digest[..8]);
    resp.extensions_mut()
        .insert(crate::http::logging::TenantLogContext {
            credential_kind: principal.credential_kind().to_string(),
            tenant_fingerprint: fingerprint,
            ..Default::default()
        });
    resp
}

/// Wraps the inner service in `tokio::time::timeout(deadline, ...)`.
/// On timeout, returns 408 + `deadline exceeded` body. Runs as the
/// OUTERMOST request-side layer so all other middleware and the
/// handler itself are bounded by the same deadline.
pub async fn request_deadline(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Response {
    let deadline = state.config.request_deadline;
    let resp = tokio::time::timeout(deadline, next.run(req)).await;
    match resp {
        Ok(response) => response,
        Err(_elapsed) => {
            let body = format!("deadline exceeded after {:?}", deadline);
            let mut response = Response::new(axum::body::Body::from(body));
            *response.status_mut() = StatusCode::REQUEST_TIMEOUT;
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response
        }
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    use axum::Router;
    use axum::routing::get;
    use std::time::Duration;
    use tower_service::Service;

    async fn slow_stub() -> Response {
        tokio::time::sleep(Duration::from_secs(2)).await;
        Response::new(axum::body::Body::empty())
    }

    #[tokio::test]
    async fn request_deadline_with_tiny_budget_returns_408() {
        // Default config has 120s; override by mutating the inner
        // HttpState (we hold the only Arc reference here).
        let mut cfg = crate::http::config::HttpConfig::default_for_test();
        cfg.request_deadline = Duration::from_millis(1);
        cfg.shutdown_grace = Duration::from_millis(1);
        let mut state = crate::http::HttpState::default_for_test().await;
        let inner = std::sync::Arc::get_mut(&mut state).expect("single owner");
        inner.config = cfg;
        let mut svc =
            Router::new()
                .route("/", get(slow_stub))
                .layer(axum::middleware::from_fn_with_state(
                    state,
                    request_deadline,
                ));
        let req = axum::http::Request::builder()
            .uri("/")
            .body(axum::body::Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::REQUEST_TIMEOUT);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use tower_service::Service;

    async fn mcp_stub() -> &'static str {
        "ok"
    }

    fn router() -> Router {
        Router::new()
            .route("/mcp", post(mcp_stub))
            .route("/", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn(reject_non_post_mcp))
    }

    #[tokio::test]
    async fn get_on_mcp_returns_405_from_middleware() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"POST required");
    }

    #[tokio::test]
    async fn delete_on_mcp_returns_405_from_middleware() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::DELETE)
            .uri("/mcp")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&body[..], b"POST required");
    }

    #[tokio::test]
    async fn get_on_other_path_is_allowed() {
        let mut r = router();
        let req = Request::builder()
            .method(Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}

#[cfg(test)]
mod preflight_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, header};
    use axum::routing::post;
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
    use tower_service::Service;

    fn metadata() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": MODERN_PROTOCOL_VERSION,
            "io.modelcontextprotocol/clientInfo": {
                "name": "preflight-test",
                "version": "0.0.0"
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    fn modern_request(method: &str, params: Value) -> Request<Body> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });
        Request::builder()
            .method(Method::POST)
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", MODERN_PROTOCOL_VERSION)
            .header("Mcp-Method", method)
            .body(Body::from(body.to_string()))
            .expect("valid test request")
    }

    fn preflight_router(state: Arc<HttpState>) -> Router {
        Router::new()
            .route("/", post(echo_body))
            .layer(axum::middleware::from_fn_with_state(state, prevalidate_mcp))
    }

    async fn echo_body(mut request: Request<Body>) -> Response {
        let validated = request.extensions().get::<ValidatedMcpRequest>();
        let subscription = validated.is_some_and(|validated| validated.subscription);
        let ingest_source_bytes = validated.and_then(|validated| validated.ingest_source_bytes);
        let body = request
            .body_mut()
            .collect()
            .await
            .expect("body collection")
            .to_bytes();
        (
            StatusCode::OK,
            format!(
                "{subscription}:bytes={ingest_source_bytes:?}:{}",
                String::from_utf8_lossy(&body)
            ),
        )
            .into_response()
    }

    async fn dispatch(request: Request<Body>) -> Response {
        let state = HttpState::default_for_test().await;
        let mut router = preflight_router(state);
        router.call(request).await.expect("dispatch")
    }

    async fn response_body(response: Response) -> String {
        String::from_utf8(
            response
                .into_body()
                .collect()
                .await
                .expect("response body")
                .to_bytes()
                .to_vec(),
        )
        .expect("UTF-8 response")
    }

    #[tokio::test]
    async fn missing_protocol_header_returns_400_before_dispatch() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request.headers_mut().remove("MCP-Protocol-Version");
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("HeaderMismatch"));
    }

    #[tokio::test]
    async fn missing_method_header_returns_400_before_dispatch() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request.headers_mut().remove("Mcp-Method");
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("MCP method"));
    }

    #[tokio::test]
    async fn body_and_method_header_mismatch_returns_400() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request
            .headers_mut()
            .insert("Mcp-Method", "tools/call".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("HeaderMismatch"));
    }

    #[tokio::test]
    async fn valid_subscription_is_classified_and_body_is_restored() {
        let request = modern_request("subscriptions/listen", json!({"_meta": metadata()}));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        assert!(body.starts_with("true:"));
        assert!(body.contains("subscriptions/listen"));
    }

    #[tokio::test]
    async fn inline_ingest_uses_utf8_byte_length_for_quota() {
        let mut request = modern_request(
            "tools/call",
            json!({
                "_meta": metadata(),
                "name": "ingest",
                "arguments": {
                    "source_type": "inline",
                    "source_id": "bytes-test",
                    "content": "ёж",
                    "t_ref": "2026-01-01T00:00:00Z"
                }
            }),
        );
        request
            .headers_mut()
            .insert("Mcp-Name", "ingest".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_body(response).await.contains("bytes=Some(4)"));
    }

    #[tokio::test]
    async fn non_ingest_request_has_no_ingest_quota_size() {
        let response = dispatch(modern_request("tools/list", json!({"_meta": metadata()}))).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_body(response).await.contains("bytes=None"));
    }

    #[tokio::test]
    async fn structurally_invalid_ingest_arguments_do_not_reserve_quota() {
        let mut request = modern_request(
            "tools/call",
            json!({
                "_meta": metadata(),
                "name": "ingest",
                "arguments": {
                    "source_type": "inline",
                    "source_id": "invalid-test",
                    "content": 42,
                    "t_ref": "2026-01-01T00:00:00Z"
                }
            }),
        );
        request
            .headers_mut()
            .insert("Mcp-Name", "ingest".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response_body(response).await.contains("bytes=None"));
    }

    #[tokio::test]
    async fn quota_denial_returns_retry_after_and_stable_json() {
        let response = quota_denied_response(
            "ingested_bytes_exceeded".into(),
            17,
            "upgrade the tenant plan".into(),
        );
        assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(response.headers()[header::RETRY_AFTER], "17");
        let body = response_body(response).await;
        assert!(body.contains("quota_exceeded"));
        assert!(body.contains("ingested_bytes_exceeded"));
        assert!(body.contains("upgrade the tenant plan"));
    }

    #[tokio::test]
    async fn invalid_content_type_returns_415() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request
            .headers_mut()
            .insert(header::CONTENT_TYPE, "text/plain".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn incomplete_accept_returns_406() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request
            .headers_mut()
            .insert(header::ACCEPT, "application/json".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    }

    #[tokio::test]
    async fn unsupported_content_encoding_returns_415() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request
            .headers_mut()
            .insert(header::CONTENT_ENCODING, "gzip".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    }

    #[tokio::test]
    async fn declared_over_limit_body_returns_413() {
        let state = HttpState::default_for_test().await;
        let limit = state.config.body_limit_bytes;
        let mut router = preflight_router(state);
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request.headers_mut().insert(
            header::CONTENT_LENGTH,
            (limit + 1).to_string().parse().expect("header"),
        );
        let response = router.call(request).await.expect("dispatch");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn malformed_json_returns_400() {
        let request = Request::builder()
            .method(Method::POST)
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .body(Body::from("{"))
            .expect("valid test request");
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn invalid_jsonrpc_envelope_returns_400() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        let body = json!({
            "jsonrpc": "1.0",
            "id": 1,
            "method": "tools/list",
            "params": {"_meta": metadata()}
        });
        *request.body_mut() = Body::from(body.to_string());
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn zero_quality_response_media_type_returns_406() {
        let mut request = modern_request("tools/list", json!({"_meta": metadata()}));
        request.headers_mut().insert(
            header::ACCEPT,
            "application/json;q=0, text/event-stream;q=1"
                .parse()
                .expect("header"),
        );
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    }

    #[tokio::test]
    async fn missing_mcp_name_for_named_method_returns_400() {
        let request = modern_request(
            "tools/call",
            json!({"_meta": metadata(), "name": "remember"}),
        );
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("MCP name"));
    }

    #[tokio::test]
    async fn mismatched_mcp_name_returns_400() {
        let mut request = modern_request(
            "tools/call",
            json!({"_meta": metadata(), "name": "remember"}),
        );
        request
            .headers_mut()
            .insert("Mcp-Name", "other".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("MCP name"));
    }
}

// =======================================================================
// SSE response header injection
// =======================================================================

use axum::http::header::HeaderValue;

const NO_CACHE: HeaderValue = HeaderValue::from_static("no-cache");
const NO_BUFFERING: HeaderValue = HeaderValue::from_static("no");

/// Inject `Cache-Control: no-cache` and `X-Accel-Buffering: no` on
/// responses whose `Content-Type` starts with `text/event-stream`.
/// All other responses pass through untouched. Runs as the OUTERMOST
/// layer so it observes the final response after every other
/// middleware.
pub async fn inject_sse_headers(req: axum::extract::Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let is_sse = resp
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    if is_sse {
        let headers = resp.headers_mut();
        headers
            .entry(axum::http::header::CACHE_CONTROL)
            .or_insert(NO_CACHE);
        headers.entry("x-accel-buffering").or_insert(NO_BUFFERING);
    }
    resp
}

#[cfg(test)]
mod sse_headers_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::header;
    use axum::routing::get;
    use tower_service::Service;

    async fn sse_stub() -> (
        StatusCode,
        [(header::HeaderName, &'static str); 1],
        &'static str,
    ) {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/event-stream")],
            "data: {}\n\n",
        )
    }

    async fn json_stub() -> (
        StatusCode,
        [(header::HeaderName, &'static str); 1],
        &'static str,
    ) {
        (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            "{}",
        )
    }

    #[tokio::test]
    async fn sse_responses_get_no_cache_and_no_buffering_headers() {
        let mut r = Router::new()
            .route("/sse", get(sse_stub))
            .layer(axum::middleware::from_fn(inject_sse_headers));
        let req = axum::http::Request::builder()
            .uri("/sse")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers()
                .get("cache-control")
                .map(|v| v.to_str().unwrap()),
            Some("no-cache")
        );
        assert_eq!(
            resp.headers()
                .get("x-accel-buffering")
                .map(|v| v.to_str().unwrap()),
            Some("no")
        );
    }

    #[tokio::test]
    async fn json_responses_are_not_modified() {
        let mut r = Router::new()
            .route("/json", get(json_stub))
            .layer(axum::middleware::from_fn(inject_sse_headers));
        let req = axum::http::Request::builder()
            .uri("/json")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert!(resp.headers().get("x-accel-buffering").is_none());
        assert!(resp.headers().get("cache-control").is_none());
    }
}

/// Host/Origin allowlist middleware. Reads `allowed_hosts` and
/// `allowed_origins` from `HttpState`. Forwarding headers are honored
/// only when the request's peer IP matches a `trusted_proxy_cidrs`
/// entry; otherwise the bare `Host` header is used (defense against
/// spoofed `X-Forwarded-Host` from arbitrary clients).
pub async fn host_origin(
    axum::extract::State(state): axum::extract::State<std::sync::Arc<crate::http::HttpState>>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let headers = req.headers();
    let peer = req
        .extensions()
        .get::<axum::extract::connect_info::ConnectInfo<std::net::SocketAddr>>()
        .map(|info| info.0.ip());
    let trusted_peer = peer.is_some_and(|ip| {
        state
            .config
            .trusted_proxy_cidrs
            .iter()
            .any(|c| c.contains(ip))
    });
    let host_header = if trusted_peer {
        headers
            .get("x-forwarded-host")
            .or_else(|| headers.get("host"))
    } else {
        headers.get("host")
    };
    let host = host_header.and_then(|h| h.to_str().ok()).unwrap_or("");
    if !state
        .config
        .allowed_hosts
        .iter()
        .any(|h| h.eq_ignore_ascii_case(host))
    {
        return Err((StatusCode::FORBIDDEN, "host not allowed"));
    }
    if let Some(origin) = headers.get("origin").and_then(|h| h.to_str().ok())
        && !state.config.allowed_origins.iter().any(|o| o == origin)
    {
        return Err((StatusCode::FORBIDDEN, "origin not allowed"));
    }
    Ok(next.run(req).await)
}

#[cfg(test)]
mod host_origin_tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::Request;
    use axum::routing::get;
    use tower_service::Service;

    async fn echo() -> &'static str {
        "ok"
    }

    async fn router_with_host_origin() -> Router {
        let state = crate::http::HttpState::default_for_test().await;
        Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn_with_state(state, host_origin))
    }

    #[tokio::test]
    async fn rejects_disallowed_origin() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .header("origin", "https://evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn allows_missing_origin_for_non_browser() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "localhost")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn rejects_disallowed_host() {
        let mut r = router_with_host_origin().await;
        let req = Request::builder()
            .uri("/")
            .header("host", "evil.example")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    }
}

#[cfg(test)]
mod auth_tests {
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

    #[tokio::test]
    async fn router_level_wiring_validates_before_auth_and_keeps_health_public() {
        // Proves the spec-mandated ordering: malformed MCP requests fail
        // before auth, while a valid modern envelope reaches auth. The
        // health endpoint remains unauthenticated.
        use crate::http::router as build_router;
        let state = crate::http::HttpState::default_for_test().await;
        let router = build_router::build_router(state);
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
        let router2 = build_router::build_router(state2);
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
}
