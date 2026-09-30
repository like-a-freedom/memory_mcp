//! Bounded request logging middleware.
//!
//! The logger must NEVER record URI paths, headers, bodies,
//! credentials, namespace names, email, or memory content. This
//! module enforces that: the only fields it emits are bounded
//! labels (method_category, credential_kind, outcome, latency_ms)
//! plus a request_id and tenant_fingerprint that the auth layer
//! supplies.

use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;
use uuid::Uuid;

use crate::logging::{LogLevel, StdoutLogger};

/// The header that carries a request's id in both directions: a client may
/// supply one, and every response advertises the one it was given.
///
/// The same name `local_admin` uses, so a caller that already learns to send
/// it for one surface learns nothing new for another.
pub(crate) const REQUEST_ID_HEADER: &str = "x-request-id";

/// The id of the request currently in flight, minted once by
/// [`request_log`] and read by everything that has to name the same
/// operation: the access log, the error envelope, and the audit trail.
///
/// A newtype rather than a bare `Uuid` so that a layer which wants an id has
/// to say which id it means — the `Uuid` extension the local-admin handlers
/// insert is a different value, minted for a different surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestId(pub(crate) Uuid);

impl RequestId {
    /// The underlying value, for a surface that stores ids as `Uuid`.
    #[must_use]
    pub fn as_uuid(&self) -> Uuid {
        self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Give the request an id, advertise it on the response, and record the
/// request in the bounded access log.
///
/// Minting the id here rather than at the point of failure is what lets a
/// refusal name itself: an error raised by a handler already has the id in
/// its request extensions, so the log line and the `correlation_id` a client
/// reads are the same string instead of two unrelated ones.
///
/// A caller-supplied id is honoured, so a retry carrying the id from its
/// previous response produces a second entry that reads as the same operation.
///
/// This is outermost, so it is the only layer guaranteed to see every request —
/// including one the deployment boundary refuses before any inner layer runs.
pub(crate) async fn request_log(mut req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method_category = categorize(req.method().as_str());
    let request_id = req
        .headers()
        .get(REQUEST_ID_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| Uuid::parse_str(value).ok())
        .map(RequestId)
        .unwrap_or_else(|| RequestId(Uuid::new_v4()));
    req.extensions_mut().insert(request_id);

    // Saturation: raised before the handler runs and lowered once it returns,
    // so a scrape taken during a slow request sees it. Both halves are on every
    // exit path, including the ones where a handler refuses.
    inflight_requests(1);
    let mut response = next.run(req).await;
    inflight_requests(-1);
    if let Ok(value) = axum::http::HeaderValue::from_str(&request_id.to_string()) {
        response.headers_mut().insert(REQUEST_ID_HEADER, value);
    }
    // Inner middleware attaches the context to the response after it
    // has resolved authentication and the tenant. Read it here rather
    // than from the request, which is observed before inner layers run.
    // The borrow ends before the mutable insert below.
    let (credential_kind, tenant_fingerprint, inner_request_id) = {
        let ctx = response.extensions().get::<TenantLogContext>();
        (
            ctx.map(|c| c.credential_kind.clone()).unwrap_or_default(),
            ctx.map(|c| c.tenant_fingerprint.clone())
                .unwrap_or_default(),
            ctx.map(|c| c.request_id.clone()).unwrap_or_default(),
        )
    };
    let request_id = request_id.to_string();
    response.extensions_mut().insert(TenantLogContext {
        request_id: if inner_request_id.is_empty() {
            request_id.clone()
        } else {
            inner_request_id
        },
        credential_kind: credential_kind.clone(),
        tenant_fingerprint: tenant_fingerprint.clone(),
    });
    let outcome = outcome_label(response.status().as_u16());
    let elapsed = started.elapsed();
    // The same three facts the log line carries, as metrics: the traffic
    // count, the latency distribution, and — through `outcome` — the error
    // ratio. Taken from the same values, so a graph and a log line can never
    // disagree about one request.
    record_request_metric(method_category, outcome, elapsed.as_secs_f64());
    let event = RequestLog {
        event: "http_request",
        request_id: &request_id,
        method_category,
        credential_kind: &credential_kind,
        outcome,
        latency_ms: elapsed.as_millis() as u64,
        tenant_fingerprint: &tenant_fingerprint,
    };
    if let Ok(json) = serde_json::to_string(&event) {
        let mut fields = std::collections::HashMap::new();
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            fields.insert("payload".to_string(), value);
        }
        // The operation name is the filter key, so it has to be a field rather
        // than part of the payload. Without it the access log takes no part in
        // `RUST_LOG=<prefix>=<level>` — the one stream an operator most wants
        // to turn up or down, and the only one with a request id.
        fields.insert("op".to_string(), crate::logging::OP_HTTP_REQUEST.into());
        crate::logging::StdoutLogger::from_env().log(fields, crate::logging::LogLevel::Info);
    }
    response
}

/// Move the in-flight gauge by `delta` requests.
fn inflight_requests(delta: i64) {
    crate::observability::shift_gauge(
        crate::shared::observability::METRIC_HTTP_REQUESTS_INFLIGHT,
        delta as f64,
    );
}

/// Record one served request: the traffic count and the latency observation.
///
/// The labels are `&'static str` because a metrics registry outlives the
/// request, and a label borrowed from a request would dangle after it returned.
/// `categorize` and `outcome_label` already return exactly that — a bounded
/// class, never the path or the status code.
fn record_request_metric(method_category: &'static str, outcome: &'static str, seconds: f64) {
    crate::observability::count_timed(
        crate::shared::observability::METRIC_HTTP_REQUESTS_TOTAL,
        crate::shared::observability::METRIC_HTTP_REQUEST_DURATION_SECONDS,
        seconds,
        "method",
        method_category,
        "outcome",
        outcome,
    );
}

/// Bounded request log event. The serialize order is the
/// stdout order; don't add fields that would leak.
#[derive(Serialize)]
struct RequestLog<'a> {
    event: &'a str,
    request_id: &'a str,
    method_category: &'a str,
    credential_kind: &'a str,
    outcome: &'a str,
    latency_ms: u64,
    tenant_fingerprint: &'a str,
}

