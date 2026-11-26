#![cfg(unix)]
use eval_harness::paired::{PairedEvaluationPlan, RunArm};
use eval_harness::paired_agent::chat_completions::{
    ChatCompletionsSampling, endpoint_config_sha256,
};
use eval_harness::paired_agent::chat_completions_backend::{
    ChatCompletionsArmRuntime, ChatCompletionsRecallBackend, RecallCaptureMode, RecallRegistration,
};
use eval_harness::paired_agent::{PairedTaskInput, PairedTaskManifest, PairedTrialBackend};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    net::TcpListener,
    os::unix::fs::PermissionsExt,
    time::{Duration, Instant},
};
fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

#[test]
fn checkpoint_recall_reuses_seed_session_but_isolates_each_arm() {
    checkpoint_recall_scenario(false);
}

#[test]
fn paired_phase_deadline_returns_observation_after_killing_the_server() {
    checkpoint_recall_scenario(true);
}

fn checkpoint_recall_scenario(blocking: bool) {
    let directory = tempfile::tempdir().unwrap();
    let binary = directory.path().join("memory-fixture");
    let tools = json!([
        {"name":"ingest","inputSchema":{}}, {"name":"extract","inputSchema":{}},
        {"name":"assemble_context","inputSchema":{"type":"object","properties":{"query":{"type":"string"},"valid_at":{"type":"string"},"known_revision":{"type":"integer"},"budget":{"type":"integer"}}}}
    ]);
    let source = r##"#!/bin/sh
log="$(dirname "$0")/events"
n=0
trap 'printf "exit %s\n" "$$" >> "$log"' 0
while IFS= read -r line; do
 printf '%s\t%s\t%s\n' "$SURREALDB_DATA_DIR" "$$" "$line" >> "$log"
 id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
 [ -n "$id" ] || continue
 case "$line" in
 *'"method":"initialize"'*) result='{}' ;;
 *'"method":"tools/list"'*) result='{"tools":__TOOLS__}' ;;
 *'"name":"ingest"'*) n=$((n+1)); result="{\"structuredContent\":{\"result\":\"episode:$n\"}}" ;;
 *'"name":"extract"'*) result='{"structuredContent":{"result":{}}}' ;;
 *'"name":"assemble_context"'*) __BLOCK__ result='{"structuredContent":{"result":[{"source_episode":"episode:1","content":"Cabinet-16"}],"context":{"snapshot":{"valid_at":"2026-05-01T10:00:00Z","known_revision":3}}}}' ;;
 *) exit 2 ;;
 esac
 printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$result"
