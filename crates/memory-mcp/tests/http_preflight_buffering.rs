#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::task::{Context, Poll, Waker};

use axum::body::Body;
use axum::extract::State;
use axum::http::{Method, Request, Response, StatusCode};
use axum::response::IntoResponse;
use axum::routing::post;
use bytes::Bytes;
use http_body::{Body as HttpBody, Frame, SizeHint};
use memory_mcp::http::{
    HttpState, config::HttpConfig, middleware::prevalidate_mcp, router::build_router,
    test_state::HttpStateTestBuilder,
};
use tokio::sync::Notify;
use tower_service::Service;

const MCP_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"preflight-test","version":"0.0.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;
const SUBSCRIPTION_BODY: &str = r#"{"jsonrpc":"2.0","id":2,"method":"subscriptions/listen","params":{"_meta":{"io.modelcontextprotocol/protocolVersion":"2026-07-28","io.modelcontextprotocol/clientInfo":{"name":"preflight-test","version":"0.0.0"},"io.modelcontextprotocol/clientCapabilities":{}}}}"#;

struct BodyGate {
    started: Notify,
    released: AtomicBool,
    polls: AtomicUsize,
    waker: Mutex<Option<Waker>>,
    bytes: Bytes,
}

impl BodyGate {
    fn blocked(bytes: impl Into<Bytes>) -> Arc<Self> {
        Arc::new(Self {
            started: Notify::new(),
            released: AtomicBool::new(false),
            polls: AtomicUsize::new(0),
            waker: Mutex::new(None),
            bytes: bytes.into(),
        })
    }

    fn released(bytes: impl Into<Bytes>) -> Arc<Self> {
        Arc::new(Self {
            started: Notify::new(),
            released: AtomicBool::new(true),
            polls: AtomicUsize::new(0),
            waker: Mutex::new(None),
            bytes: bytes.into(),
        })
    }

    fn release(&self) {
        self.released.store(true, Ordering::Release);
        if let Some(waker) = self.waker.lock().expect("waker lock").take() {
            waker.wake();
        }
    }
}

struct GatedBody {
    gate: Arc<BodyGate>,
    sent: bool,
}

impl HttpBody for GatedBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let this = self.get_mut();
        if this.sent {
            return Poll::Ready(None);
        }

        this.gate.polls.fetch_add(1, Ordering::Relaxed);
        this.gate.started.notify_one();
        if !this.gate.released.load(Ordering::Acquire) {
            *this.gate.waker.lock().expect("waker lock") = Some(cx.waker().clone());
            if this.gate.released.load(Ordering::Acquire) {
                cx.waker().wake_by_ref();
            }
            return Poll::Pending;
        }

        this.sent = true;
        Poll::Ready(Some(Ok(Frame::data(this.gate.bytes.clone()))))
    }

    fn is_end_stream(&self) -> bool {
        self.sent
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn gated_body(gate: Arc<BodyGate>) -> Body {
    Body::new(GatedBody { gate, sent: false })
}

struct FailingBody {
    emitted: bool,
}

impl HttpBody for FailingBody {
    type Data = Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        if self.emitted {
            Poll::Ready(None)
        } else {
            self.emitted = true;
            Poll::Ready(Some(Err(std::io::Error::other("synthetic body error"))))
        }
    }

    fn is_end_stream(&self) -> bool {
        false
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

fn mcp_post(body: Body) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/list")
        .body(body)
        .expect("valid test request")
}

fn subscription_post() -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "subscriptions/listen")
        .body(Body::from(SUBSCRIPTION_BODY))
        .expect("valid subscription request")
}

fn padded_mcp_body(length: usize) -> Bytes {
    assert!(length >= MCP_BODY.len());
    let mut body = MCP_BODY.as_bytes().to_vec();
    body.resize(length, b' ');
    Bytes::from(body)
}

async fn test_state_with_slots(preflight_request_limit: usize) -> Arc<HttpState> {
    let mut config = HttpConfig::default_for_test();
    config.preflight_request_limit = preflight_request_limit;
    config.body_limit_bytes = 1024;
    config.preflight_bytes = 1024;
    HttpStateTestBuilder::new()
        .await
        .with_config(config)
        .build()
        .await
        .expect("test HTTP state")
}

async fn test_state() -> Arc<HttpState> {
    test_state_with_slots(1).await
}

async fn test_router_with_slots(preflight_request_limit: usize) -> axum::Router {
    build_router(test_state_with_slots(preflight_request_limit).await, None).expect("HTTP router")
}

async fn test_router() -> axum::Router {
    build_router(test_state().await, None).expect("HTTP router")
}

struct DownstreamGate {
    entered: Notify,
    release: Notify,
}

async fn wait_for_downstream_release(State(gate): State<Arc<DownstreamGate>>) -> StatusCode {
    gate.entered.notify_one();
    gate.release.notified().await;
    StatusCode::OK
}

async fn return_open_response(State(gate): State<Arc<BodyGate>>) -> Response<Body> {
    (StatusCode::OK, gated_body(gate)).into_response()
}

