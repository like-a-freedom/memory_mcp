//! Integration tests: real loopback HTTP, filesystem and a controlled child process.
use super::*;
use std::{
    io::{Read, Write},
    net::TcpListener,
    thread,
};

struct MockChatServer {
    endpoint: String,
    requests: thread::JoinHandle<Vec<Value>>,
}
impl MockChatServer {
    fn with_responses(responses: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let requests = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut requests = vec![];
            for response in responses {
                let mut stream = loop {
                    match listener.accept() {
                        Ok((stream, _)) => break stream,
                        Err(error)
                            if error.kind() == std::io::ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::yield_now()
                        }
                        Err(error) => panic!("bounded controlled endpoint: {error}"),
                    }
                };
                stream.set_nonblocking(false).unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut headers = vec![];
                let mut byte = [0];
                while !headers.ends_with(b"\r\n\r\n") {
                    stream.read_exact(&mut byte).unwrap();
                    headers.push(byte[0]);
                }
                let length: usize = String::from_utf8(headers)
                    .unwrap()
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .unwrap();
                let mut body = vec![0; length];
                stream.read_exact(&mut body).unwrap();
                requests.push(serde_json::from_slice(&body).unwrap());
                let body = response.to_string();
                write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",body.len(),body).unwrap();
            }
            requests
        });
        Self { endpoint, requests }
    }
    fn requests(self) -> Vec<Value> {
        self.requests.join().unwrap()
    }
}
fn model_response(message: Value, prompt_tokens: u64, completion_tokens: u64) -> Value {
    json!({"model":"test-model","choices":[{"message":message}],"usage":{"prompt_tokens":prompt_tokens,"completion_tokens":completion_tokens}})
}
fn budgets() -> ChatCompletionBudgets {
    ChatCompletionBudgets {
        context_tokens: 10000,
        tool_calls: 2,
        turns: 3,
        output_tokens: 1000,
        wall_time: Duration::from_millis(500),
    }
}
// Removing phase-to-transport cancellation must make these real blocking reads
// outlive the phase budget, even though the ordinary tool timeout is bounded.
#[test]
fn phase_deadline_interrupts_fixed_recall_and_reaps_owned_server() {
    assert_blocked_mcp_phase(1);
}

#[test]
fn phase_deadline_retains_a_completed_read_before_the_blocked_read() {
    assert_blocked_mcp_phase(2);
}

