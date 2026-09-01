#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

//! Proxy streaming/no-buffering tests.
//!
//! Verifies that SSE responses carry the correct headers for
//! proxy streaming: X-Accel-Buffering: no and Cache-Control: no-cache.
//!
//! Run: cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_proxy_streaming

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

const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_proxystreaming0123456789abcdef12";

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
            format!("proxy={BOOTSTRAP_KEY}"),
        ),
    ]
}

async fn spawn_server() -> Server {
    let port = free_port();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    for (key, value) in base_env(port) {
        cmd.env(key, value);
    }
    cmd.env("RUST_LOG", "info");
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
    let _stderr_drain = tokio::task::spawn_blocking(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines() {
            let line = line.expect("read stderr");
            eprintln!("server stderr: {line}");
        }
    });
    let bound_line = bound_line.await.expect("join bound");
    let addr = bound_line
        .trim_start_matches("memory_mcp_http bound=")
        .to_string();
    Server {
        child,
        base_url: format!("http://{addr}"),
    }
}

fn modern_meta() -> serde_json::Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {
            "name": "memory-mcp-proxy-test",
            "version": "0.0.0",
        },
        "io.modelcontextprotocol/clientCapabilities": {},
    })
}

#[tokio::test]
async fn sse_response_carries_x_accel_buffering_no() {
    let server = spawn_server().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client
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
        .expect("send request");
    assert_eq!(resp.status(), 200);

    let headers = resp.headers().clone();
    let accel_buffering = headers
        .get("x-accel-buffering")
        .and_then(|v| v.to_str().ok());
    assert_eq!(
        accel_buffering,
        Some("no"),
        "SSE response must carry X-Accel-Buffering: no"
    );
}

#[tokio::test]
async fn sse_response_carries_cache_control_no_cache() {
    let server = spawn_server().await;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("client");

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "server/discover",
        "params": {"_meta": modern_meta()},
    });
    let resp = client
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
        .expect("send request");
    assert_eq!(resp.status(), 200);

    let headers = resp.headers().clone();
    let cache_control = headers.get("cache-control").and_then(|v| v.to_str().ok());
    assert!(
        cache_control
            .map(|v| v.contains("no-cache"))
            .unwrap_or(false),
        "SSE response must carry Cache-Control: no-cache"
    );
}