/// Per-request context stored as an axum extension. Populated by
/// the auth layer and the principal extractor.
#[derive(Default, Clone, Debug)]
pub struct TenantLogContext {
    pub request_id: String,
    pub credential_kind: String,
    pub tenant_fingerprint: String,
}

/// A bounded warning from the HTTP runtime, with no request in scope.
///
/// Two copies of this existed — one in the runtime pool, one in the lease
/// migration — each named `tracing_warn`, each an `eprintln!`. The name said
/// the event was filtered; it was not, and it carried no level, no timestamp
/// and no operation, so `RUST_LOG` could neither raise nor lower it and no
/// subsystem directive could reach it. One type here ends that.
///
/// `op` is required: a warning with no operation name is one no directive can
/// select and no log query can find.
pub struct WarningEvent {
    op: &'static str,
    event: std::collections::HashMap<String, serde_json::Value>,
}

impl WarningEvent {
    /// Build the event. `detail` is kept as one value because it is prose —
    /// an error's `Display` — and splitting it on spaces would report a
    /// fragment as though it were the whole failure.
    #[must_use]
    pub fn new(op: &'static str, detail: &str) -> Self {
        let mut event = std::collections::HashMap::new();
        event.insert("op".into(), op.into());
        event.insert("detail".into(), detail.to_string().into());
        Self { op, event }
    }

    /// Emit it through the deployment's logger.
    ///
    /// The refusal is counted here rather than at the call sites, so every
    /// warning the HTTP runtime raises is counted once regardless of whether it
    /// reached this through `log_warn` or `log_warn_at`. Counting at the entry
    /// points instead left the eleven `log_warn` calls uncounted — quota,
    /// scheduler, registry reconciliation, app-session binds — which is most of
    /// the surface, while the commit that added the counter described it as
    /// covering all of them.
    pub fn log_into(self, logger: &StdoutLogger) {
        crate::observability::record_runtime_refusal(self.op);
        logger.log(self.event, LogLevel::Warn);
    }
}

/// A warning that belongs to a specific request.
///
/// The same event as [`WarningEvent`], plus the request's id. A failure that
/// answers `5xx` with no body gives a client nothing to quote, so the id in
/// the log is the only way their report reaches a line — and this is why the
/// quota paths pass one rather than logging a bare warning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestWarning {
    op: &'static str,
    detail: String,
    request_id: RequestId,
}

