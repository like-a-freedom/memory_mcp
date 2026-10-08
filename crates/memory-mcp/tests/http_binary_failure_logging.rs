//! A failure after `logging::install()` must reach the log sink, not stderr prose.
//!
//! The HTTP binary installs the subscriber early, so a bind failure at
//! `server::serve` is a post-install failure: it belongs on the same structured
//! stream as every other event, not on a bare `eprintln!` line an operator's
//! collector cannot parse or filter by `op`.

#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

mod common;

use std::io::Read;
use std::process::{Command, Stdio};

use common::http_server::{HttpServerConfig, TestTenant, build_env};

/// A well-formed bootstrap key, so the binary's config validation passes and it
/// reaches the bind — the failure under test.
const BOOTSTRAP_KEY: &str =
    "mem_sk_ak_01234567-89ab-4cde-8f01-23456789abcd_conformancesuite0123456789abcdef";

#[test]
fn serve_failed_is_structured() {
    // Occupy a port, then point the binary at it so its bind fails *after*
    // logging is installed — the seam this test exists for.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind an occupied port");
    let occupied = listener.local_addr().expect("occupied port address");

    let config =
        HttpServerConfig::default().with_tenant(TestTenant::new("conformance", BOOTSTRAP_KEY));
    let mut env = build_env(&config, "mem://");
    env.push(("MEMORY_MCP_HTTP_BIND".to_string(), occupied.to_string()));

    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    command
        .env("RUST_LOG", "info")
        // The structured (NDJSON) format, so the assertion reads the fields
        // rather than the rendered line.
        .env("MEMORY_LOG_FORMAT", "json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (key, value) in env {
        command.env(key, value);
    }

    let mut child = command.spawn().expect("spawn memory_mcp_http");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr piped")
        .read_to_string(&mut stderr)
        .expect("read server stderr");
    let status = child.wait().expect("reap memory_mcp_http");
    drop(listener);

    assert!(!status.success(), "a bind failure must exit non-zero");
    assert!(
        stderr.contains("\"op\":\"http.serve_failed\""),
        "the bind failure must be a structured event, not prose: {stderr}"
    );
    assert!(
        stderr.contains("\"level\":\"error\""),
        "the bind failure must be at ERROR: {stderr}"
    );
}
