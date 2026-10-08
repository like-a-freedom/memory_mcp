//! One request id must span the access log and every event a tool emits.
//!
//! The HTTP transport runs each request on a task of its own, so the correlation
//! scope the access-log middleware opens does not reach the tool by itself. rmcp
//! carries the HTTP request parts on the tool context, and the tool recovers the
//! id from there. Without that recovery the tool mints a fresh `req_NNNN`, and
//! the access line and the tool line name two different ids that cannot be
//! joined. See ADR-0080.

#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

mod common;

use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};

use common::http_server::{HttpServerConfig, TestTenant, build_env, modern_meta};
use serde_json::json;

const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef";

/// Kill the child on every exit path, so a failed assertion cannot leak the
/// server process and hold the harness open.
struct KillOnDrop(Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[tokio::test]
async fn a_tool_event_shares_the_access_lines_request_id() {
    let config =
        HttpServerConfig::default().with_tenant(TestTenant::new("correlation", BOOTSTRAP_KEY));
    let mut env = build_env(&config, "mem://");

    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    command
        .env("RUST_LOG", "info")
        // The structured (NDJSON) format, so the assertion reads the fields
        // rather than the rendered line.
        .env("MEMORY_LOG_FORMAT", "json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env.drain(..) {
        command.env(key, value);
    }

    let mut child = KillOnDrop(command.spawn().expect("spawn memory_mcp_http"));
    let stdout = child.0.stdout.take().expect("stdout piped");
    let stderr = child.0.stderr.take().expect("stderr piped");

    let addr = tokio::task::spawn_blocking(move || {
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                panic!("server exited before reporting its bound address");
            }
            if let Some(addr) = line.strip_prefix("memory_mcp_http bound=") {
                return addr.trim().to_string();
            }
        }
    })
    .await
    .expect("join stdout reader");

    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "ingest",
            "arguments": {
                "source_type": "note",
                "source_id": "correlation-1",
                "content": "correlation smoke",
                "t_ref": "2026-10-08T00:00:00Z",
            },
            "_meta": modern_meta(),
        },
    });
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/mcp"))
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .header("host", "localhost")
        .header("authorization", format!("Bearer {BOOTSTRAP_KEY}"))
        .header("mcp-protocol-version", "2026-07-28")
        .header("mcp-method", "tools/call")
        .header("mcp-name", "ingest")
        .body(body.to_string())
        .send()
        .await
        .expect("send ingest");
    assert_eq!(response.status().as_u16(), 200);
    let request_id = response
        .headers()
        .get("x-request-id")
        .expect("the response advertises the request id")
        .to_str()
        .expect("request id is ASCII")
        .to_string();
    let _ = response.text().await;

    // The tool line is written before the response body, so the id is already on
    // the pipe; killing the server lets the read reach EOF.
    child.0.kill().expect("kill server");
    let _ = child.0.wait();
    let log = tokio::task::spawn_blocking(move || {
        let mut log = String::new();
        let mut reader = BufReader::new(stderr);
        let _ = reader.read_to_string(&mut log);
        log
    })
    .await
    .expect("join stderr reader");

    let tool_line = log
        .lines()
        .find(|line| line.contains("\"op\":\"ingest.done\""))
        .unwrap_or_else(|| panic!("no ingest.done event in the server log:\n{log}"));
    assert!(
        tool_line.contains(&format!("\"request_id\":\"{request_id}\"")),
        "the tool event must carry the id the response advertised ({request_id}), \
         not a freshly minted one:\n{tool_line}"
    );
}
