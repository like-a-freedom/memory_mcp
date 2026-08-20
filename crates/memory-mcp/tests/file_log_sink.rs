//! Integration test for the process-global file log sink.
//!
//! This test installs the global sink and verifies that `StdoutLogger`
//! writes to the file instead of stderr. It runs in its own binary to
//! avoid polluting other tests with the process-global `OnceLock`.

use std::collections::HashMap;

use memory_mcp::logging::{LogLevel, StdoutLogger, install_log_file};
use serde_json::json;

#[test]
fn install_log_file_writes_events_to_file() {
    let dir = tempfile::TempDir::new().expect("temp dir");
    let log_path = dir.path().join("test.log");
    let path_str = log_path.to_str().expect("valid utf8 path");

    // Install the global file sink
    install_log_file(path_str).expect("install_log_file should succeed");

    // Second install must fail (OnceLock already set) and must not
    // create a stray file at the new path.
    let other_path = dir.path().join("other.log");
    let result = install_log_file(other_path.to_str().expect("valid utf8 path"));
    assert!(result.is_err(), "second install should fail");
    assert!(
        !other_path.exists(),
        "second install must not create a file"
    );

    // Create a logger and emit an event
    let logger = StdoutLogger::new("info");
    let mut event = HashMap::new();
    event.insert("op".to_string(), json!("test.file_sink"));
    event.insert("value".to_string(), json!(42u64));
    logger.log(event, LogLevel::Info);

    // Read the file and verify content
    let content = std::fs::read_to_string(&log_path).expect("read log file");
    assert!(
        content.contains("op=test.file_sink"),
        "missing op field: {content}"
    );
    assert!(
        content.contains("value=42"),
        "missing value field: {content}"
    );
    assert!(content.contains("INFO"), "missing level: {content}");

    // Level filtering applies to the file sink too: a Debug event must
    // not be written by an info-level logger.
    let mut debug_event = HashMap::new();
    debug_event.insert("op".to_string(), json!("test.file_sink_debug"));
    logger.log(debug_event, LogLevel::Debug);
    let content = std::fs::read_to_string(&log_path).expect("read log file");
    assert!(
        !content.contains("test.file_sink_debug"),
        "debug event must be filtered at info level: {content}"
    );
}

#[test]
fn install_log_file_fails_for_nonexistent_directory() {
    // open() fails before OnceLock::set() is reached, so this test is
    // independent of whether another test already installed the sink.
    let dir = tempfile::TempDir::new().expect("temp dir");
    let path = dir.path().join("no_such_dir").join("test.log");
    let result = install_log_file(path.to_str().expect("valid utf8 path"));
    assert!(result.is_err(), "should fail for missing parent directory");
}
