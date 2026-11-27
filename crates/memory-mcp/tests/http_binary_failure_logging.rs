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
use std::sync::mpsc;
use std::time::Duration;

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

    let (status, stderr) = run_failure(command);
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

#[test]
fn maintenance_failure_preserves_safe_recovery_guidance_without_error_details() {
    let config =
        HttpServerConfig::default().with_tenant(TestTenant::new("conformance", BOOTSTRAP_KEY));
    let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
    command.env_clear();
    for (key, value) in build_env(&config, "mem://") {
        command.env(key, value);
    }
    command
        .arg("surrealdb-v4-maintenance")
        .env("RUST_LOG", "info")
        .env("MEMORY_LOG_FORMAT", "json")
        .env("MEMORY_MCP_HTTP_AUTH_METHODS", "private-error-canary")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (status, stderr) = run_failure(command);
    assert!(!status.success());
    let event = stderr
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["op"] == "http.maintenance_failed")
        .expect("structured maintenance failure");
    assert_eq!(event["error"], "[redacted]");
    assert!(!stderr.contains("private-error-canary"), "{stderr}");
    let guidance = event["guidance"].as_str().expect("safe operator guidance");
    assert!(guidance.contains("Keep HTTP offline"), "{guidance}");
    assert!(
        guidance.contains("partial conversion possible"),
        "{guidance}"
    );
    assert!(
        guidance.contains("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X=true"),
        "{guidance}"
    );
    assert!(
        guidance.contains("restore") && guidance.contains("backup"),
        "{guidance}"
    );
}

fn run_failure(mut command: Command) -> (std::process::ExitStatus, String) {
    let mut child = command.spawn().expect("spawn memory_mcp_http");
    let mut stderr_pipe = child.stderr.take().expect("stderr piped");
    // Read on a thread with a bound: a regression that stops the server from
    // exiting must fail the test in seconds, not hang it.
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = String::new();
        let _ = stderr_pipe.read_to_string(&mut buf);
        let _ = tx.send(buf);
    });
    let stderr = match rx.recv_timeout(Duration::from_secs(20)) {
        Ok(buf) => buf,
        Err(_) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the failing server did not close its stderr within 20s");
        }
    };
    let status = child.wait().expect("reap memory_mcp_http");

    (status, stderr)
}