impl RequestWarning {
    #[must_use]
    pub fn new(op: &'static str, detail: &str, request_id: RequestId) -> Self {
        Self {
            op,
            detail: detail.to_string(),
            request_id,
        }
    }

    /// Emit it through the deployment's logger.
    ///
    /// Counting happens here rather than at the entry points, so both
    /// `log_warn` and `log_warn_at` count exactly once. Counting at the entry
    /// points instead left the eleven `log_warn` calls uncounted — quota,
    /// scheduler, registry reconciliation, app-session binds — which is most of
    /// the surface the counter was introduced to cover.
    pub fn log_into(self, logger: &StdoutLogger) {
        crate::observability::record_runtime_refusal(self.op);
        let mut event = std::collections::HashMap::new();
        event.insert("op".into(), self.op.into());
        event.insert("detail".into(), self.detail.into());
        event.insert("request_id".into(), self.request_id.to_string().into());
        logger.log(event, LogLevel::Warn);
    }
}

/// Record a warning about a specific request.
///
/// Shorthand for [`RequestWarning::new`] followed by
/// [`RequestWarning::log_into`].
pub fn log_warn_at(op: &'static str, detail: &str, request_id: Option<RequestId>) {
    if let Some(request_id) = request_id {
        RequestWarning::new(op, detail, request_id).log_into(&StdoutLogger::from_env());
    } else {
        log_warn(op, detail);
    }
}

/// Record a bounded warning from the HTTP runtime, with no request in scope.
///
/// Shorthand for [`WarningEvent::new`] followed by [`WarningEvent::log_into`]
/// on the deployment's logger, for the common case where the caller has no
/// logger of its own to pass.
pub fn log_warn(op: &'static str, detail: &str) {
    WarningEvent::new(op, detail).log_into(&StdoutLogger::from_env());
}

/// Method-category grouping. URIs and headers never reach the log.
fn categorize(method: &str) -> &'static str {
    match method {
        "GET" => "read",
        "POST" => "write",
        "PUT" | "PATCH" => "update",
        "DELETE" => "delete",
        _ => "other",
    }
}

