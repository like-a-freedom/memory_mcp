//! Pre-MCP request validation.
//!
//! Runs before any auth or admission decision. Validates the modern
//! MCP envelope (mirrored headers, JSON-RPC 2.0, version, method
//! classification) and attaches a [`ValidatedMcpRequest`] extension
//! that downstream middleware and the handler consume.

use axum::http::{StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;

use crate::http::HttpState;

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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode, header};
    use axum::routing::{get, post};
    use http_body_util::BodyExt;
    use serde_json::{Value, json};
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
            .method(axum::http::Method::POST)
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
