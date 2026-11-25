use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::Duration;

use eval_harness::codex_agent::{
    CodexAgentConfig, CodexMemoryToolProfile, CodexMemoryTransport, run_codex_agent,
};
use eval_harness::paired::TrialStatus;

fn fake_config(root: &Path, script: &str, wall_time_budget: Duration) -> CodexAgentConfig {
    let codex_home = root.join("codex-home");
    let home = root.join("home");
    let database = root.join("database");
    let working = root.join("agent-workspace");
    for path in [&codex_home, &home, &database, &working] {
        std::fs::create_dir(path).expect("create isolated fixture directory");
    }
    let server = root.join("memory-mcp");
    std::fs::write(&server, "fixture").expect("create server fixture");
    let fake_codex = root.join("codex-fake.sh");
    std::fs::write(&fake_codex, script).expect("write fake Codex executable");
    let mut permissions = std::fs::metadata(&fake_codex)
        .expect("read fake Codex permissions")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&fake_codex, permissions).expect("make fake Codex executable");
    CodexAgentConfig {
        codex_binary: fake_codex,
        codex_home,
        home_directory: home,
        model: "pinned-model".into(),
        claim_rollout_stage: "shadow".into(),
        memory_server_binary: server,
        database_path: database,
        working_directory: working,
        memory_server_working_directory: None,
        context_budget_tokens: 4096,
        tool_call_budget: 12,
        turn_budget: 20,
        output_token_budget: 1024,
        wall_time_budget,
        max_jsonl_bytes: 1_048_576,
        memory_transport: CodexMemoryTransport::Stdio,
        memory_tool_profile: CodexMemoryToolProfile::RecallOnly,
        codex_api_key: None,
    }
}

#[test]
fn kills_fake_codex_when_tool_call_budget_is_exceeded() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.started","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"A bounded task"}}}' \
  '{"type":"item.completed","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"A bounded task"}}}' \
  '{"type":"item.started","item":{"id":"call-2","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"A bounded task"}}}'
exec /bin/sleep 2
"##,
        Duration::from_secs(5),
    );
    config.tool_call_budget = 1;

    let observation = run_codex_agent(&config, "A bounded task")
        .expect("over-budget Codex attempt remains available as a failure receipt");

    assert_eq!(observation.status, TrialStatus::BudgetExceeded);
    assert_eq!(observation.tool_calls, 2);
    assert_eq!(
        observation.trace.last().expect("terminal event").kind,
        eval_harness::paired::TraceEventKind::BudgetLimitReached
    );
}

#[test]
fn kills_fake_codex_when_turn_budget_is_exceeded() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"turn.failed","usage":{"input_tokens":4,"output_tokens":2}}' \
  '{"type":"turn.started"}'
exec /bin/sleep 2
"##,
        Duration::from_secs(5),
    );
    config.turn_budget = 1;

    let observation = run_codex_agent(&config, "A bounded task")
        .expect("over-budget Codex attempt remains available as a failure receipt");

    assert_eq!(observation.status, TrialStatus::BudgetExceeded);
    assert_eq!(observation.turns_used, 2);
    assert_eq!(
        observation.trace.last().expect("terminal event").kind,
        eval_harness::paired::TraceEventKind::BudgetLimitReached
    );
}

#[test]
fn kills_fake_codex_when_aggregate_output_token_budget_is_exceeded() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":4}}' \
  '{"type":"turn.started"}' \
  '{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":4}}'
exec /bin/sleep 2
"##,
        Duration::from_secs(5),
    );
    config.output_token_budget = 7;

    let observation = run_codex_agent(&config, "A bounded task")
        .expect("over-budget Codex attempt remains available as a failure receipt");

    assert_eq!(observation.status, TrialStatus::BudgetExceeded);
    assert_eq!(observation.output_tokens, 8);
    assert_eq!(
        observation.trace.last().expect("terminal event").kind,
        eval_harness::paired::TraceEventKind::BudgetLimitReached
    );
}

#[test]
fn kills_fake_codex_when_jsonl_output_byte_budget_is_exceeded() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"turn.completed","usage":{"input_tokens":4,"output_tokens":2}}'
exec /bin/sleep 2
"##,
        Duration::from_secs(5),
    );
    config.max_jsonl_bytes = 20;

    let observation = run_codex_agent(&config, "A bounded task")
        .expect("oversized Codex attempt remains available as a failure receipt");

    assert_eq!(observation.status, TrialStatus::BudgetExceeded);
    assert_eq!(
        observation.trace.last().expect("terminal event").kind,
        eval_harness::paired::TraceEventKind::BudgetLimitReached
    );
}

