//! Proxy streaming/no-buffering tests.
//!
//! Verifies that SSE responses carry the correct headers for
//! proxy streaming: X-Accel-Buffering: no and Cache-Control: no-cache.
//!
//! Run: cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_proxy_streaming

use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};

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
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_proxystreaming0123456789abcdef";

fn base_env(port: u16) -> Vec<(String, String)> {
    vec![
        ("MEMORY_MCP_HTTP_BIND".into(), format!("127.0.0.1:{port}")),
        (
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL".into(),
            format!("http://localhost:{port}"),
        ),
        ("ALLOWED_HOSTS".into(), "localhost,127.0.0.1".into()),
        ("ALLOWED_ORIGINS".into(), "http://localhost".into()),
        ("MEMORY_MCP_API_KEY_PEPPER".into(), "x".repeat(40)),
        ("MEMORY_MCP_SURREALDB_URL".into(), "mem://".into()),
        ("MEMORY_MCP_SURREALDB_NS".into(), "proxy_test".into()),
        ("MEMORY_MCP_SURREALDB_DB".into(), "main".into()),
        (
            "MEMORY_MCP_HTTP_TEST_BOOTSTRAP".into(),
            format!("test={BOOTSTRAP_KEY}"),
        ),
    ]
}

fn spawn_server() -> Server {
    let port = free_port();
    let mut env = base_env(port);
    env.push(("RUST_LOG".into(), "info".into()));

    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();

    let child = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .envs(env_refs.iter().copied())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn server");

    let mut server = Server {
        child,
        base_url: format!("http://127.0.0.1:{port}"),
    };

    let stdout = server.child.stdout.take().unwrap();
    let reader = BufReader::new(stdout);
    for line in reader.lines() {
        let line = line.expect("read line");
        if line.contains("bound=") {
            break;
        }
    }
    server
}

#[tokio::test]
async fn sse_response_carries_x_accel_buffering_no() {
    let server = spawn_server();
    let client = reqwest::Client::new();

    // Initialize SSE connection
    let resp = client
        .post(format!("{}/mcp", server.base_url))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0.1"}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("send request");

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
    let server = spawn_server();
    let client = reqwest::Client::new();

    let resp = client
        .post(format!("{}/mcp", server.base_url))
        .header("content-type", "application/json")
        .header("accept", "text/event-stream")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .body(
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": {"name": "test", "version": "0.1"}
                }
            })
            .to_string(),
        )
        .send()
        .await
        .expect("send request");

    let headers = resp.headers().clone();
    let cache_control = headers.get("cache-control").and_then(|v| v.to_str().ok());
    assert!(
        cache_control
            .map(|v| v.contains("no-cache"))
            .unwrap_or(false),
        "SSE response must carry Cache-Control: no-cache"
    );
}
