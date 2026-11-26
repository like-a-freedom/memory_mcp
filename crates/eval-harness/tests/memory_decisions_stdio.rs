#![cfg(unix)]

pub use eval_harness::EvalError;

#[allow(dead_code)]
#[path = "../src/suites/memory_decisions/receipts.rs"]
mod receipts;
#[path = "../src/suites/memory_decisions/stdio.rs"]
mod stdio;

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use stdio::StdioSession;
use tempfile::TempDir;

fn fake_server(temp: &TempDir, mode: &str) -> PathBuf {
    let path = temp.path().join("fake-memory-server.sh");
    let script = r##"#!/bin/sh
mode='__MODE__'
printf 'started\n' >> "$0.starts"
printf '%s\n' "$SURREALDB_DATA_DIR" > "$0.store-log"
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
    if [ "$mode" = "normal" ] || [ "$mode" = "stderr" ] || [ "$mode" = "call_hang" ] || [ "$mode" = "call_delay" ] || [ "$mode" = "error" ]; then
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"ingest","inputSchema":{"properties":{}}},{"name":"extract","inputSchema":{"properties":{}}},{"name":"assemble_context","inputSchema":{"properties":{"as_of":{}}}}]}}'
    fi
    printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"serverInfo\":{\"name\":\"fake-memory\",\"version\":\"1\"}}}"
  elif [ "$method" = "notifications/initialized" ]; then
    continue
  elif [ "$method" = "tools/list" ]; then
    printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"ingest\",\"inputSchema\":{\"properties\":{}}},{\"name\":\"extract\",\"inputSchema\":{\"properties\":{}}},{\"name\":\"assemble_context\",\"inputSchema\":{\"properties\":{\"as_of\":{}}}}]}}"
  elif [ "$method" = "tools/call" ]; then
    if [ "$mode" = "call_hang" ]; then continue; fi
    if [ "$mode" = "call_delay" ]; then sleep 0.15; fi
    if [ "$mode" = "empty" ] && printf '%s' "$line" | /usr/bin/grep -q '"name":"assemble_context"'; then
      printf '%s\n' "{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"structuredContent\":{\"result\":[]}}}"
    elif [ "$mode" = "error" ]; then
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
fn whole_run_deadline_bounds_initialization_and_reaps_the_server() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "init_hang");
    let store_path = store(&temp, "deadline");
    let started = Instant::now();
    let result = StdioSession::spawn_with_deadline(
        &binary,
        &store_path,
        "default",
        started + Duration::from_millis(700),
    );
    assert!(result.is_err());
    assert!(started.elapsed() < Duration::from_secs(5));
    let pid = std::fs::read_to_string(store_path.join("child.pid")).unwrap();
    let status = Command::new("/bin/kill")
        .args(["-0", pid.trim()])
        .status()
        .unwrap();
    assert!(!status.success(), "timed out server must be reaped");
}

#[test]
fn whole_run_deadline_also_bounds_a_tool_call_after_initialization() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "call_hang");
    let store_path = store(&temp, "deadline-call");
    let started = Instant::now();
    let mut session = StdioSession::spawn_with_deadline(
        &binary,
        &store_path,
        "default",
        started + Duration::from_millis(700),
    )
    .unwrap();
    assert!(session.call_tool("inspect", json!({})).is_err());
    drop(session);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_child_exited(&store_path);
}