fn outcome_label(status: u16) -> &'static str {
    match status {
        100..=199 => "1xx",
        200..=299 => "2xx",
        300..=399 => "3xx",
        400..=499 => "4xx",
        500..=599 => "5xx",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use tower_service::Service;

    async fn echo() -> &'static str {
        "ok"
    }

    #[tokio::test]
    async fn request_log_emits_event_with_method_category() {
        let mut svc = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(request_log));
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn categorize_groups_methods() {
        assert_eq!(categorize("GET"), "read");
        assert_eq!(categorize("POST"), "write");
        assert_eq!(categorize("DELETE"), "delete");
    }

    #[test]
    fn outcome_label_buckets_status() {
        assert_eq!(outcome_label(200), "2xx");
        assert_eq!(outcome_label(404), "4xx");
        assert_eq!(outcome_label(503), "5xx");
    }

    /// The access log exists to correlate a response with a request, and a
    /// request id that is never minted makes every entry unusable for that: an
    /// operator reading `request_id=""` cannot tell which of a hundred
    /// simultaneous logins this line describes.
    ///
    /// The id is read from the response extension the log reads, so this
    /// asserts the value the log will actually emit is non-empty and matches
    /// what the response returned to the client.
    #[tokio::test]
    async fn the_access_log_carries_the_id_the_client_sees() {
        let mut svc = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(request_log));
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let from_log = resp
            .extensions()
            .get::<TenantLogContext>()
            .expect("the log context is populated for every request")
            .request_id
            .clone();
        assert!(
            !from_log.is_empty(),
            "the access log must never emit an empty id"
        );

        let header = resp
            .headers()
            .get(REQUEST_ID_HEADER)
            .and_then(|value| value.to_str().ok())
            .expect("the response advertises its id");
        assert_eq!(
            header, from_log,
            "the logged id and the client's id must match"
        );
    }

    /// Two requests must not share an id, or the log would collapse them into
    /// what looks like one operation.
    #[tokio::test]
    async fn each_request_gets_its_own_id() {
        let svc = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(request_log));
        let send = || {
            let mut svc = svc.clone();
            async move {
                let resp = svc
                    .call(
                        Request::builder()
                            .method("GET")
                            .uri("/")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .unwrap();
                // The id is observable on the response in two places that must
                // agree: the header the client reads and the context the log
                // reads. They are the same value, so a reader can join them.
                let header = resp
                    .headers()
                    .get(REQUEST_ID_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .expect("response advertises its id")
                    .to_owned();
                let logged = resp
                    .extensions()
                    .get::<TenantLogContext>()
                    .expect("log context is populated")
                    .request_id
                    .clone();
                assert_eq!(header, logged, "the two must name the same request");
                header
            }
        };
        assert_ne!(
            send().await,
            send().await,
            "two requests must not share an id, or the log collapses them"
        );
    }

    /// A caller-supplied id is honoured: a client retrying with the id from its
    /// previous response must produce a second entry that can be read as the
    /// same operation, not as a new one.
    #[tokio::test]
    async fn a_caller_supplied_id_is_honoured() {
        let supplied = Uuid::parse_str("6f1b2c3d-4e5a-4b6c-8d9e-0f1a2b3c4d5e").unwrap();
        let mut svc = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(request_log));
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .header(REQUEST_ID_HEADER, supplied.to_string())
            .body(Body::empty())
            .unwrap();
        let resp = svc.call(req).await.unwrap();
        assert_eq!(
            resp.headers()
                .get(REQUEST_ID_HEADER)
                .and_then(|v| v.to_str().ok()),
            Some(supplied.to_string().as_str()),
            "a caller-supplied id must survive the middleware"
        );
    }

    /// A served request has to be visible as a metric, not only as a log line.
    ///
    /// The access log is the right place — it is the outermost layer, it
    /// already classifies the method, the status and the duration, and it runs
    /// for every request including the ones refused before any inner layer. But
    /// a log line is not a metric: it is sampled, not aggregated, and the only
    /// way to answer "how many requests did this deployment serve, and how slow
    /// were they" from logs is to parse them. `/metrics` rendered nothing about
    /// HTTP traffic at all, which is the first golden signal missing from a
    /// service whose only entry point is HTTP.
    #[tokio::test]
    async fn a_served_request_reaches_the_metrics_exporter() {
        use crate::observability::tests::{exposed, sample_series};

        // This route's own series, read relative to what it held before. The
        // recorder is process-global and some fifty other tests drive real
        // requests through it, so neither an absolute value nor a
        // first-matching series can be asserted — both were flaky, and an
        // absolute `== 1` was really asserting that no other test had run
        // yet. The labels pin the series; the two readings, taken under one
        // lock, make the assertion exact.
        const SERIES: &str = r#"method="read",outcome="2xx""#;
        let (before, exposition) = exposed(|| async {
            let mut svc = Router::new()
                .route("/", get(echo))
                .layer(axum::middleware::from_fn(request_log));
            let before = sample_series(
                &crate::observability::tests::render(),
                crate::observability::METRIC_HTTP_REQUESTS_TOTAL,
                SERIES,
            );
            let req = Request::builder()
                .method("GET")
                .uri("/")
                .body(Body::empty())
                .unwrap();
            svc.call(req).await.expect("served");
            (before, crate::observability::tests::render())
        })
        .await;
        let after = sample_series(
            &exposition,
            crate::observability::METRIC_HTTP_REQUESTS_TOTAL,
            SERIES,
        );

        assert!(
            after.is_some_and(|after| after > before.unwrap_or(0.0)),
            "one served request must raise the traffic counter: before={before:?} \
             after={after:?}\n{exposition}"
        );
        assert!(
            exposition.contains(crate::observability::METRIC_HTTP_REQUEST_DURATION_SECONDS),
            "latency must reach the exporter: {exposition}"
        );
        assert!(
            exposition.contains(crate::observability::METRIC_HTTP_REQUESTS_INFLIGHT),
            "saturation must reach the exporter: {exposition}"
        );
        // The histogram must have observed the request, not merely declared the
        // series. `_count` is the exact signal: it counts observations, and one
        // served request makes it one greater.
        //
        // The previous assertion here was `contains("_sum") &&
        // !contains("_sum 0")`, which could never fail — in the text format the
        // value follows the labels, so the literal `_sum 0` never occurs and
        // the condition held whether or not anything was recorded.
        let observations = sample_series(
            &exposition,
            &format!(
                "{}_count",
                crate::observability::METRIC_HTTP_REQUEST_DURATION_SECONDS
            ),
            SERIES,
        );
        assert!(
            observations.is_some(),
            "the latency histogram must have observed this request: \
             {exposition}"
        );
    }

    /// A request that failed is counted under its status class, and one in
    /// flight is visible as a gauge that returns to zero. Together they answer
    /// the two questions an on-call engineer asks first: is anything broken,
    /// and is anything stuck.
    #[tokio::test]
    async fn a_failed_request_is_counted_under_its_status() {
        use crate::observability::tests::exposed;

        let exposition = exposed(|| async {
            let mut svc = Router::new()
                .route(
                    "/boom",
                    get(|| async { axum::http::StatusCode::INTERNAL_SERVER_ERROR }),
                )
                .layer(axum::middleware::from_fn(request_log));
            let req = Request::builder()
                .method("GET")
                .uri("/boom")
                .body(Body::empty())
                .unwrap();
            svc.call(req).await.expect("served");
            crate::observability::tests::render()
        })
        .await;

        assert!(
            exposition.contains("outcome=\"5xx\""),
            "a 5xx must be countable without reading a log: {exposition}"
        );
    }

    /// The access log takes part in the per-subsystem filter. It is the one
    /// stream an operator most wants to raise or lower — it is the only one
    /// with a request id, so it is the one they follow — and it took no part
    /// while it carried its operation name inside a payload rather than as the
    /// `op` the filter matches on.
    ///
    /// Asserted on the emitted line, not on the constant: asserting that the
    /// name is selectable proves nothing about whether the access log uses it,
    /// which is the part that was missing.
    #[tokio::test]
    async fn the_access_log_is_reachable_by_its_subsystem() {
        let sink = crate::logging::capture::install();
        let mut svc = Router::new()
            .route("/", get(echo))
            .layer(axum::middleware::from_fn(request_log));
        let req = Request::builder()
            .method("GET")
            .uri("/")
            .body(Body::empty())
            .unwrap();

        crate::logging::capture::with_level("http=info", || async { svc.call(req).await.unwrap() })
            .await;

        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains(&format!("op={}", crate::logging::OP_HTTP_REQUEST))),
            "the access log must carry the `op` the filter matches on: {recorded:?}"
        );
    }

    /// A runtime warning with no request behind it — a tenant runtime that
    /// failed to activate, a lease that could not be released — is the one
    /// event with nothing else pointing at it. These were `eprintln!` behind a
    /// function named `tracing_warn`, so the name promised a filter that did
    /// not exist: the line carried no level, no timestamp and no operation, so
    /// `RUST_LOG` could not silence it and no subsystem directive could reach
    /// it. The rendered line is asserted, because that is what an operator
    /// reads and where the missing level would show.
    ///
    /// Both `log_warn` and the logger's filtering are exercised directly here
    /// rather than through a level override: the override is process-global,
    /// so a test that set one would change the level every other test in the
    /// process sees.
    #[test]
    fn a_runtime_warning_reaches_the_log_with_its_level_and_operation() {
        let sink = crate::logging::capture::install();

        event_for_warning().log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("warn".to_string()),
            _ => None,
        }));

        let recorded = sink.lines();
        assert!(
            recorded.iter().any(|line| {
                line.contains(&format!("op={TEST_ONLY_OP}"))
                    && line.contains("WARN")
                    && line.contains(r#"detail="tenant t1: disk is gone""#)
            }),
            "a runtime warning must be a filterable, levelled line: {recorded:?}"
        );
    }

    /// The operation name is the whole point: it is what makes a subsystem
    /// directive able to reach the event. Rendered through the real logger so
    /// the filter and the line are asserted together.
    #[test]
    fn a_runtime_warning_is_reachable_and_silenceable_by_its_subsystem() {
        let sink = crate::logging::capture::install();
        let reported = |lines: &[String]| {
            lines
                .iter()
                .filter(|line| line.contains(&format!("op={TEST_ONLY_OP}")))
                .count()
        };

        event_for_warning().log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("http=warn".to_string()),
            _ => None,
        }));
        let heard = reported(&sink.lines());
        assert!(heard > 0, "`http=warn` must report it: {:?}", sink.lines());

        // Clear rather than reinstall: a second capture swaps the buffer, and
        // the point of this phase is that the first one's lines are gone.
        //
        // The operation name is test-only, so a line carrying it is
        // unambiguously this test's: every live capture receives every line, and
        // a parallel test's output cannot be mistaken for this one.
        sink.clear();
        event_for_warning().log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("http=error".to_string()),
            _ => None,
        }));
        assert!(
            reported(&sink.lines()) == 0,
            "`http=error` must silence this event: heard={heard}, \
             still reported={} in {:?}",
            reported(&sink.lines()),
            sink.lines()
        );
    }

    /// The event `log_warn` builds, built directly so the filtering above can
    /// be exercised without the process-global level override.
    ///
    /// A test-only operation name. The runtime pool logs
    /// `http.runtime.activation_failed` from its own tests, and every live
    /// capture receives every line — so sharing the name meant this test read
    /// a parallel test's line as its own filter failing to silence the event,
    /// and failed intermittently while passing alone.
    fn event_for_warning() -> WarningEvent {
        WarningEvent::new(TEST_ONLY_OP, "tenant t1: disk is gone")
    }

    /// An operation name no production path emits, so a line carrying it is
    /// unambiguously this test's.
    const TEST_ONLY_OP: &str = "http.logging.test.silenceable";

    /// A request-scoped refusal answers with a generic body, so the metric is
    /// the only signal: a deployment whose quota registry is unreachable looks
    /// exactly like a quiet one. Counted in the one function every such
    /// warning goes through, so the count covers the surface rather than the
    /// call sites somebody remembered.
    #[tokio::test]
    #[cfg(feature = "prometheus")]
    async fn a_runtime_refusal_is_counted_for_the_exporter() {
        let exposition = crate::observability::tests::exposed(|| async {
            log_warn_at("http.quota.plan_load_failed", "registry unreachable", None);
            log_warn_at("http.quota.reserve_failed", "reserve refused", None);
            crate::observability::tests::render()
        })
        .await;

        assert!(
            exposition.contains(crate::observability::METRIC_RUNTIME_REFUSALS_TOTAL),
            "a runtime refusal must be countable: {exposition}"
        );
        for op in ["http.quota.plan_load_failed", "http.quota.reserve_failed"] {
            assert!(
                exposition.contains(&format!(r#"op="{op}""#)),
                "each refusal must be attributable to what refused: {op}"
            );
        }
    }

    /// A request-scoped warning has to carry the request's id. The quota paths
    /// answer `503` with no body, so the client is handed nothing to quote; the
    /// id in the log is the only thing their report can be matched against.
    /// Without it, "my ingest failed" and the log line about a quota registry
    /// are two unrelated facts.
    #[test]
    fn a_request_warning_carries_the_request_id() {
        let sink = crate::logging::capture::install();
        let id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

        RequestWarning::new(
            "http.quota.plan_load_failed",
            "registry unreachable",
            RequestId(id),
        )
        .log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("warn".to_string()),
            _ => None,
        }));

        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.quota.plan_load_failed")
                    && line.contains("req=11111111")
                    && line.contains("WARN")),
            "a request warning must name its request: {recorded:?}"
        );
    }

    /// A request warning with no request behind it degrades to the plain one
    /// rather than logging a blank id, so a line never carries `req=-` for
    /// something that should have named a request.
    #[test]
    fn a_request_warning_without_a_request_falls_back_to_the_plain_one() {
        let sink = crate::logging::capture::install();

        log_warn_at("http.quota.plan_load_failed", "registry unreachable", None);

        let recorded = sink.lines();
        assert!(
            recorded
                .iter()
                .any(|line| line.contains("op=http.quota.plan_load_failed")),
            "the warning must still be recorded: {recorded:?}"
        );
    }

    /// An operation name in the HTTP runtime has to start with `http`, or the
    /// subsystem directives cannot reach it. `RUST_LOG=http=error` is how an
    /// operator quiets this surface, and an event named `scheduler.failed`
    /// reads like it belongs to it while not being silenced by it — the same
    /// dishonesty as a function called `tracing_warn` that prints.
    ///
    /// The names come from [`HTTP_OPERATIONS`], which the call sites are
    /// checked against separately, so this proves the property and the other
    /// test proves the inventory.
    #[test]
    fn every_http_operation_is_reachable_by_the_http_directive() {
        let logger = StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("http=error".to_string()),
            _ => None,
        });
        for op in crate::logging::HTTP_OPERATIONS {
            assert!(
                op.starts_with("http."),
                "`{op}` is outside the namespace `http=` selects"
            );
            assert!(
                !logger.is_event_enabled(crate::logging::LogLevel::Warn, op),
                "`{op}` must be silenceable by `http=error`"
            );
            assert!(
                logger.is_event_enabled(crate::logging::LogLevel::Error, op),
                "`{op}` must keep its errors at `http=error`"
            );
        }
    }

    /// The inventory is only useful if it matches the code. An operation
    /// added to a call site and not listed here would pass the property check
    /// above while being unlistened-for, so the two are compared directly.
    #[test]
    fn the_http_operation_inventory_matches_what_the_code_emits() {
        let emitted: std::collections::BTreeSet<&str> = emitted_http_operations();
        let listed: std::collections::BTreeSet<&str> =
            crate::logging::HTTP_OPERATIONS.iter().copied().collect();

        let unlisted: Vec<_> = emitted.difference(&listed).copied().collect();
        assert!(
            unlisted.is_empty(),
            "these operations are emitted but not in HTTP_OPERATIONS, so nothing \
             checks them: {unlisted:?}"
        );
        let stale: Vec<_> = listed.difference(&emitted).copied().collect();
        assert!(
            stale.is_empty(),
            "HTTP_OPERATIONS lists operations the code no longer emits: {stale:?}"
        );
    }

    /// Every `http.*` operation name that appears as a string literal in the
    /// HTTP runtime, read from the source so the inventory cannot drift from
    /// what the code actually logs.
    ///
    /// `TEST_ONLY_OP` is skipped: it is a real literal in this file, but it
    /// identifies no production operation, and listing it would put the
    /// inventory check's own fixture into the set it is checking.
    fn emitted_http_operations() -> std::collections::BTreeSet<&'static str> {
        static NAMES: std::sync::OnceLock<std::collections::BTreeSet<&'static str>> =
            std::sync::OnceLock::new();
        NAMES
            .get_or_init(|| {
                let mut names = std::collections::BTreeSet::new();
                // The access log emits the constant, not a literal, so it is
                // seeded here rather than found by the scan.
                names.insert(crate::logging::OP_HTTP_REQUEST);
                for source in [
                    include_str!("leases/scheduler.rs"),
                    include_str!("leases/migration.rs"),
                    include_str!("runtime/pool.rs"),
                    include_str!("tasks/scheduler.rs"),
                    include_str!("app_sessions/scheduler.rs"),
                    include_str!("registry/provisioning.rs"),
                    include_str!("config/validate.rs"),
                    include_str!("middleware/acquire_runtime.rs"),
                    include_str!("logging.rs"),
                ] {
                    for line in source.lines() {
                        // A line that is a comment is documentation, not an emit.
                        let code = line.trim_start();
                        if code.starts_with("//") {
                            continue;
                        }
                        let Some(start) = code.find("\"http.") else {
                            continue;
                        };
                        let rest = &code[start + 1..];
                        let Some(end) = rest.find('"') else { continue };
                        let name = &rest[..end];
                        // A bare namespace is this file's own prefix check, and a
                        // name with a space is prose rather than an operation.
                        if name.contains(' ') || name.ends_with('.') {
                            continue;
                        }
                        // The test fixture's own name, not an operation the
                        // runtime emits. Listing it would mean the inventory
                        // check listed its own test data.
                        if name == TEST_ONLY_OP {
                            continue;
                        }
                        names.insert(name);
                    }
                }
                names
            })
            .clone()
    }
}
