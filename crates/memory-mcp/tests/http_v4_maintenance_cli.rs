#![cfg(feature = "streamable-http")]

use std::process::Command;

#[cfg(feature = "test-fixtures")]
mod actual_command {
    use super::*;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use crate::common::http_server::{HttpServerConfig, TestTenant, build_env};

    #[test]
    fn maintenance_runs_without_binding_the_configured_http_address() {
        let config =
            HttpServerConfig::default().with_tenant(TestTenant::new("seed", "test-bootstrap-key"));
        let mut command = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"));
        command
            .arg("surrealdb-v4-maintenance")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("RUST_LOG", "error");
        for (key, value) in build_env(&config, "mem://") {
            command.env(key, value);
        }
        let mut child = command.spawn().expect("spawn HTTP maintenance command");
        let deadline = Instant::now() + Duration::from_secs(20);
        while child
            .try_wait()
            .expect("poll HTTP maintenance command")
            .is_none()
        {
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("maintenance command did not exit; it may have started the HTTP server");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let output = child
            .wait_with_output()
            .expect("collect HTTP maintenance output");
        assert!(
            output.status.success(),
            "maintenance should complete without binding the occupied address; stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let response: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("maintenance JSON response");
        assert_eq!(response["status"], "completed");
        assert_eq!(response["role"], "http");
        assert_eq!(response["ready_tenants"], 0);
        assert_eq!(response["tenant_databases"], 0);
    }
}

#[cfg(feature = "test-fixtures")]
mod common;

#[test]
fn maintenance_help_is_handled_before_http_server_configuration() {
    let output = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .args(["surrealdb-v4-maintenance", "--help"])
        .env_remove("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")
        .output()
        .expect("run HTTP maintenance help");

    assert!(
        output.status.success(),
        "maintenance help should exit before server config is loaded; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--recover-ambiguous"), "{stdout}");
    assert!(
        stdout.contains("--confirm-no-prior-submitter-can-resume"),
        "{stdout}"
    );
}

#[test]
fn ambiguous_recovery_requires_operator_confirmation_before_loading_config() {
    let output = Command::new(env!("CARGO_BIN_EXE_memory_mcp_http"))
        .args(["surrealdb-v4-maintenance", "--recover-ambiguous"])
        .env_remove("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")
        .output()
        .expect("run HTTP maintenance with incomplete recovery authorization");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--confirm-no-prior-submitter-can-resume"),
        "argument validation must run before server configuration; stderr: {stderr}"
    );
}
