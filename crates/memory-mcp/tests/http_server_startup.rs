//! HTTP subprocess-fixture startup failure coverage.
#![cfg(all(feature = "streamable-http", feature = "test-fixtures"))]

mod common;

use common::http_server::{HttpServerConfig, HttpServerFixture};
use std::time::{Duration, Instant};

#[test]
#[ignore = "explicitly executed as the live-child fixture by the parent scenario"]
fn live_child_without_an_address() {
    std::fs::write(
        std::env::var_os("STARTUP_CHILD_PID").expect("owned PID file"),
        std::process::id().to_string(),
    )
    .expect("publish child identity");
    std::thread::sleep(Duration::from_secs(30));
}

#[cfg(unix)]
#[tokio::test]
async fn startup_deadline_kills_and_reaps_a_live_child_without_an_address() {
    let temp = tempfile::tempdir().expect("owned PID directory");
    let pid_file = temp.path().join("child.pid");
    let mut command = std::process::Command::new(std::env::current_exe().expect("test binary"));
    command
        .args(["--exact", "live_child_without_an_address", "--ignored"])
        .env("STARTUP_CHILD_PID", &pid_file);
    let attempt = tokio::spawn(async move {
        HttpServerFixture::spawn_command(HttpServerConfig::default(), command).await
    });
    let outcome = tokio::time::timeout(Duration::from_secs(13), attempt)
        .await
        .expect("startup deadline and cleanup are bounded");
    let failure = match outcome {
        Ok(_) => panic!("a child without an address cannot start a server fixture"),
        Err(error) => error,
    };
    let message = failure.into_panic();
    let reason = message
        .downcast_ref::<String>()
        .expect("startup panic explains the failure");
    assert!(reason.contains("did not report its bound address"));
    let pid = std::fs::read_to_string(pid_file).expect("child actually ran");
    let absent = std::process::Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .status()
        .expect("probe exited child");
    assert!(
        !absent.success(),
        "the child must not survive startup failure"
    );
}

#[tokio::test]
async fn fixture_startup_reaps_a_server_that_exits_before_binding() {
    let config = HttpServerConfig::default().with_env("MEMORY_MCP_HTTP_BIND", "not-a-socket");
    let started = Instant::now();
    let spawn = tokio::spawn(async move { HttpServerFixture::spawn(config).await });

    let result = tokio::time::timeout(Duration::from_secs(12), spawn)
        .await
        .expect("startup failure is bounded");
    let joined = match result {
        Ok(_) => panic!("an invalid bind address must abort fixture startup"),
        Err(error) => error,
    };

    assert!(joined.is_panic());
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the fixture must stop after the child reports startup failure"
    );
}
