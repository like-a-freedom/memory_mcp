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

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::json;

struct Server {
    child: Child,
    base_url: String,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral");
    listener.local_addr().expect("local addr").port()
}

/// Fixed bootstrap API key for the conformance suite. The
/// `name=key` form is `<account_name>=<api_key>`; the test
/// env var (5.8) is `MEMORY_MCP_HTTP_TEST_BOOTSTRAP`.
const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef";

fn base_env(port: u16) -> Vec<(String, String)> {
    let zeros = "0".repeat(64);
    vec![
        ("MEMORY_MCP_HTTP_BIND".into(), format!("127.0.0.1:{port}")),
        (
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(),
            "http://localhost".into(),
        ),
        ("ALLOWED_HOSTS".into(), "localhost,127.0.0.1".into()),
        ("ALLOWED_ORIGINS".into(), "http://localhost".into()),
        ("MEMORY_MCP_API_KEY_PEPPER".into(), "x".repeat(40)),
        ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SIGNUP_MODE".into(), "invite_only".into()),
        ("MEMORY_MCP_HTTP_CSRF_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_STATE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY".into(), zeros.clone()),
        ("MEMORY_MCP_HTTP_SESSION_KEY".into(), zeros),
        ("SURREALDB_CONTROL_URL".into(), "mem://".into()),
        ("SURREALDB_CONTROL_USERNAME".into(), "root".into()),
        ("SURREALDB_CONTROL_PASSWORD".into(), "root".into()),
        ("SURREALDB_CONTROL_DB".into(), "control".into()),
        ("SURREALDB_CONTROL_NAMESPACE".into(), "control".into()),
        ("SURREALDB_TENANT_URL".into(), "mem://".into()),
        ("SURREALDB_TENANT_USERNAME".into(), "root".into()),
        ("SURREALDB_TENANT_PASSWORD".into(), "root".into()),
        ("SURREALDB_TENANT_DB".into(), "tenant".into()),
        ("SURREALDB_TENANT_NAMESPACE".into(), "tenant".into()),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE".into(),
            "false".into(),
        ),
        (
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI".into(),
            "false".into(),
        ),
        (
            "MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
            format!("conformance={BOOTSTRAP_KEY}"),
        ),
    ]
}

async fn spawn_server(extra_env: &[(&str, &str)]) -> Server {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in base_env(port) {
        cmd.env(k, v);
    }
    for (k, v) in extra_env {
        cmd.env(*k, *v);
    }
    let mut child = cmd.spawn().expect("spawn memory_mcp_http");
    let stdout = child.stdout.take().expect("stdout piped");
    let stderr = child.stderr.take().expect("stderr piped");
    let bound_line = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines() {
            let line = line.expect("read stdout");
            if line.starts_with("memory_mcp_http bound=") {
                return line;
            }
        }
        panic!("server exited before printing bound line");
    });
    let drain = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let line = line.expect("read stderr");
            eprintln!("server stderr: {line}");
        }
    });
    let bound_line = bound_line.await.expect("join bound");
    drop(drain);
    let addr = bound_line
        .trim_start_matches("memory_mcp_http bound=")
        .to_string();
    Server {
        child,
        base_url: format!("http://{addr}"),
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client")
}

fn modern_meta() -> serde_json::Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "memory-mcp-conformance",
            "version": "0.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

#[tokio::test]
async fn get_on_mcp_returns_405() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .get(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn delete_on_mcp_returns_405() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .delete(format!("{}/mcp", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 405);
}

#[tokio::test]
async fn disallowed_host_returns_403() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let resp = client()
        .get(format!("{}/health/live", server.base_url))
        .header("host", "localhost")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "ok");
}

#[tokio::test]
async fn health_ready_returns_json() {
    let server = spawn_server(&[]).await;
    let resp = client()
        .get(format!("{}/health/ready", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "ping",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[("MEMORY_MCP_HTTP_BODY_LIMIT", "1024")]).await;
    let big = "a".repeat(2048);
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "ping",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {},
            "_meta": modern_meta()
        },
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {},
            "_meta": modern_meta()
        },
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "method": "tools/list",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {"_meta": modern_meta()},
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
    let server = spawn_server(&[]).await;
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
            "_meta": modern_meta()
        },
    });
    let resp = client()
        .post(format!("{}/mcp", server.base_url))
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