#[test]
fn text_only_tool_reply_has_unknown_read_outcome() {
    let directory = tempfile::tempdir().unwrap();
    let config = fake_config(
        directory.path(),
        r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '{"type":"turn.started"}' \
 '{"type":"item.started","item":{"id":"read","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"Cedar"}}}' \
 '{"type":"item.completed","item":{"id":"read","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","result":{"content":[{"type":"text","text":"UNRECOGNIZED_REPLY_SENTINEL"}]}}}' \
 '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"#,
        Duration::from_secs(5),
    );
    let captured =
        serde_json::to_value(run_codex_agent(&config, "Where is Cedar?").unwrap()).unwrap();
    assert_eq!(captured["memory_reads"][0]["outcome"], "unknown");
    assert!(captured["memory_reads"][0]["elapsed_ms"].as_u64().is_some());
    assert!(captured["memory_reads"][0]["item_count"].is_null());
    assert!(!captured.to_string().contains("UNRECOGNIZED_REPLY_SENTINEL"));
}

#[test]
fn concurrent_reads_keep_elapsed_timing_with_the_matching_response() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
 '{"type":"turn.started"}' \
 '{"type":"item.started","item":{"id":"read-first","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"PRIVATE FIRST QUERY"}}}' \
 '{"type":"item.started","item":{"id":"read-second","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"PRIVATE SECOND QUERY"}}}' \
 '{"type":"item.completed","item":{"id":"read-second","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","result":{"structuredContent":{"result":[]}}}}' \
 '{"type":"item.completed","item":{"id":"read-first","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","result":{"structuredContent":{"result":[]}}}}' \
 '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"{\"answer\":\"unknown\",\"citations\":[],\"needs_clarification\":true}"}}' \
 '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"##,
        Duration::from_secs(5),
    );

    let observation = run_codex_agent(&config, "Find either recorded value.")
        .expect("interleaved tool calls produce a complete observation");
    let reads = observation
        .memory_reads
        .as_ref()
        .expect("captured read observations");

    assert_eq!(reads.len(), 2);
    for read in reads {
        assert!(read.elapsed_ms.is_some());
        let request = observation
            .trace
            .iter()
            .find(|event| event.sequence == read.request_sequence)
            .expect("read request is present in trace");
        let response = observation
            .trace
            .iter()
            .find(|event| Some(event.sequence) == read.response_sequence)
            .expect("read response is present in trace");
        assert_eq!(request.call_id, response.call_id);
    }
    let captured = serde_json::to_string(&observation).expect("serialize content-free telemetry");
    assert!(!captured.contains("PRIVATE FIRST QUERY"));
    assert!(!captured.contains("PRIVATE SECOND QUERY"));
}

#[test]
fn completion_without_observed_start_does_not_fabricate_read_timing() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' '{"type":"turn.started"}'
printf '%s\n' '{"type":"item.completed","item":{"id":"read","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"PRIVATE QUERY"},"result":{"structuredContent":{"result":[]}}}}'
printf '%s\n' '{"type":"item.completed","item":{"id":"answer","type":"agent_message","text":"{\"answer\":\"unknown\",\"citations\":[],\"needs_clarification\":true}"}}'
printf '%s\n' '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"##,
        Duration::from_secs(5),
    );

    let observation = run_codex_agent(&config, "Where is Cedar?")
        .expect("the process runner records completion-only telemetry as partial");

    assert_eq!(
        observation.read_capture_status,
        Some(eval_harness::paired_agent::ReadCaptureStatus::Partial)
    );
    assert!(observation.memory_reads.unwrap_or_default().is_empty());
}

#[test]
fn launches_fake_codex_with_isolated_mcp_and_removes_temporary_schema() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let config = fake_config(
        directory.path(),
        r##"#!/bin/sh
args="$*"
case "$args" in
  *"--json"*"--disable shell_tool"*"enabled_tools=[\"assemble_context\"]"*) ;;
  *) exit 73 ;;
esac
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"FJ-38\",\"citations\":[\"episode-1\"],\"needs_clarification\":false}"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"##,
        Duration::from_secs(5),
    );

    let actual = run_codex_agent(&config, "Which station code is current?")
        .expect("fake Codex run emits a valid trace");

    assert_eq!(actual.status, TrialStatus::Completed);
    assert_eq!(
        actual.final_answer.as_deref(),
        Some(r#"{"answer":"FJ-38","citations":["episode-1"],"needs_clarification":false}"#)
    );
    assert_eq!(actual.input_tokens, 9);
    assert_eq!(actual.output_tokens, 4);
    assert!(
        !config
            .working_directory
            .join("codex-output-schema.json")
            .exists()
    );
}

#[test]
fn write_profile_enables_only_registered_memory_mutations_and_keeps_shell_disabled() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
args="$*"
case "$args" in
  *"--disable shell_tool"*"enabled_tools=[\"assemble_context\",\"ingest\",\"extract\",\"invalidate\"]"*) ;;
  *) exit 73 ;;
