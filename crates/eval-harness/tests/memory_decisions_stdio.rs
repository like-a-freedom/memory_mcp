#![cfg(unix)]

pub use eval_harness::EvalError;

#[path = "../src/suites/memory_decisions/comparison.rs"]
mod comparison;
#[allow(dead_code)]
#[path = "../src/suites/memory_decisions/receipts.rs"]
mod receipts;
#[path = "../src/suites/memory_decisions/stdio.rs"]
mod stdio;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use serde_json::json;
use stdio::StdioSession;
use tempfile::TempDir;

fn fake_server(temp: &TempDir, mode: &str) -> PathBuf {
    let path = temp.path().join("fake-memory-server.sh");
    let script = r##"#!/bin/sh
mode='__MODE__'
printf '%s\n' "$$" > "$SURREALDB_DATA_DIR/child.pid"
printf '%s\n' "$MEMORY_CLAIM_ROLLOUT_STAGE" > "$SURREALDB_DATA_DIR/claim-stage.txt"
pwd > "$SURREALDB_DATA_DIR/working-directory.txt"
if [ "$mode" = "stderr" ]; then
  dd if=/dev/zero bs=8192 count=256 2>/dev/null | tr '\000' 'x' >&2
fi
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  method=$(printf '%s\n' "$line" | sed -n 's/.*"method":"\([^"]*\)".*/\1/p')
  if [ -z "$id" ]; then
    continue
  fi
  if [ "$mode" = "early" ]; then exit 0; fi
  if [ "$mode" = "init_hang" ] && [ "$method" = "initialize" ]; then continue; fi
  if [ "$method" = "initialize" ]; then
    if [ "$mode" = "normal" ] || [ "$mode" = "stderr" ] || [ "$mode" = "call_hang" ] || [ "$mode" = "error" ]; then
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ingest","inputSchema":{"properties":{}}},{"name":"extract","inputSchema":{"properties":{}}},{"name":"assemble_context","inputSchema":{"properties":{"as_of":{}}}}]}}'
    fi
    printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"serverInfo\":{\"name\":\"fake-memory\",\"version\":\"1\"}}}"
  elif [ "$method" = "notifications/initialized" ]; then
    continue
  elif [ "$method" = "tools/list" ]; then
    printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"ingest\",\"inputSchema\":{\"properties\":{}}},{\"name\":\"extract\",\"inputSchema\":{\"properties\":{}}},{\"name\":\"assemble_context\",\"inputSchema\":{\"properties\":{\"as_of\":{}}}}]}}"
  elif [ "$method" = "tools/call" ]; then
    if [ "$mode" = "call_hang" ]; then continue; fi
    if [ "$mode" = "error" ]; then
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"isError\":true,\"content\":[{\"type\":\"text\",\"text\":\"tool failed\"}]}}"
    else
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"structuredContent\":{\"status\":\"success\",\"result\":\"$SURREALDB_DATA_DIR\"}}}"
    fi
  fi
done
"##
    .replace("__MODE__", mode);
    std::fs::write(&path, script).expect("write fake server");
    let mut permissions = std::fs::metadata(&path).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).unwrap();
    path
}

fn store(temp: &TempDir, name: &str) -> PathBuf {
    let path = temp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn short_timeouts() -> (Duration, Duration, Duration) {
    (
        Duration::from_secs(5),
        Duration::from_millis(300),
        Duration::from_millis(500),
    )
}

#[test]
fn stdio_initializes_without_waiting_for_a_notification_reply_and_buffers_out_of_order_ids() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "normal");
    let store_path = store(&temp, "store");

    let mut session = StdioSession::spawn(&binary, &store_path).unwrap();
    let response = session
        .call_tool("inspect", json!({}))
        .expect("tools/call succeeds");

    assert_eq!(session.initialized()["serverInfo"]["name"], "fake-memory");
    assert_eq!(session.tool_schemas().as_array().unwrap().len(), 3);
    assert_eq!(
        response["result"]["structuredContent"]["result"],
        store_path.display().to_string()
    );
    session.shutdown().unwrap();
}

#[test]
fn stdio_rejects_an_mcp_is_error_result() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "error");
    let store_path = store(&temp, "store");
    let (_, call, shutdown) = short_timeouts();
    let startup = Duration::from_millis(500);

    let mut session =
        StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown).unwrap();
    let result = session.call_tool("inspect", json!({}));

    assert!(result.is_err());
}

#[test]
fn stdio_drains_large_stderr_without_blocking_json_rpc() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "stderr");
    let store_path = store(&temp, "store");
    let (startup, call, shutdown) = short_timeouts();

    let mut session =
        StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown).unwrap();
    let response = session.call_tool("inspect", json!({})).unwrap();

    assert_eq!(
        response["result"]["structuredContent"]["result"],
        store_path.display().to_string()
    );
    session.shutdown().unwrap();
    let stderr_excerpt = session.stderr_excerpt();
    assert!(!stderr_excerpt.is_empty());
    assert!(stderr_excerpt.len() <= 32_768);
}