#[tokio::test]
async fn saturated_preflight_refuses_without_polling_the_denied_body() {
    let router = test_router().await;
    let first_gate = BodyGate::blocked(Bytes::from_static(MCP_BODY.as_bytes()));
    let second_gate = BodyGate::released(Bytes::from_static(MCP_BODY.as_bytes()));

    let first_gate_for_body = Arc::clone(&first_gate);
    let mut first_router = router.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(mcp_post(gated_body(first_gate_for_body)))
            .await
            .expect("first request")
    });
    first_gate.started.notified().await;

    let mut second_router = router.clone();
    let second_response: Response<Body> = second_router
        .call(mcp_post(gated_body(Arc::clone(&second_gate))))
        .await
        .expect("second request");
    assert_eq!(second_response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(
        second_gate.polls.load(Ordering::Relaxed),
        0,
        "the denied request body must not be polled"
    );

    first_gate.release();
    let first_response = first.await.expect("first request task");
    assert_eq!(first_response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn declared_content_length_budget_refusal_does_not_poll_body() {
    let router = test_router_with_slots(2).await;
    let first_gate = BodyGate::blocked(Bytes::from_static(MCP_BODY.as_bytes()));
    let first_gate_for_body = Arc::clone(&first_gate);
    let mut first_request = mcp_post(gated_body(first_gate_for_body));
    first_request
        .headers_mut()
        .insert("content-length", "800".parse().expect("content length"));
    let mut first_router = router.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(first_request)
            .await
            .expect("first request")
    });
    first_gate.started.notified().await;

    let second_gate = BodyGate::released(Bytes::from_static(MCP_BODY.as_bytes()));
    let mut second_request = mcp_post(gated_body(Arc::clone(&second_gate)));
    second_request
        .headers_mut()
        .insert("content-length", "300".parse().expect("content length"));
    let mut second_router = router.clone();
    let second = second_router
        .call(second_request)
        .await
        .expect("byte-budget-refused request");
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(second_gate.polls.load(Ordering::Relaxed), 0);

    first_gate.release();
    assert_eq!(
        first.await.expect("first request task").status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn understated_content_length_is_charged_before_body_copy() {
    let router = test_router_with_slots(2).await;
    let first_gate = BodyGate::blocked(Bytes::from_static(MCP_BODY.as_bytes()));
    let first_gate_for_body = Arc::clone(&first_gate);
    let mut first_request = mcp_post(gated_body(first_gate_for_body));
    first_request
        .headers_mut()
        .insert("content-length", "800".parse().expect("content length"));
    let mut first_router = router.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(first_request)
            .await
            .expect("first request")
    });
    first_gate.started.notified().await;

    let second_gate = BodyGate::released(padded_mcp_body(300));
    let mut second_request = mcp_post(gated_body(Arc::clone(&second_gate)));
    second_request
        .headers_mut()
        .insert("content-length", "1".parse().expect("understated length"));
    let mut second_router = router.clone();
    let second = second_router
        .call(second_request)
        .await
        .expect("understated-length refusal");
    assert_eq!(second.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(second_gate.polls.load(Ordering::Relaxed), 1);

    first_gate.release();
    assert_eq!(
        first.await.expect("first request task").status(),
        StatusCode::UNAUTHORIZED
    );

    let mut retry_router = router;
    let retry = retry_router
        .call(mcp_post(Body::from(MCP_BODY)))
        .await
        .expect("retry after byte reservations release");
    assert_eq!(retry.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cancellation_during_body_collection_releases_preflight_capacity() {
    let router = test_router().await;
    let first_gate = BodyGate::blocked(Bytes::from_static(MCP_BODY.as_bytes()));
    let first_gate_for_body = Arc::clone(&first_gate);
    let mut first_router = router.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(mcp_post(gated_body(first_gate_for_body)))
            .await
            .expect("cancelled request future")
    });
    first_gate.started.notified().await;

    first.abort();
    assert!(
        first
            .await
            .expect_err("request was cancelled")
            .is_cancelled()
    );

    let retry_gate = BodyGate::released(Bytes::from_static(MCP_BODY.as_bytes()));
    let mut retry_router = router;
    let retry_response = retry_router
        .call(mcp_post(gated_body(Arc::clone(&retry_gate))))
        .await
        .expect("retry request");
    assert_eq!(retry_response.status(), StatusCode::UNAUTHORIZED);
    assert!(retry_gate.polls.load(Ordering::Relaxed) > 0);
}

#[tokio::test]
async fn malformed_json_releases_preflight_capacity() {
    let mut router = test_router().await;
    let response = router
        .call(mcp_post(Body::from("{")))
        .await
        .expect("malformed request");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);

    let retry = router
        .call(mcp_post(Body::from(MCP_BODY)))
        .await
        .expect("retry request");
    assert_eq!(retry.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn oversized_advertised_length_is_rejected_before_body_polling() {
    let mut router = test_router().await;
    let body_gate = BodyGate::released(Bytes::from_static(MCP_BODY.as_bytes()));
    let mut request = mcp_post(gated_body(Arc::clone(&body_gate)));
    request
        .headers_mut()
        .insert("content-length", "1025".parse().expect("content length"));

    let response = router.call(request).await.expect("oversized request");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body_gate.polls.load(Ordering::Relaxed), 0);
}

async fn assert_header_refusal_during_saturation(
    name: &'static str,
    value: &'static str,
    expected: StatusCode,
) {
    let router = test_router().await;
    let first_gate = BodyGate::blocked(Bytes::from_static(MCP_BODY.as_bytes()));
    let first_gate_for_body = Arc::clone(&first_gate);
    let mut first_router = router.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(mcp_post(gated_body(first_gate_for_body)))
            .await
            .expect("first request")
    });
    first_gate.started.notified().await;

    let body_gate = BodyGate::released(Bytes::from_static(MCP_BODY.as_bytes()));
    let mut request = mcp_post(gated_body(Arc::clone(&body_gate)));
    request
        .headers_mut()
        .insert(name, value.parse().expect("test header"));
    let mut request_router = router;
    let response = request_router.call(request).await.expect("header refusal");
    assert_eq!(response.status(), expected);
    assert_eq!(body_gate.polls.load(Ordering::Relaxed), 0);

    first_gate.release();
    assert_eq!(
        first.await.expect("first request task").status(),
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn unsupported_content_type_precedes_saturated_preflight() {
    assert_header_refusal_during_saturation(
        "content-type",
        "text/plain",
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
    )
    .await;
}

#[tokio::test]
async fn unacceptable_response_media_types_precede_saturated_preflight() {
    assert_header_refusal_during_saturation(
        "accept",
        "application/json",
        StatusCode::NOT_ACCEPTABLE,
    )
    .await;
}

#[tokio::test]
async fn oversized_content_length_precedes_saturated_preflight() {
    assert_header_refusal_during_saturation(
        "content-length",
        "1025",
        StatusCode::PAYLOAD_TOO_LARGE,
    )
    .await;
}

#[tokio::test]
async fn chunked_body_at_limit_is_accepted_for_protocol_validation() {
    let mut router = test_router().await;
    let request = mcp_post(Body::from(padded_mcp_body(1024)));
    assert!(request.headers().get("content-length").is_none());

    let response = router.call(request).await.expect("exact-limit request");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn chunked_body_over_limit_is_rejected_and_releases_capacity() {
    let mut router = test_router().await;
    let request = mcp_post(Body::from(padded_mcp_body(1025)));
    assert!(request.headers().get("content-length").is_none());

    let response = router.call(request).await.expect("over-limit request");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let retry = router
        .call(mcp_post(Body::from(MCP_BODY)))
        .await
        .expect("retry request");
    assert_eq!(retry.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn body_stream_error_is_reported_and_releases_preflight_capacity() {
    let mut router = test_router().await;
    let response = router
        .call(mcp_post(Body::new(FailingBody { emitted: false })))
        .await
        .expect("request middleware response");
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);

    let retry = router
        .call(mcp_post(Body::from(MCP_BODY)))
        .await
        .expect("retry request");
    assert_eq!(retry.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn cancellation_while_downstream_is_pending_releases_preflight_capacity() {
    let gate = Arc::new(DownstreamGate {
        entered: Notify::new(),
        release: Notify::new(),
    });
    let app = axum::Router::new()
        .route("/mcp", post(wait_for_downstream_release))
        .with_state(Arc::clone(&gate))
        .layer(axum::middleware::from_fn_with_state(
            test_state().await,
            prevalidate_mcp,
        ));

    let mut first_router = app.clone();
    let first = tokio::spawn(async move {
        first_router
            .call(mcp_post(Body::from(MCP_BODY)))
            .await
            .expect("downstream request")
    });
    gate.entered.notified().await;
    first.abort();
    assert!(
        first
            .await
            .expect_err("downstream request was cancelled")
            .is_cancelled()
    );

    gate.release.notify_one();
    let mut retry_router = app;
    let retry = retry_router
        .call(mcp_post(Body::from(MCP_BODY)))
        .await
        .expect("retry request");
    assert_eq!(retry.status(), StatusCode::OK);
}

#[tokio::test]
async fn subscription_response_releases_capacity_before_stream_completion() {
    let response_gate = BodyGate::blocked(Bytes::from_static(b"event"));
    let app = axum::Router::new()
        .route("/mcp", post(return_open_response))
        .with_state(Arc::clone(&response_gate))
        .layer(axum::middleware::from_fn_with_state(
            test_state().await,
            prevalidate_mcp,
        ));

    let mut first_router = app.clone();
    let first_response = first_router
        .call(subscription_post())
        .await
        .expect("subscription response");
    assert_eq!(first_response.status(), StatusCode::OK);
    assert_eq!(response_gate.polls.load(Ordering::Relaxed), 0);

    let mut second_router = app;
    let second_response = second_router
        .call(subscription_post())
        .await
        .expect("second subscription response");
    assert_eq!(second_response.status(), StatusCode::OK);
    assert_eq!(response_gate.polls.load(Ordering::Relaxed), 0);
}