esac
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.completed","item":{"id":"write-1","type":"mcp_tool_call","server":"memory_decisions","tool":"ingest","arguments":{"content":"PRIVATE MEMORY TEXT"},"result":{"structuredContent":{"result":"episode:agent-created"}}}}' \
  '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"stored\",\"citations\":[\"episode:agent-created\"],\"needs_clarification\":false}"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":21,"output_tokens":7}}'
"##,
        Duration::from_secs(5),
    );
    config.memory_tool_profile = CodexMemoryToolProfile::AgentManagedMemory;

    let actual = run_codex_agent(&config, "Store the durable fact and verify it.")
        .expect("explicit agent-managed profile runs");

    assert_eq!(actual.status, TrialStatus::Completed);
    assert_eq!(actual.tool_calls, 1);
    assert_eq!(
        actual.memory_write_episode_ids,
        vec!["episode:agent-created"]
    );
    assert!(actual.trace.iter().any(|event| {
        event.kind == eval_harness::paired::TraceEventKind::MemoryWrite
            && event.tool_name.as_deref() == Some("memory_decisions/ingest")
    }));
    let trace = serde_json::to_string(&actual.trace).expect("serialize metadata-only trace");
    assert!(!trace.contains("PRIVATE MEMORY TEXT"));
}

#[test]
fn maintenance_v2_profile_runs_through_agent_runner() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"corrected\",\"citations\":[\"episode:corrected\"],\"needs_clarification\":false}"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"##,
        Duration::from_secs(5),
    );
    config.memory_tool_profile = CodexMemoryToolProfile::AgentMaintenanceV2;

    let actual = run_codex_agent(&config, "Apply the registered correction.")
        .expect("maintenance profile launches with a bounded reader response");

    assert_eq!(actual.status, TrialStatus::Completed);
}

#[test]
fn maintenance_v4_profile_uses_only_memory_tools_needed_for_safe_correction() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let mut config = fake_config(
        directory.path(),
        r##"#!/bin/sh
printf '%s\n' "$@" > "$HOME/codex-args"
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.completed","item":{"id":"answer-1","type":"agent_message","text":"{\"answer\":\"corrected\",\"citations\":[\"episode:corrected\"],\"needs_clarification\":false}"}}' \
  '{"type":"turn.completed","usage":{"input_tokens":9,"output_tokens":4}}'
"##,
        Duration::from_secs(5),
    );
    config.memory_tool_profile =
        CodexMemoryToolProfile::from_name("agent_maintenance_v4").expect("registered profile");
    assert_eq!(
        config.memory_tool_profile.enabled_tools(),
        &["assemble_context", "extract", "invalidate"]
    );

    run_codex_agent(&config, "Apply the registered correction.")
        .expect("v4 profile launches with a pinned narrow tool surface");
    let args = std::fs::read_to_string(config.home_directory.join("codex-args"))
        .expect("fake Codex captured the public command invocation");
    assert!(args.contains("--disable\nshell_tool"));
    assert!(args.contains(
        "mcp_servers.memory_decisions.enabled_tools=[\"assemble_context\",\"extract\",\"invalidate\"]"
    ));
    assert!(
        args.contains(
            "mcp_servers.memory_decisions.tools.assemble_context.output_token_limit=1024"
        )
    );
}

#[test]
fn kills_fake_codex_when_wall_time_budget_expires() {
    let directory = tempfile::tempdir().expect("temporary runner directory");
    let config = fake_config(
        directory.path(),
        r##"#!/bin/sh
cat >/dev/null
printf '%s\n' \
  '{"type":"turn.started"}' \
  '{"type":"item.started","item":{"id":"call-1","type":"mcp_tool_call","server":"memory_decisions","tool":"assemble_context","arguments":{"query":"A bounded task"}}}'
  exec /bin/sleep 5
"##,
        Duration::from_secs(1),
    );

    let observation = run_codex_agent(&config, "A bounded task")
        .expect("timed-out Codex attempt remains available as a failure receipt");

    assert_eq!(observation.status, TrialStatus::TimedOut);
    assert_eq!(observation.trace.len(), 3);
    assert_eq!(
        observation.trace[0].kind,
        eval_harness::paired::TraceEventKind::AgentTurnStarted
    );
    assert_eq!(
        observation.trace[1].kind,
        eval_harness::paired::TraceEventKind::MemoryRead
    );
    assert!(
        observation.memory_reads.as_ref().unwrap()[0]
            .elapsed_ms
            .is_none()
    );
    assert_eq!(
        observation.trace[2].kind,
        eval_harness::paired::TraceEventKind::Timeout
    );
    assert!(
        !config
            .working_directory
            .join("codex-output-schema.json")
            .exists()
    );
}
