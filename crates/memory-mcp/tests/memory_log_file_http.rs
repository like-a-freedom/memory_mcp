#![cfg(feature = "streamable-http")]
//! `MEMORY_LOG_FILE` must reach the HTTP binary.
//!
//! It used to be installed only by the stdio runner, so the variable was
//! silently ignored in an HTTP deployment — the operator set a path, the
//! container kept writing to stderr, and nothing said why. The sink is
//! installed before configuration is read, so the file appears even when the
//! binary then fails validation, which is what lets this test observe the
//! install without a working server config.

use std::time::{Duration, Instant};

#[test]
fn the_http_binary_installs_the_file_log_sink_from_the_environment() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log_path = dir.path().join("http.log");
    let exe = env!("CARGO_BIN_EXE_memory_mcp_http");

    // Deliberately no HTTP configuration: the binary exits at validation, but
    // only after installing the sink — and the install is what creates the
    // file. A binary that never installs never creates it.
    let mut child = std::process::Command::new(exe)
        .env("MEMORY_LOG_FILE", log_path.to_str().expect("utf8 log path"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn memory_mcp_http");

    // The install happens first thing; poll rather than wait for exit so a
    // binary that lingers on a config prompt cannot hang the test.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !log_path.exists() {
        std::thread::sleep(Duration::from_millis(50));
    }
    let existed = log_path.exists();
    let _ = child.kill();
    let _ = child.wait();

    assert!(
        existed,
        "MEMORY_LOG_FILE must be installed by the HTTP binary before it reads \
         configuration; the file was never created at {}",
        log_path.display()
    );
}
