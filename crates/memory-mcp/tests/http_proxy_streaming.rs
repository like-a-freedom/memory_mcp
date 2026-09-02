#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

//! Proxy streaming/no-buffering tests.
//!
//! Verifies that SSE responses carry the correct headers for
//! proxy streaming: X-Accel-Buffering: no and Cache-Control: no-cache.
//!
//! Run: cargo test -p memory_mcp --features streamable-http,test-fixtures \
//!      --test http_proxy_streaming

use std::time::Duration;

use serde_json::json;

mod common;

use common::http_server::{HttpServerConfig, HttpServerFixture, TestTenant, modern_meta};

const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_proxystreaming0123456789abcdef12";

fn proxy_tenant() -> TestTenant {
    TestTenant::new("proxy", BOOTSTRAP_KEY)
}

#[tokio::test]
async fn sse_response_carries_x_accel_buffering_no() {
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(proxy_tenant())).await;
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
    let fixture =
        HttpServerFixture::spawn(HttpServerConfig::default().with_tenant(proxy_tenant())).await;
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