done
"##.replace("__TOOLS__", &tools.to_string()).replace("__BLOCK__", if blocking { r#"case "$line" in *'"query":""'*) ;; *) continue ;; esac;"# } else { "" });
    fs::write(&binary, source).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = format!("http://{}/v1", listener.local_addr().unwrap());
    let manifest: PairedTaskManifest = serde_json::from_str(include_str!(
        "../../../evals/paired/recall-read-local-screen-2026-11-24/tasks.json"
    ))
    .unwrap();
    let mut plan: PairedEvaluationPlan = serde_json::from_str(include_str!(
        "../../../evals/paired/recall-read-local-screen-2026-11-24/live-plan.json"
    ))
    .unwrap();
    if blocking {
        plan.shared_identity.wall_time_budget_ms = 500;
    }
    plan.shared_identity.provider_fingerprints.insert(
        "memory_eval_cli".into(),
        digest(&fs::read(std::env::current_exe().unwrap()).unwrap()),
    );
    plan.shared_identity.provider_fingerprints.insert(
        "chat_completions_endpoint_config".into(),
        endpoint_config_sha256(&endpoint).unwrap(),
    );
    plan.shared_identity.reader_config_sha256 = digest(
        &serde_json::to_vec(&(
            "recall-read/v1",
            RecallCaptureMode::Live,
            Vec::<Value>::new(),
        ))
        .unwrap(),
    );
    for product in [&mut plan.incumbent_product, &mut plan.candidate_product] {
        product.binary_sha256 = digest(&fs::read(&binary).unwrap());
        product.tool_schema_sha256 = digest(&serde_json::to_vec(&tools).unwrap());
    }
    let registration = RecallRegistration::new(RecallCaptureMode::Live, None, &plan).unwrap();
    let (left, right) = plan.claim_rollout_stages(None).unwrap();
    let runtime = |stage: &str| ChatCompletionsArmRuntime {
        memory_server_binary: binary.clone(),
        endpoint: endpoint.clone(),
        bearer_token: None,
        temp_root: Some(directory.path().into()),
        claim_rollout_stage: stage.into(),
    };
    let trace_directory = directory.path().join("traces");
    let mut backend = ChatCompletionsRecallBackend::new(
        &plan,
        &manifest,
        registration,
        runtime(&left),
        runtime(&right),
        ChatCompletionsSampling::default(),
        None,
        None,
    )
    .unwrap()
    .with_public_synthetic_traces(trace_directory.clone());
    listener.set_nonblocking(true).unwrap();
    let model = plan.shared_identity.model.clone();
    let worker = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(15);
        for turn in 0..if blocking { 2 } else { 4 } {
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(error)
                        if error.kind() == std::io::ErrorKind::WouldBlock
                            && Instant::now() < deadline =>
                    {
                        std::thread::yield_now()
                    }
                    Err(error) => panic!("controlled model request deadline: {error}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut headers = Vec::new();
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
            let message = if blocking || turn % 2 == 0 {
                json!({"role":"assistant","content":null,"tool_calls":[{"id":"read-1","type":"function","function":{"name":"assemble_context","arguments":"{\"query\":\"Cedar calibration key\",\"valid_at\":\"2026-05-01T10:00:00Z\",\"known_revision\":3,\"budget\":1}"}}]})
            } else {
                json!({"role":"assistant","content":"{\"answer\":\"Cabinet-16\",\"citations\":[\"episode:1\"],\"needs_clarification\":false}"})
            };
            let response=json!({"model":model,"choices":[{"message":message}],"usage":{"prompt_tokens":20,"completion_tokens":10}}).to_string();
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",response.len(),response).unwrap();
        }
    });
    let task = &manifest.tasks[2];
    let prepared = backend
        .prepare(&PairedTaskInput {
            key: task.key.clone(),
            prompt: task.prompt.clone(),
            knowledge_before_event_id: task.knowledge_before_event_id.clone(),
            seed_episodes: task.seed_episodes.clone(),
        })
        .unwrap();
    assert_eq!(prepared.expected_known_revision, Some(3));
    assert!(!prepared.input.prompt.contains("{{KNOWN_REVISION}}"));
    let first = backend.execute(RunArm::Incumbent, &prepared).unwrap();
    let second = backend.execute(RunArm::Candidate, &prepared).unwrap();
    worker.join().unwrap();
    assert_eq!(
        first
            .episode_id_to_event_id
            .get("episode:1")
            .map(String::as_str),
        Some("ledger")
    );
    assert_eq!(second.episode_id_to_event_id, first.episode_id_to_event_id);
    assert_eq!(first.observation.tool_calls, 1);
    assert_eq!(second.observation.tool_calls, 1);
    let events = fs::read_to_string(directory.path().join("events")).unwrap();
    let mut stores: BTreeMap<&str, Vec<(&str, Value)>> = BTreeMap::new();
    for line in events.lines().filter(|line| !line.starts_with("exit ")) {
        let mut fields = line.splitn(3, '\t');
        let store = fields.next().unwrap();
        let pid = fields.next().unwrap();
        let call = serde_json::from_str(fields.next().unwrap()).unwrap();
        stores.entry(store).or_default().push((pid, call));
    }
    assert_eq!(
        stores.len(),
        3,
        "checkpoint preparation plus one isolated store per arm"
    );
    assert!(
        stores.values().all(|calls| calls
            .iter()
            .filter(|(_, call)| call["method"] == "initialize")
            .count()
            == 1),
        "each fresh recall store must use its seeded process, not reopen another process"
    );
    if blocking {
        use eval_harness::paired::{NormalizedErrorClass, TrialStatus};
        assert_eq!(first.observation.status, TrialStatus::TimedOut);
        assert_eq!(second.observation.status, TrialStatus::TimedOut);
        assert_eq!(
            first.observation.trace.last().unwrap().error_class,
            Some(NormalizedErrorClass::Timeout)
        );
        assert_eq!(
            second.observation.trace.last().unwrap().error_class,
            Some(NormalizedErrorClass::Timeout)
        );
        assert!(first.observation.wall_time_ms < 1500);
        assert!(second.observation.wall_time_ms < 1500);
        let traces: Vec<Value> = fs::read_dir(trace_directory)
            .unwrap()
            .map(|entry| serde_json::from_slice(&fs::read(entry.unwrap().path()).unwrap()).unwrap())
            .collect();
        assert_eq!(traces.len(), 2);
        assert!(
            traces
                .iter()
                .all(|trace| trace["calls"][0]["call_failed"] == true)
        );
        assert!(
            stores
                .values()
                .all(|calls| !std::process::Command::new("/bin/kill")
                    .args(["-0", calls[0].0])
                    .output()
                    .unwrap()
                    .status
                    .success()),
            "all owned processes must be reaped"
        );
    } else {
        assert_eq!(
            events
                .lines()
                .filter(|line| line.starts_with("exit "))
                .count(),
            3,
            "all owned sessions must exit before trial completion"
        );
    }
}