fn assert_blocked_mcp_phase(block_on: usize) {
    use crate::paired::{CaseTrialKey, NormalizedErrorClass, RunArm, TrialStatus};
    use crate::paired_agent::{
        MemoryReadOutcome,
        chat_completions::{RecallMemorySession, RecallReadMode},
        public_trace::{PublicSyntheticSession, SyntheticTraceIdentity},
    };
    use crate::suites::memory_decisions::stdio::StdioSession;
    use sha2::{Digest, Sha256};
    use std::{fs, os::unix::fs::PermissionsExt, process::Command, time::Duration};
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("blocking-memory");
    let tools = json!([{"name":"ingest","inputSchema":{}},{"name":"extract","inputSchema":{}},{"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"budget":{"type":"integer"}},"required":["query","budget"]}}]);
    let script = r##"#!/bin/sh
printf '%s' "$$" > "$SURREALDB_DATA_DIR/child.pid"
n=0
while IFS= read -r line; do
 id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 [ -n "$id" ] || continue
 case "$line" in
 *'"method":"initialize"'*) result='{}' ;;
 *'"method":"tools/list"'*) result='{"tools":__TOOLS__}' ;;
 *'"name":"assemble_context"'*)
   n=$((n+1))
   [ "$n" -eq __BLOCK_ON__ ] && continue
   result='{"structuredContent":{"result":[{"source_episode":"episode:cedar","content":"Cabinet-28"}]}}' ;;
 *) exit 2 ;;
 esac
 printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"##.replace("__TOOLS__", &tools.to_string()).replace("__BLOCK_ON__", &block_on.to_string());
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let store = directory.path().join("store");
    let mut session = StdioSession::spawn_with_timeouts(
        &binary,
        &store,
        Duration::from_secs(5),
        Duration::from_secs(2),
        Duration::from_millis(50),
    )
    .unwrap();
    let responses = (0..block_on).map(|index| model_response(json!({"role":"assistant","content":null,"tool_calls":[{"id":format!("read-{index}"),"type":"function","function":{"name":"assemble_context","arguments":"{\"query\":\"Where is Cedar?\",\"budget\":1}"}}]}),40,12)).collect();
    let server = MockChatServer::with_responses(responses);
    let agent = ChatCompletionsAgent::new(ChatCompletionsConfig::new(
        server.endpoint.clone(),
        "test-model",
    ))
    .unwrap();
    let identity = SyntheticTraceIdentity {
        plan_sha256: "1".repeat(64),
        product_binary_sha256: "2".repeat(64),
        tool_schema_sha256: hex::encode(Sha256::digest(serde_json::to_vec(&tools).unwrap())),
        key: CaseTrialKey {
            stratum_id: "deadline".into(),
            family_id: "blocked".into(),
            case_id: "read".into(),
            repetition: 0,
            incumbent_first: true,
        },
        arm: RunArm::Candidate,
        phase: "recall".into(),
    };
    let mut traced = PublicSyntheticSession::new(&mut session, identity).unwrap();
    let mut limits = budgets();
    limits.wall_time = Duration::from_millis(500);
    limits.tool_calls = 2;
    let started = Instant::now();
    let observation = if block_on == 1 {
        let mut fixed = RecallMemorySession::new(
            &mut traced,
            RecallReadMode::FixedReply(json!({"query":"Where is Cedar?","budget":1})),
        )
        .unwrap();
        agent.run_phase(
            &mut fixed,
            "Where is Cedar?",
            "Use memory.",
            "Return JSON.",
            ChatCompletionsToolProfile::RecallOnly,
            limits,
        )
    } else {
        agent.run_phase(
            &mut traced,
            "Where is Cedar?",
            "Use memory.",
            "Return JSON.",
            ChatCompletionsToolProfile::RecallOnly,
            limits,
        )
    }
    .unwrap();
    let phase_elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    assert!(
        started.elapsed() < Duration::from_millis(1500),
        "phase deadline must interrupt the two-second transport wait"
    );
    assert_eq!(observation.status, TrialStatus::TimedOut);
    assert_eq!(
        observation.trace.last().unwrap().error_class,
        Some(NormalizedErrorClass::Timeout)
    );
    let mut manifest: crate::paired_agent::PairedTaskManifest = serde_json::from_str(include_str!(
        "../../../../../evals/paired/recall-read-local-screen-2026-11-24/tasks.json"
    ))
    .unwrap();
    let mut plan: crate::paired::PairedEvaluationPlan = serde_json::from_str(include_str!(
        "../../../../../evals/paired/recall-read-local-screen-2026-11-24/live-plan.json"
    ))
    .unwrap();
    manifest.tasks.truncate(2);
    plan.expected_trials = manifest.tasks.iter().map(|task| task.key.clone()).collect();
    plan.planned_independent_families = 2;
    plan.shared_identity.corpus_sha256 = manifest.corpus_sha256().unwrap();
    plan.shared_identity.oracle_sha256 = manifest.oracle_sha256().unwrap();
    plan.shared_identity.tool_call_budget = 2;
    plan.shared_identity.wall_time_budget_ms = 500;
    let execution = crate::paired_agent::PairedTrialExecution {
        observation: observation.clone(),
        episode_id_to_event_id: [(
            "episode:cedar".into(),
            manifest.tasks[0].seed_episodes[0].event_id.clone(),
        )]
        .into(),
    };
    // Replay the captured real execution through the public artifact pipeline.
    // This checks serialization and validation, not independent paired outcomes.
    struct CapturedExecution(crate::paired_agent::PairedTrialExecution);
    impl crate::paired_agent::PairedTrialBackend for CapturedExecution {
        fn execute(
            &mut self,
            _arm: RunArm,
            task: &crate::paired_agent::PreparedPairedTask,
        ) -> Result<crate::paired_agent::PairedTrialExecution, EvalError> {
            let mut execution = self.0.clone();
            execution.episode_id_to_event_id = [(
                "episode:cedar".into(),
                task.input.seed_episodes[0].event_id.clone(),
            )]
            .into();
            Ok(execution)
        }
    }
    let output = directory.path().join("reports");
    let (incumbent, candidate) = crate::paired_agent::run_paired_agent_evaluation(
        &plan,
        &manifest,
        &mut CapturedExecution(execution),
        &output,
    )
    .expect("public runner must persist a timed-out execution as valid reports");
    incumbent.validate().unwrap();
    candidate.validate().unwrap();
    assert_eq!(candidate.receipts[0].status, TrialStatus::TimedOut);
    let persisted: crate::paired::PairedRunArtifact =
        serde_json::from_slice(&fs::read(output.join("candidate.json")).unwrap()).unwrap();
    persisted.validate().unwrap();
    assert_eq!(persisted.receipts, candidate.receipts);
    let reads = observation.memory_reads.unwrap();
    assert_eq!(reads.len(), block_on);
    assert_eq!(reads.last().unwrap().outcome, MemoryReadOutcome::Failed);
    assert_eq!(
        reads
            .iter()
            .filter(|read| read.outcome == MemoryReadOutcome::Nonempty)
            .count(),
        block_on - 1
    );
    let trace = serde_json::to_value(traced.finish()).unwrap();
    assert_eq!(trace["calls"].as_array().unwrap().len(), block_on);
    assert_eq!(trace["calls"][block_on - 1]["call_failed"], true);
    let pid = fs::read_to_string(store.join("child.pid")).unwrap();
    assert!(
        !Command::new("/bin/kill")
            .args(["-0", pid.trim()])
            .output()
            .unwrap()
            .status
            .success(),
        "deadline must reap the owned child before returning"
    );
    assert_eq!(server.requests().len(), block_on);
    println!(
        "DEADLINE_RECEIPT {}",
        json!({"block_on_read":block_on,"phase_budget_ms":500,"tool_timeout_ms":2000,"phase_elapsed_ms":phase_elapsed_ms,"status":"timed_out","completed_reads_retained":block_on-1,"failed_reads":1,"owned_child_reaped":true})
    );
}
