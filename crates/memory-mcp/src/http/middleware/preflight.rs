//! Pre-MCP request validation.
//!
//! Runs before any auth or admission decision. Validates the MCP envelope
//! (JSON-RPC 2.0, protocol revision, mirrored-header agreement) and attaches a
//! [`ValidatedMcpRequest`] extension that downstream middleware and the
//! handler consume.
//!
//! The profile is dual-era (see ADR-0071). The era is decided by the request
//! body: a request carrying modern per-request `_meta` is modern-shaped and its
//! mirrored headers are required to agree; anything else is legacy-shaped,
//! where those headers are absent by design and tolerated when present. Either
//! way the extension is built from the body, never from a header, so admission
//! can never be steered by a header that disagrees with the dispatched request.

use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use bytes::{Bytes, BytesMut};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::preflight_budget::{PreflightRefusal, PreflightReservation};
use crate::http::HttpState;

/// Whether a revision belongs to the legacy era: a known revision that still
/// has an `initialize` handshake. Membership is tested against rmcp's own list
/// rather than by comparing dates, so a malformed or near-miss string is not
/// mistaken for a revision, and the set stays correct as revisions are added.
fn is_legacy_revision(version: &str) -> bool {
    rmcp::model::ProtocolVersion::KNOWN_VERSIONS
        .iter()
        .filter(|known| known.has_initialize())
        .any(|known| known.as_str() == version)
}

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

/// Reject every non-POST method on `/mcp`. Runs before
/// routing; all other paths pass through untouched. Defense in depth
/// on top of axum's own method matcher.
pub async fn reject_non_post_mcp(
    method: axum::http::Method,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    let path = req.uri().path();
    if path == "/mcp" && method != axum::http::Method::POST {
        return Err((StatusCode::METHOD_NOT_ALLOWED, "POST required"));
    }
    Ok(next.run(req).await)
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

pub(super) fn quota_denied_response(
    reason: String,
    retry_after_secs: u32,
    guidance: String,
) -> Response {
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
    if let Ok(value) = axum::http::HeaderValue::from_str(&retry_after_secs.to_string()) {
        response.headers_mut().insert(header::RETRY_AFTER, value);
    }
    response
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreflightOutcome {
    FullyCollected,
    RequestCapacityRefused,
    AggregateByteCapacityRefused,
    BodyLimitRefused,
    BodyReadError,
}

pub(crate) struct PreflightObservation {
    pub(crate) declared_content_length_bytes: Option<u64>,
    pub(crate) observed_body_bytes: u64,
    pub(crate) outcome: PreflightOutcome,
}

impl PreflightOutcome {
    pub(crate) fn refusal_reason(self) -> Option<&'static str> {
        match self {
            Self::RequestCapacityRefused => Some("request_capacity"),
            Self::AggregateByteCapacityRefused => Some("aggregate_byte_capacity"),
            Self::BodyLimitRefused => Some("body_limit"),
            Self::FullyCollected | Self::BodyReadError => None,
        }
    }
}

fn record_preflight_observation(observation: PreflightObservation, request_id: Option<uuid::Uuid>) {
    if observation.outcome == PreflightOutcome::FullyCollected {
        metrics::histogram!(crate::shared::observability::METRIC_HTTP_PREFLIGHT_BODY_BYTES)
            .record(observation.observed_body_bytes as f64);
    }
    let log_kind = match observation.outcome {
        PreflightOutcome::RequestCapacityRefused => Some(PreflightLogKind::RequestCapacity),
        PreflightOutcome::AggregateByteCapacityRefused => {
            Some(PreflightLogKind::AggregateByteCapacity)
        }
        PreflightOutcome::BodyLimitRefused => Some(PreflightLogKind::BodyLimit),
        PreflightOutcome::FullyCollected
            if observation.observed_body_bytes >= LARGE_BODY_LOG_THRESHOLD_BYTES =>
        {
            Some(PreflightLogKind::LargeBody)
        }
        PreflightOutcome::FullyCollected | PreflightOutcome::BodyReadError => None,
    };
    if let Some(reason) = observation.outcome.refusal_reason() {
        metrics::counter!(
            crate::shared::observability::METRIC_HTTP_PREFLIGHT_REFUSALS_TOTAL,
            "reason" => reason
        )
        .increment(1);
    }
    if let Some(log_kind) = log_kind {
        log_preflight_observation(log_kind, observation, request_id);
    }
}

const LARGE_BODY_LOG_THRESHOLD_BYTES: u64 = 65_536;
const PREFLIGHT_LOG_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Clone, Copy)]
enum PreflightLogKind {
    RequestCapacity,
    AggregateByteCapacity,
    BodyLimit,
    LargeBody,
}