#[test]
fn cli_deadline_persists_all_cases_and_leaves_unstarted_attempts_at_zero() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "init_hang");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let profile = temp.path().join("profile.json");
    let artifact = temp.path().join("report.json");
    let receipts = temp.path().join("receipts");
    let tools = temp.path().join("tools");
    std::fs::create_dir(&tools).unwrap();
    let marker = temp.path().join("fingerprint-command-started");
    let script = format!(
        "#!/bin/sh\nprintf called > '{}'\nprintf untrusted-runtime-tool\n",
        marker.display()
    );
    std::fs::write(tools.join("git"), &script).unwrap();
    std::fs::set_permissions(tools.join("git"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(tools.join("rustc"), script).unwrap();
    std::fs::set_permissions(tools.join("rustc"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let tool_path = format!("{}:{}", tools.display(), std::env::var("PATH").unwrap());
    std::fs::write(&profile, serde_json::to_vec(&json!({
        "schema_version":"memory-mcp-eval-profile/v1", "profile":"nightly",
        "time_budget_seconds":1,
        "suites":[{"id":"memory-decisions", "corpus_root":root.join("evals/corpora/memory-decisions-v9")}],
        "gates":[]
    })).unwrap()).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args(["run", "--profile"])
        .arg(&profile)
        .arg("--artifact")
        .arg(&artifact)
        .env("PATH", tool_path)
        .env("MEMORY_DECISIONS_SERVER_BIN", &binary)
        .env("MEMORY_DECISIONS_ARTIFACT_ROOT", &receipts)
        .env("MEMORY_DECISIONS_PRODUCT_REVISION", "deadline-test")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let safety = Instant::now() + Duration::from_secs(15);
    while child.try_wait().unwrap().is_none() {
        if Instant::now() >= safety {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("CLI did not stop at the run deadline");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&artifact).unwrap()).unwrap();
    assert_eq!(report["budget_status"], "failed");
    let outcomes = report["outcomes"].as_array().unwrap();
    assert_eq!(outcomes.len(), 8);
    assert_eq!(
        outcomes.iter().filter(|case| case["attempts"] == 0).count(),
        7
    );
    assert!(receipts.join("index.json").exists());
    assert!(
        !marker.exists(),
        "expired run must not start fingerprint subprocesses"
    );
    let store =
        std::fs::read_to_string(temp.path().join("fake-memory-server.sh.store-log")).unwrap();
    assert!(
        !Path::new(store.trim()).exists(),
        "failed case must clean its temporary store"
    );
}

#[test]
fn cli_receipt_separates_scenario_stages_from_total_wall_time() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "empty");
    let corpus =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../evals/corpora/memory-decisions-v9");
    let profile = temp.path().join("profile.json");
    let receipts = temp.path().join("receipts");
    std::fs::write(&profile, serde_json::to_vec(&json!({
        "schema_version":"memory-mcp-eval-profile/v1", "profile":"nightly", "time_budget_seconds":5,
        "suites":[{"id":"memory-decisions","corpus_root":corpus,"case_ids":["absent-subject"],"diagnostics":false}], "gates":[]
    })).unwrap()).unwrap();
    let result = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args(["run", "--profile"])
        .arg(profile)
        .arg("--artifact")
        .arg(temp.path().join("run.json"))
        .env("MEMORY_DECISIONS_SERVER_BIN", binary)
        .env("MEMORY_DECISIONS_PRODUCT_REVISION", "timing-test")
        .env("MEMORY_DECISIONS_ARTIFACT_ROOT", &receipts)
        .output()
        .unwrap();
    assert!(
        matches!(result.status.code(), Some(0 | 1)),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(temp.path().join("run.json")).unwrap()).unwrap();
    assert_eq!(report["expected_cases"].as_array().unwrap().len(), 1);
    assert_eq!(report["outcomes"].as_array().unwrap().len(), 1);
    assert!(
        report["outcomes"][0]["evidence"]
            .get("evidence_stage_temporal_probe_success")
            .is_none(),
        "omitted diagnostic is unmeasured, not failed"
    );
    assert!(
        report["suite_summaries"][0]["metrics"]
            .get("evidence_stage_temporal_probe_success_rate")
            .is_none()
    );
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(receipts.join("receipt-absent-subject.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["schema_version"], "memory-decisions-receipt/v5");
    assert_eq!(
        std::fs::read_to_string(temp.path().join("fake-memory-server.sh.starts"))
            .unwrap()
            .lines()
            .count(),
        1
    );
    let timings = receipt["primary_stage_timings_us"]
        .as_object()
        .expect("public stage timing receipt");
    assert_eq!(timings.len(), 7);
    let phase_sum: u64 = timings
        .iter()
        .filter(|(key, _)| key.as_str() != "wall")
        .map(|(_, value)| value.as_u64().unwrap())
        .sum();
    assert!(phase_sum <= timings["wall"].as_u64().unwrap());
    assert!(timings["startup"].as_u64().unwrap() > 0);
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
fn stdio_measured_tool_call_records_success_latency_and_compact_response_bytes() {
    let temp = TempDir::new().unwrap();
    let binary = fake_server(&temp, "call_delay");
    let store_path = store(&temp, "store");
    let (startup, call, shutdown) = short_timeouts();

    let mut session =
        StdioSession::spawn_with_timeouts(&binary, &store_path, startup, call, shutdown).unwrap();
    let measured = session
        .call_tool_measured("assemble_context", json!({"query":"fixed"}))
        .unwrap();

    assert!(measured.elapsed_us >= 100_000);
    assert_eq!(
        measured.compact_jsonrpc_bytes,
        serde_json::to_vec(&measured.response).unwrap().len()
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
