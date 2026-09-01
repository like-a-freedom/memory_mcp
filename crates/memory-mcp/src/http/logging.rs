//! Bounded request logging middleware.
//!
//! The logger must NEVER record URI paths, headers, bodies,
//! credentials, namespace names, email, or memory content. This
//! module enforces that: the only fields it emits are bounded
//! labels (method_category, credential_kind, outcome, latency_ms)
//! plus a request_id and tenant_fingerprint that the auth layer
//! supplies.

use std::sync::OnceLock;
use std::time::Instant;

use axum::extract::Request;
use axum::middleware::Next;
use axum::response::Response;
use serde::Serialize;

use crate::logging::StdoutLogger;

static LOGGER: OnceLock<StdoutLogger> = OnceLock::new();

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

/// Always-available middleware. Records latency and outcome but
/// relies on the auth layer to populate `TenantLogContext`. If the
/// extension is missing, fields are empty strings — never path/header
/// values.
pub async fn request_log(req: Request, next: Next) -> Response {
    let started = Instant::now();
    let method_category = categorize(req.method().as_str());
    let response = next.run(req).await;
    // Inner middleware attaches the context to the response after it
    // has resolved authentication and the tenant. Read it here rather
    // than from the request, which is observed before inner layers run.
    let ctx = response.extensions().get::<TenantLogContext>();
    let request_id = ctx.map(|c| c.request_id.as_str()).unwrap_or("");
    let credential_kind = ctx.map(|c| c.credential_kind.as_str()).unwrap_or("");
    let tenant_fingerprint = ctx.map(|c| c.tenant_fingerprint.as_str()).unwrap_or("");
    let outcome = outcome_label(response.status().as_u16());
    let event = RequestLog {
        event: "http_request",
        request_id,
        method_category,
        credential_kind,
        outcome,
        latency_ms: started.elapsed().as_millis() as u64,
        tenant_fingerprint,
    };
    if let Ok(json) = serde_json::to_string(&event) {
        if let Some(logger) = LOGGER.get() {
            let mut fields = std::collections::HashMap::new();
            if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
                fields.insert("payload".to_string(), value);
            }
            logger.log(fields, crate::logging::LogLevel::Info);
        } else {
            eprintln!("{json}");
        }
    }
    response
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
}