impl PreflightLogKind {
    fn slot(self) -> usize {
        match self {
            Self::RequestCapacity => 0,
            Self::AggregateByteCapacity => 1,
            Self::BodyLimit => 2,
            Self::LargeBody => 3,
        }
    }

    fn operation(self) -> &'static str {
        match self {
            Self::LargeBody => "http.preflight.large_body",
            Self::RequestCapacity | Self::AggregateByteCapacity | Self::BodyLimit => {
                "http.preflight.refused"
            }
        }
    }
}

struct PreflightLogRateLimiter {
    last_emitted: [Option<Instant>; 4],
}

impl PreflightLogRateLimiter {
    fn new() -> Self {
        Self {
            last_emitted: [None; 4],
        }
    }

    fn allow_at(&mut self, slot: usize, now: Instant) -> bool {
        let allowed = self.last_emitted[slot].is_none_or(|last| {
            now.checked_duration_since(last)
                .is_some_and(|elapsed| elapsed >= PREFLIGHT_LOG_INTERVAL)
        });
        if allowed {
            self.last_emitted[slot] = Some(now);
        }
        allowed
    }
}

static PREFLIGHT_LOG_RATE_LIMITER: OnceLock<Mutex<PreflightLogRateLimiter>> = OnceLock::new();

fn log_preflight_observation(
    kind: PreflightLogKind,
    observation: PreflightObservation,
    request_id: Option<uuid::Uuid>,
) {
    let slot = kind.slot();
    let limiter =
        PREFLIGHT_LOG_RATE_LIMITER.get_or_init(|| Mutex::new(PreflightLogRateLimiter::new()));
    let should_log = limiter
        .lock()
        .map(|mut limiter| limiter.allow_at(slot, Instant::now()))
        .unwrap_or(false);
    if should_log {
        emit_preflight_log(kind, observation, request_id);
    }
}

fn emit_preflight_log(
    kind: PreflightLogKind,
    observation: PreflightObservation,
    request_id: Option<uuid::Uuid>,
) {
    let mut event = std::collections::HashMap::with_capacity(5);
    event.insert(
        "op".to_owned(),
        serde_json::Value::String(kind.operation().to_owned()),
    );
    event.insert(
        "outcome".to_owned(),
        serde_json::Value::String(
            match observation.outcome {
                PreflightOutcome::FullyCollected => "fully_collected",
                PreflightOutcome::RequestCapacityRefused => "request_capacity",
                PreflightOutcome::AggregateByteCapacityRefused => "aggregate_byte_capacity",
                PreflightOutcome::BodyLimitRefused => "body_limit",
                PreflightOutcome::BodyReadError => "body_read_error",
            }
            .to_owned(),
        ),
    );
    event.insert(
        "observed_body_bytes".to_owned(),
        serde_json::Value::from(observation.observed_body_bytes),
    );
    if let Some(declared_content_length_bytes) = observation.declared_content_length_bytes {
        event.insert(
            "declared_content_length_bytes".to_owned(),
            serde_json::Value::from(declared_content_length_bytes),
        );
    }
    if let Some(request_id) = request_id {
        event.insert(
            "request_id".to_owned(),
            serde_json::Value::String(request_id.to_string()),
        );
    }
    crate::logging::StdoutLogger::from_env().log(event, crate::logging::LogLevel::Debug);
}

struct BodyCollectionFailure {
    observed_body_bytes: u64,
    outcome: PreflightOutcome,
}