#[test]
fn stdio_bounds_the_initialize_wait_and_kills_the_child_on_timeout() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "init_hang");
    let store_path = store(&temp, "store");
    let (_, call, shutdown) = short_timeouts();
    let startup = Duration::from_millis(500);

    let result = StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown);

    assert!(result.is_err());
    assert_child_exited(&store_path);
}

#[test]
fn stdio_bounds_tool_calls_and_reaps_a_timed_out_child() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "call_hang");
    let store_path = store(&temp, "store");
    let (startup, call, shutdown) = short_timeouts();
    let mut session =
        StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown).unwrap();
    let result = session.call_tool("inspect", json!({}));

    assert!(result.is_err());
    drop(session);
    assert_child_exited(&store_path);
}

#[test]
fn stdio_reports_early_child_exit() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "early");
    let store_path = store(&temp, "store");
    let (startup, call, shutdown) = short_timeouts();

    let result = StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown);

    assert!(result.is_err());
}

#[test]
fn separate_sessions_receive_distinct_store_paths() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "normal");
    let first_store = store(&temp, "first-store");
    let second_store = store(&temp, "second-store");
    let (startup, call, shutdown) = short_timeouts();
    let mut first =
        StdioSession::spawn_with_timeouts(&binary, &first_store, startup, call, shutdown).unwrap();
    let mut second =
        StdioSession::spawn_with_timeouts(&binary, &second_store, startup, call, shutdown).unwrap();

    let first_result = first.call_tool("inspect", json!({})).unwrap();
    let second_result = second.call_tool("inspect", json!({})).unwrap();

    assert_eq!(
        first_result["result"]["structuredContent"]["result"],
        first_store.display().to_string()
    );
    assert_eq!(
        second_result["result"]["structuredContent"]["result"],
        second_store.display().to_string()
    );
}

#[test]
fn stdio_process_uses_the_explicit_claim_rollout_stage() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "normal");
    let store_path = store(&temp, "evidence-store");

    let mut session =
        StdioSession::spawn_with_claim_stage(&binary, &store_path, "evidence").unwrap();

    assert_eq!(
        std::fs::read_to_string(store_path.join("claim-stage.txt"))
            .unwrap()
            .trim(),
        "evidence"
    );
    assert_eq!(
        session.launch_profile["MEMORY_CLAIM_ROLLOUT_STAGE"],
        "evidence"
    );
    session.shutdown().unwrap();
}

#[test]
fn stdio_process_can_resolve_legacy_relative_sources_from_the_registered_root() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "normal");
    let store_path = store(&temp, "source-root-store");
    let source_root = temp.path().join("registered-source-root");
    std::fs::create_dir(&source_root).unwrap();

    let mut session = StdioSession::spawn_with_claim_stage_and_cwd(
        &binary,
        &store_path,
        "evidence",
        &source_root,
    )
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(store_path.join("working-directory.txt"))
            .unwrap()
            .trim(),
        std::fs::canonicalize(&source_root)
            .unwrap()
            .display()
            .to_string()
    );
    session.shutdown().unwrap();
}

#[test]
fn default_stdio_session_leaves_claim_stage_unset_for_product_configuration() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "normal");
    let store_path = store(&temp, "default-store");

    let mut session = StdioSession::spawn(&binary, &store_path).unwrap();

    assert_eq!(
        std::fs::read_to_string(store_path.join("claim-stage.txt"))
            .unwrap()
            .trim(),
        "",
        "the default evaluation must not pass a rollout override"
    );
    assert!(
        !session
            .launch_profile
            .contains_key("MEMORY_CLAIM_ROLLOUT_STAGE")
    );
    session.shutdown().unwrap();
}

#[test]
fn receipt_writes_reject_existing_paths_without_changing_the_saved_bytes() {
    let temp = TempDir::new().unwrap();
    let value = serde_json::json!({"case_id":"case-a","score":{"probe_success":false}});

    let (path, _) = receipts::write_receipt(temp.path(), "case-a", &value).unwrap();
    let before = std::fs::read(&path).unwrap();
    let second_write =
        receipts::write_receipt(temp.path(), "case-a", &serde_json::json!({"changed":true}));

    assert!(second_write.is_err());
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[test]
#[ignore = "runs after two pinned memory-decisions profile executions"]
fn compare_saved_replays() {
    let left_path =
        std::env::var("MEMORY_DECISIONS_REPLAY_A").expect("set MEMORY_DECISIONS_REPLAY_A");
    let right_path =
        std::env::var("MEMORY_DECISIONS_REPLAY_B").expect("set MEMORY_DECISIONS_REPLAY_B");
    let left: receipts::ReplayIndex =
        serde_json::from_slice(&std::fs::read(left_path).unwrap()).unwrap();
    let right: receipts::ReplayIndex =
        serde_json::from_slice(&std::fs::read(right_path).unwrap()).unwrap();

    comparison::compare_replays(&left, &right).expect("semantic replay results must match");
}

fn assert_child_exited(store: &Path) {
    let pid = std::fs::read_to_string(store.join("child.pid")).unwrap();
    let status = Command::new("kill")
        .args(["-0", pid.trim()])
        .status()
        .unwrap();
    assert!(
        !status.success(),
        "child process {} must be reaped",
        pid.trim()
    );
}
