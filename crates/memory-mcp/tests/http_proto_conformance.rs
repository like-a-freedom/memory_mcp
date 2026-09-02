#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

//! Black-box protocol conformance for the HTTP SaaS profile.
//!
//! Spawns the `memory_mcp_http` binary on an ephemeral port, waits for
//! the `memory_mcp_http bound=<addr>` line, then drives a series of
//! checks against the live server. Tests are async because the
//! server's dispatch path is async.
//!
//! Run: cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_proto_conformance -- --nocapture

use serde_json::json;

mod common;

use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant};

/// Fixed bootstrap API key for the conformance suite. The
/// `name=key` form is `<account_name>=<api_key>`; the test
/// env var (5.8) is `MEMORY_MCP_HTTP_TEST_BOOTSTRAP`.
const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef";

fn conformance_tenant() -> TestTenant {
    TestTenant::new("conformance", BOOTSTRAP_KEY)
}

#[tokio::test]
async fn get_on_mcp_returns_405() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .get(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn delete_on_mcp_returns_405() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .delete(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn disallowed_host_returns_403() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn disallowed_origin_returns_403() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("origin", "https://evil.example")
        .header("content-type", "application/json")
        .body("{}")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 403);
}

#[tokio::test]
async fn health_live_returns_ok() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .get(format!("{}/health/live", fixture.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn health_ready_returns_json() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let resp = fixture
        .client()
        .get(format!("{}/health/ready", fixture.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "ready");
}

#[tokio::test]
async fn no_mcp_session_id_header_is_set() {
    // 2026-07-28 stateless profile must never set Mcp-Session-Id
    // because it removes protocol sessions entirely.
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert!(
        resp.headers().get("mcp-session-id").is_none(),
        "2026-07-28 stateless profile must never set Mcp-Session-Id"
    );
}

#[tokio::test]
async fn server_discover_advertises_only_2026_07_28() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "server/discover")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        content_type.contains("text/event-stream") || content_type.contains("application/json"),
        "unexpected discovery content type: {content_type}"
    );
    let text = resp.text().await.expect("discovery body");
    assert!(
        text.contains("2026-07-28"),
        "discovery omitted protocol version: {text}"
    );
}

#[tokio::test]
async fn removed_ping_method_is_not_available() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "ping")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    let text = resp.text().await.expect("body");
    assert!(
        text.contains("-32601") || text.to_ascii_lowercase().contains("method not found"),
        "ping must not be available in 2026-07-28: {text}"
    );
}

#[tokio::test]
async fn unsupported_legacy_version_returns_400() {
    // 2025-03-26 is a KNOWN version, so the header check alone
    // would pass it. The 400 below comes from
    // stateless_protocol_metadata_required = true: legacy
    // requests carry no per-request _meta protocol version, and
    // rmcp rejects them.
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2025-03-26")
        .header("Mcp-Method", "ping")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
}

#[tokio::test]
async fn body_over_limit_returns_413() {
    // Shrink the limit so the test does not push 8 MiB.
    let fixture = HttpServerFixture::spawn(
        HttpServerConfig::default()
            .with_tenant(conformance_tenant())
            .with_env("MEMORY_MCP_HTTP_BODY_LIMIT", "1024"),
    )
    .await;
    let big = "a".repeat(2048);
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(big)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 413);
}

#[tokio::test]
async fn missing_accept_returns_406() {
    // rmcp requires Accept to include both application/json and
    // text/event-stream on stateless POSTs. reqwest forces Accept:
    // */* unless overridden, so we set it explicitly to a value
    // that does NOT include both media types.
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "ping")
        .header("accept", "*/*")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 406);
}

#[tokio::test]
async fn header_body_mismatch_returns_header_mismatch_error() {
    // The plan asserts that sending Mcp-Method=ping with a body
    // whose JSON-RPC method is something else (e.g. tools/list) is
    // rejected as a header-mismatch error. rmcp returns 400 with
    // a jsonrpc error body in that case.
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "ping")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    let text = resp.text().await.expect("body");
    assert!(
        text.contains("method") || text.contains("mismatch") || text.contains("header"),
        "expected method/mismatch/header in body, got: {text}"
    );
}

#[tokio::test]
async fn missing_mcp_method_returns_400_before_authentication() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    assert!(resp.text().await.expect("body").contains("MCP method"));
}

#[tokio::test]
async fn missing_mcp_name_returns_400_before_authentication() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {},
            "_meta": common::http_server::modern_meta()
        },
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    assert!(resp.text().await.expect("body").contains("MCP name"));
}

#[tokio::test]
async fn mismatched_mcp_name_returns_400() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {},
            "_meta": common::http_server::modern_meta()
        },
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "resolve")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    assert!(resp.text().await.expect("body").contains("MCP name"));
}

#[tokio::test]
async fn missing_protocol_version_returns_400() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("Mcp-Method", "server/discover")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    assert!(
        resp.text()
            .await
            .expect("body")
            .contains("protocol version")
    );
}

#[tokio::test]
async fn notification_returns_202_with_empty_body() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/list")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 202);
    assert!(resp.bytes().await.expect("body").is_empty());
}

#[tokio::test]
async fn forged_subscription_header_cannot_bypass_preflight() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": common::http_server::modern_meta()},
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "subscriptions/listen")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 400);
    assert!(resp.text().await.expect("body").contains("HeaderMismatch"));
}

#[tokio::test]
async fn tools_call_requires_matching_mcp_name() {
    // Plan asserts that the Mcp-Method header must equal the body
    // method for tools/call. rmcp validates this.
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(conformance_tenant()))
            .await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {
                "content": "conformance marker",
                "source_type": "conformance",
                "source_id": "conformance-tools-call",
                "t_ref": "2026-08-27T00:00:00Z",
                "t_ingested": null,
                "policy_tags": []
            },
            "_meta": common::http_server::modern_meta()
        },
    });
    let resp = fixture
        .client()
        .post(format!("{}/mcp", fixture.base_url))
        .header("host", "localhost")
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", "2026-07-28")
        .header("Mcp-Method", "tools/call")
        .header("Mcp-Name", "ingest")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(body.to_string())
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
}
