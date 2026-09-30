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

    let mut response = next.run(req).await;
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
    let event = RequestLog {
        event: "http_request",
        request_id: &request_id,
        method_category,
        credential_kind: &credential_kind,
        outcome,
        latency_ms: started.elapsed().as_millis() as u64,
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
        Self { event }
    }

    /// Emit it through the deployment's logger.
    pub fn log_into(self, logger: &StdoutLogger) {
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
    pub fn log_into(self, logger: &StdoutLogger) {
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
                line.contains("op=http.runtime.activation_failed")
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
        let raised = crate::logging::capture::install();
        event_for_warning().log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("http=warn".to_string()),
            _ => None,
        }));
        assert!(
            !raised.lines().is_empty(),
            "`http=warn` must report it: {:?}",
            raised.lines()
        );

        // Clear rather than reinstall: a second capture swaps the buffer, and
        // the point of this phase is that the first one's lines are gone.
        raised.clear();
        event_for_warning().log_into(&StdoutLogger::from_env_with(|key| match key {
            "RUST_LOG" => Some("http=error".to_string()),
            _ => None,
        }));
        assert!(
            raised.lines().is_empty(),
            "`http=error` must silence it: {:?}",
            raised.lines()
        );
    }

    /// The event `log_warn` builds, built directly so the filtering above can
    /// be exercised without the process-global level override.
    fn event_for_warning() -> WarningEvent {
        WarningEvent::new("http.runtime.activation_failed", "tenant t1: disk is gone")
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
}