async fn collect_bounded_body(
    body: axum::body::Body,
    body_limit_bytes: usize,
    reservation: &mut PreflightReservation,
) -> Result<(Bytes, u64), BodyCollectionFailure> {
    let mut body = http_body_util::Limited::new(body, body_limit_bytes);
    let mut collected = BytesMut::new();
    let mut observed_body_bytes = 0_u64;

    while let Some(frame) = body.frame().await {
        let frame = match frame {
            Ok(frame) => frame,
            Err(error) => {
                let outcome = if error.is::<http_body_util::LengthLimitError>() {
                    PreflightOutcome::BodyLimitRefused
                } else {
                    PreflightOutcome::BodyReadError
                };
                return Err(BodyCollectionFailure {
                    observed_body_bytes,
                    outcome,
                });
            }
        };
        if let Ok(data) = frame.into_data() {
            observed_body_bytes =
                observed_body_bytes.saturating_add(u64::try_from(data.len()).unwrap_or(u64::MAX));
            let total_bytes = collected
                .len()
                .checked_add(data.len())
                .filter(|total| *total <= body_limit_bytes)
                .ok_or(BodyCollectionFailure {
                    observed_body_bytes,
                    outcome: PreflightOutcome::BodyLimitRefused,
                })?;
            reservation
                .try_reserve_through(total_bytes)
                .map_err(|_| BodyCollectionFailure {
                    observed_body_bytes,
                    outcome: PreflightOutcome::AggregateByteCapacityRefused,
                })?;
            collected.extend_from_slice(&data);
        }
    }

    Ok((collected.freeze(), observed_body_bytes))
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

    let request_id = req
        .extensions()
        .get::<crate::http::logging::RequestId>()
        .map(crate::http::logging::RequestId::as_uuid);
    let content_length = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    let declared_content_length_bytes =
        content_length.and_then(|length| u64::try_from(length).ok());
    if content_length.is_some_and(|length| length > state.config.body_limit_bytes) {
        record_preflight_observation(
            PreflightObservation {
                declared_content_length_bytes,
                observed_body_bytes: 0,
                outcome: PreflightOutcome::BodyLimitRefused,
            },
            request_id,
        );
        return plain_error(StatusCode::PAYLOAD_TOO_LARGE, "request body too large");
    }

    let mut reservation = match state
        .preflight_budget
        .try_reserve(content_length.unwrap_or(0))
    {
        Ok(reservation) => reservation,
        Err(refusal) => {
            let outcome = match refusal {
                PreflightRefusal::Requests => PreflightOutcome::RequestCapacityRefused,
                PreflightRefusal::Bytes => PreflightOutcome::AggregateByteCapacityRefused,
            };
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes,
                    observed_body_bytes: 0,
                    outcome,
                },
                request_id,
            );
            return plain_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "preflight buffering capacity exhausted",
            );
        }
    };

    let (parts, body) = req.into_parts();
    let bytes = match collect_bounded_body(body, state.config.body_limit_bytes, &mut reservation)
        .await
    {
        Ok((bytes, observed_body_bytes)) => {
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes,
                    observed_body_bytes,
                    outcome: PreflightOutcome::FullyCollected,
                },
                request_id,
            );
            bytes
        }
        Err(failure) => {
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes,
                    observed_body_bytes: failure.observed_body_bytes,
                    outcome: failure.outcome,
                },
                request_id,
            );
            let (status, message) = match failure.outcome {
                PreflightOutcome::BodyLimitRefused => {
                    (StatusCode::PAYLOAD_TOO_LARGE, "request body too large")
                }
                PreflightOutcome::AggregateByteCapacityRefused => (
                    StatusCode::SERVICE_UNAVAILABLE,
                    "preflight buffering capacity exhausted",
                ),
                PreflightOutcome::BodyReadError => {
                    (StatusCode::PAYLOAD_TOO_LARGE, "request body too large")
                }
                PreflightOutcome::FullyCollected | PreflightOutcome::RequestCapacityRefused => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "preflight collection failed",
                ),
            };
            return plain_error(status, message);
        }
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
    let params = json_params(&value);
    let protocol_header = headers
        .get("mcp-protocol-version")
        .and_then(|value| value.to_str().ok());
    let modern_version = params
        .and_then(|params| params.get("_meta"))
        .and_then(|meta| {
            meta.get("io.modelcontextprotocol/protocolVersion")
                .and_then(Value::as_str)
        });

    // The era is read from the body, never from a header, so no client can
    // present a header that disagrees with the request rmcp dispatches on.
    // See ADR-0071.
    match modern_version {
        // Modern era: the revision is declared per request in `_meta` and
        // mirrored in the header. SEP-2243 requires both to agree.
        Some(modern_version) => {
            if protocol_header != Some(modern_version) {
                return bad_request("HeaderMismatch: protocol version");
            }
        }
        // Legacy era: no per-request metadata. `initialize` names its revision
        // in `params.protocolVersion`; later requests may carry it in the
        // header alone. Neither may name a revision that has no handshake, and
        // where both are present they must agree.
        //
        // An `initialize` is the one request where the named revision is a
        // *proposal*, not a selection: rmcp's negotiation answers it with a
        // supported legacy revision, falling back when the proposal is unknown.
        // Checking membership here would refuse that proposal before
        // authentication, which is exactly the failure ADR-0071 removed for
        // known revisions. Later requests have already been answered by a
        // handshake, so their revision must be one this server negotiated.
        None => {
            let declared = params
                .and_then(|params| params.get("protocolVersion"))
                .and_then(Value::as_str);
            if body_method != "initialize"
                && let Some(declared) = declared
                && !is_legacy_revision(declared)
            {
                return bad_request("HeaderMismatch: protocol version");
            }
            if let Some(protocol_header) = protocol_header {
                if !is_legacy_revision(protocol_header) {
                    return bad_request("HeaderMismatch: protocol version");
                }
                if declared.is_some_and(|declared| declared != protocol_header) {
                    return bad_request("HeaderMismatch: protocol version");
                }
            }
        }
    }

    let method_header = headers
        .get("mcp-method")
        .and_then(|value| value.to_str().ok());
    if modern_version.is_some() {
        if method_header != Some(body_method) {
            return bad_request("HeaderMismatch: MCP method");
        }
    } else if method_header.is_some_and(|method_header| method_header != body_method) {
        // A legacy client need not send the mirrored header, but one that does
        // must not contradict the body.
        return bad_request("HeaderMismatch: MCP method");
    }

    let expected_name = params.and_then(|params| match body_method {
        "tools/call" | "prompts/get" => params.get("name").and_then(Value::as_str),
        "resources/read" => params.get("uri").and_then(Value::as_str),
        _ => None,
    });
    let name_header = headers
        .get("mcp-name")
        .and_then(|value| value.to_str().ok());
    if modern_version.is_some() {
        // Modern era: the mirrored name is required and must agree.
        if let Some(expected_name) = expected_name {
            if name_header != Some(expected_name) {
                return bad_request("HeaderMismatch: MCP name");
            }
        } else if matches!(body_method, "tools/call" | "resources/read" | "prompts/get") {
            return bad_request("HeaderMismatch: MCP name");
        }
    } else if name_header
        .zip(expected_name)
        .is_some_and(|(name_header, expected_name)| name_header != expected_name)
    {
        // Legacy era: `Mcp-Name` is a modern-era header a legacy client has
        // never heard of, so its absence is normal. One that is present must
        // still agree with the body.
        return bad_request("HeaderMismatch: MCP name");
    }

    let validated = ValidatedMcpRequest {
        method: body_method.to_string(),
        subscription: body_method == "subscriptions/listen",
        ingest_source_bytes: inline_ingest_source_bytes(body_method, &value),
    };
    drop(value);
    let mut request = axum::http::Request::from_parts(parts, axum::body::Body::from(bytes));
    request.extensions_mut().insert(validated);
    let response = next.run(request).await;
    drop(reservation);
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};

    /// The modern revision, for the helpers that build modern-shaped requests.
    use super::super::super::transport::PROTOCOL_VERSION;
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
            .method(axum::http::Method::GET)
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
            .method(axum::http::Method::DELETE)
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
            .method(axum::http::Method::GET)
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = r.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    fn metadata() -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": PROTOCOL_VERSION,
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
            .method(axum::http::Method::POST)
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", PROTOCOL_VERSION.as_str())
            .header("Mcp-Method", method)
            .body(Body::from(body.to_string()))
            .expect("valid test request")
    }

    /// A legacy-shaped request: no per-request `_meta`, no mirrored headers,
    /// and the revision named in the header alone. This is what a `2025-11-25`
    /// client actually sends. See ADR-0071.
    fn legacy_request(method: &str, params: Value) -> Request<Body> {
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": params
        });
        Request::builder()
            .method(axum::http::Method::POST)
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
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
    async fn legacy_ingest_reserves_quota_without_mirrored_headers() {
        // A legacy-shaped request must still populate `ValidatedMcpRequest`
        // from the body. If it did not, the extension would be absent and
        // `acquire_runtime` would skip the ingest quota pre-reservation
        // entirely, with no error anywhere. See ADR-0071.
        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "tools/call",
            "params": {
                "name": "ingest",
                "arguments": {
                    "source_type": "inline",
                    "source_id": "legacy-quota-test",
                    "content": "ёж",
                    "t_ref": "2026-01-01T00:00:00Z"
                }
            }
        });
        let request = Request::builder()
            .method(axum::http::Method::POST)
            .uri("/")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", "2025-11-25")
            .body(Body::from(body.to_string()))
            .expect("valid test request");
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::OK);
        // `echo_body` renders as `<subscription>:bytes=<n>:<body>`.
        let echoed = response_body(response).await;
        assert!(
            echoed.starts_with("false:bytes=Some(4):"),
            "a legacy ingest must still be classified and must still reserve \
             its quota: {echoed}"
        );
    }

    #[tokio::test]
    async fn legacy_request_need_not_send_mirrored_headers() {
        // `Mcp-Method` and `Mcp-Name` are 2026-07-28 headers. A legacy client
        // has never heard of them, so requiring either would refuse every
        // legacy tool call.
        let response = dispatch(legacy_request("tools/call", json!({"name": "ingest"}))).await;
        assert_eq!(
            response.status(),
            StatusCode::OK,
            "a legacy tools/call must not require the modern mirrored headers"
        );
    }

    #[tokio::test]
    async fn legacy_request_with_contradicting_mcp_name_is_rejected() {
        // Absence is tolerated on the legacy path; a present-and-wrong value
        // is not.
        let mut request = legacy_request("tools/call", json!({"name": "ingest"}));
        request
            .headers_mut()
            .insert("Mcp-Name", "other".parse().expect("header"));
        let response = dispatch(request).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(response_body(response).await.contains("MCP name"));
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
            .method(axum::http::Method::POST)
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

#[cfg(test)]
mod observation_tests {
    use std::time::{Duration, Instant};

    use super::{
        PreflightLogKind, PreflightLogRateLimiter, PreflightObservation, PreflightOutcome,
        emit_preflight_log, record_preflight_observation,
    };

    #[test]
    fn refusal_reason_uses_closed_vocabulary() {
        assert_eq!(
            PreflightOutcome::RequestCapacityRefused.refusal_reason(),
            Some("request_capacity")
        );
        assert_eq!(
            PreflightOutcome::AggregateByteCapacityRefused.refusal_reason(),
            Some("aggregate_byte_capacity")
        );
        assert_eq!(
            PreflightOutcome::BodyLimitRefused.refusal_reason(),
            Some("body_limit")
        );
        assert_eq!(PreflightOutcome::FullyCollected.refusal_reason(), None);
        assert_eq!(PreflightOutcome::BodyReadError.refusal_reason(), None);
    }

    #[test]
    fn preflight_log_rate_limit_is_fixed_and_bounded() {
        let mut limiter = PreflightLogRateLimiter::new();
        let start = Instant::now();
        let one_second_early = start + Duration::from_millis(999);
        let one_second_later = start + Duration::from_secs(1);

        assert!(limiter.allow_at(PreflightLogKind::RequestCapacity.slot(), start));
        assert!(!limiter.allow_at(PreflightLogKind::RequestCapacity.slot(), one_second_early));
        assert!(limiter.allow_at(
            PreflightLogKind::AggregateByteCapacity.slot(),
            one_second_early
        ));
        assert!(limiter.allow_at(PreflightLogKind::BodyLimit.slot(), start));
        assert!(limiter.allow_at(PreflightLogKind::LargeBody.slot(), start));
        assert!(limiter.allow_at(PreflightLogKind::RequestCapacity.slot(), one_second_later));
    }

    #[tokio::test]
    async fn large_body_log_requires_fully_collected_threshold() {
        let sink = crate::logging::capture::install();
        let below_threshold = uuid::Uuid::from_u128(101);
        let at_threshold = uuid::Uuid::from_u128(102);
        let refused = uuid::Uuid::from_u128(103);
        let read_error = uuid::Uuid::from_u128(104);

        crate::logging::capture::with_level("http.preflight=debug", || async {
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes: None,
                    observed_body_bytes: 65_535,
                    outcome: PreflightOutcome::FullyCollected,
                },
                Some(below_threshold),
            );
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes: None,
                    observed_body_bytes: 65_536,
                    outcome: PreflightOutcome::FullyCollected,
                },
                Some(at_threshold),
            );
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes: None,
                    observed_body_bytes: 65_536,
                    outcome: PreflightOutcome::BodyLimitRefused,
                },
                Some(refused),
            );
            record_preflight_observation(
                PreflightObservation {
                    declared_content_length_bytes: None,
                    observed_body_bytes: 65_536,
                    outcome: PreflightOutcome::BodyReadError,
                },
                Some(read_error),
            );
        })
        .await;

        let lines = sink.lines();
        let large_body_lines: Vec<_> = lines
            .iter()
            .filter(|line| line.contains("http.preflight.large_body"))
            .collect();
        assert_eq!(large_body_lines.len(), 1, "{lines:?}");
        assert!(large_body_lines[0].contains(&at_threshold.to_string()));
        assert!(!large_body_lines[0].contains(&below_threshold.to_string()));
        assert!(!large_body_lines[0].contains(&refused.to_string()));
        assert!(!large_body_lines[0].contains(&read_error.to_string()));
        assert!(large_body_lines[0].contains("fully_collected"));
    }

    #[tokio::test]
    async fn preflight_refusal_log_contains_only_allowlisted_fields() {
        let sink = crate::logging::capture::install();
        let observation = PreflightObservation {
            declared_content_length_bytes: Some(8),
            observed_body_bytes: 0,
            outcome: PreflightOutcome::BodyLimitRefused,
        };
        let request_id = uuid::Uuid::from_u128(1);

        // The process-wide rate limiter is tested separately; exercise the same
        // emitter directly so a parallel refusal cannot consume this sample.
        crate::logging::capture::with_level("http.preflight=debug", || async {
            emit_preflight_log(PreflightLogKind::BodyLimit, observation, Some(request_id));
        })
        .await;

        let lines = sink.lines();
        let expected_request = format!("req={request_id}");
        let event = lines.iter().find(|line| {
            line.contains("http.preflight.refused") && line.contains(&expected_request)
        });
        let event = event.unwrap_or_else(|| panic!("preflight refusal debug event: {lines:?}"));
        let event = event.replace("\u{1b}[0m", "");
        assert!(event.contains("body_limit"), "{event}");
        assert!(event.contains("declared_content_length_bytes=8"), "{event}");
        assert!(event.contains("observed_body_bytes=0"), "{event}");
        assert!(
            event.contains("req=00000000-0000-0000-0000-000000000001"),
            "{event}"
        );
        assert!(!event.contains("header-secret"), "{event}");
        assert!(!event.contains("body-secret"), "{event}");
        assert!(!event.contains("x-private-header"), "{event}");
        assert!(!event.contains("/mcp"), "{event}");
    }
}
