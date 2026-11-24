//! Runner-free verified acceptance through the shipped CLI.
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
fn sha(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}
const CONFIG: &str = "storage=embedded-surreal-local;namespace=main;embeddings=disabled;retrieval=lexical-fallback;ner=regex-builtin;primary_claim_rollout=shadow;evidence_diagnostic_claim_rollout=evidence;transaction_time=server-assigned";
fn event(id: &str) -> Value {
    json!({"id":id,"source_id":id,"source_type":"test","content":"correct","t_ref":"2026-07-01T09:00:00Z","correction_of":null})
}
fn oracle(id: &str) -> Value {
    json!({"probe_id":id,"disposition":"answerable","evidence_alternatives":[[[id]]],"expected_assertions":{id:"correct"},"expected_roles":{id:"current"},"forbidden_event_ids":[],"relation_outcome":null})
}
fn corpus(root: &std::path::Path) -> String {
    std::fs::create_dir(root).unwrap();
    let mut digest = Sha256::new();
    let files = [
        (
            "events.json",
            json!({"schema_version":"memory-decisions-events/v1","events":[event("target"),event("ordinary")]}),
        ),
        (
            "probes.json",
            json!({"schema_version":"memory-decisions-probes/v1","probes":[{"id":"target","query":"fixed","event_ids":["target"],"valid_at":"2026-07-01T09:00:00Z","budget":5},{"id":"ordinary","query":"fixed","event_ids":["ordinary"],"valid_at":"2026-07-01T09:00:00Z","budget":5}]}),
        ),
        (
            "oracle.json",
            json!({"schema_version":"memory-decisions-oracle/v1","oracles":[oracle("target"),oracle("ordinary")]}),
        ),
    ];
    for (name, data) in files {
        let bytes = serde_json::to_vec(&data).unwrap();
        std::fs::write(root.join(name), &bytes).unwrap();
        digest.update(name.as_bytes());
        digest.update([0]);
        digest.update((bytes.len() as u64).to_be_bytes());
        digest.update(&bytes);
    }
    hex::encode(digest.finalize())
}
fn replay(
    root: &std::path::Path,
    success: bool,
    product: &str,
    repeat: &str,
    corpus_sha: &str,
) -> Value {
    std::fs::create_dir(root).unwrap();
    let schema = sha(b"[]");
    let identity = json!({"suite_id":"memory-decisions","product_revision":product,"product_binary_sha256":product.repeat(64),"corpus_sha256":corpus_sha,"config_sha256":sha(CONFIG.as_bytes()),"input_schema_sha256":schema});
    let mut cases = vec![];
    for (id, passed) in [("target", success), ("ordinary", true)] {
        let text = if passed { "correct" } else { "wrong" };
        let observation = json!({"sources":[{"event_id":id,"text":text,"role":"current"}],"disposition":"supported","item_count":1,"budget_used":1,"relation_outcomes":[],"guidance":null});
        let score = json!({"evidence_success":passed,"decision_success":true,"probe_success":passed,"reason_codes":if passed {vec![]} else {vec!["missing_evidence","wrong_assertion"]}});
        let raw = json!({"schema_version":"memory-decisions-receipt/v3","case_id":id,"product_revision":product,"product_binary_sha256":product.repeat(64),"corpus_sha256":corpus_sha,"config_sha256":sha(CONFIG.as_bytes()),"repeat_nonce":repeat,"provider_configuration":CONFIG,"primary_claim_rollout_stage":"shadow","events":[event(id)],
            "supported_api":{"input_schema":[]},"desired_goal":{"valid_at":"2026-07-01T09:00:00Z","knowledge_before_event_id":null,"oracle":{"probe_id":id,"disposition":"answerable","evidence_alternatives":[[[id]]],"expected_assertions":{id:"correct"},"expected_roles":{id:"current"},"forbidden_event_ids":[],"relation_outcome":null}},
            "evidence_enabled_temporal_revision_score":score,
            "evidence_enabled_diagnostic":{"claim_rollout_stage":"evidence","episode_bindings":{"episode:fixed":id},
            "ingest_receipts":[{"event_id":id,"episode_id":"episode:fixed","request":{"source_type":"test","source_id":id,"content":"correct","t_ref":"2026-07-01T09:00:00+00:00"},"response":{"result":{"structuredContent":{"result":"episode:fixed"}}}}],
            "extract_receipts":[{"event_id":id,"request":{"episode_id":"episode:fixed"}}],
            "revision_checkpoint_receipts":[{"after_event_id":id,"request":{"query":"","valid_at":"2026-07-01T09:00:00+00:00","budget":1,"compact":true},"known_revision":1,"response":{"result":{"structuredContent":{"context":{"snapshot":{"valid_at":"2026-07-01T09:00:00Z","known_revision":1}}}}}}],
            "temporal_revision_request":{"query":"fixed","valid_at":"2026-07-01T09:00:00+00:00","known_revision":1,"budget":5,"compact":true},
            "temporal_revision_response":{"result":{"structuredContent":{"result":[{"source_episode":"episode:fixed","content":text,"evidence_status":"current"}],"context":{"snapshot":{"valid_at":"2026-07-01T09:00:00Z","known_revision":1}}}}}}});
        let bytes = serde_json::to_vec_pretty(&raw).unwrap();
        let filename = format!("receipt-{id}.json");
        std::fs::write(root.join(&filename), &bytes).unwrap();
        cases.push(json!({"case_id":id,"source_order":[id],"roles":["current"],"disposition":"supported","evidence_success":passed,"decision_success":true,"probe_success":passed,"evidence_enabled_temporal_observation":observation,"evidence_enabled_temporal_score":score,"reason_codes":[],"receipt_path":filename,"receipt_sha256":sha(&bytes)}));
    }
    json!({"schema_version":"memory-decisions-replay/v1","identity":identity,"cases":cases})
}
fn run(
    incumbent_success: bool,
    missing_observation: bool,
) -> (std::process::Output, Option<Value>) {
    let dir = tempfile::tempdir().unwrap();
    let corpus_root = dir.path().join("corpus");
    let corpus_sha = corpus(&corpus_root);
    let plan = json!({"schema_version":"memory-mcp-deterministic-acceptance/v1","corpus_sha256":corpus_sha,"config_sha256":sha(CONFIG.as_bytes()),"families":{"target":"f1","ordinary":"f2"},"ordinary_cases":["ordinary"],"minimum_gain_basis_points":2500,"maximum_items":5});
    std::fs::write(dir.path().join("plan"), serde_json::to_vec(&plan).unwrap()).unwrap();
    for (name, passed, product) in [
        ("a", incumbent_success, "a"),
        ("aa", incumbent_success, "a"),
        ("b", true, "b"),
        ("bb", true, "b"),
    ] {
        let root = dir.path().join(name);
        let mut index = replay(&root, passed, product, name, &corpus_sha);
        if missing_observation && product == "b" {
            index["cases"][0]["evidence_enabled_temporal_observation"] = Value::Null;
        }
        std::fs::write(root.join("index.json"), serde_json::to_vec(&index).unwrap()).unwrap();
    }
    let report = dir.path().join("report.json");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .arg("accept-memory-candidate")
        .arg("--corpus-root")
        .arg(&corpus_root)
        .arg("--plan")
        .arg(dir.path().join("plan"))
        .arg("--incumbent-first")
        .arg(dir.path().join("a/index.json"))
        .arg("--incumbent-second")
        .arg(dir.path().join("aa/index.json"))
        .arg("--candidate-first")
        .arg(dir.path().join("b/index.json"))
        .arg("--candidate-second")
        .arg(dir.path().join("bb/index.json"))
        .arg("--output")
        .arg(&report)
        .output()
        .unwrap();
    let value = std::fs::read(report)
        .ok()
        .map(|bytes| serde_json::from_slice(&bytes).unwrap());
    (output, value)
}
#[test]
fn accepts_improvement_without_agent_or_provider_environment() {
    let (out, report) = run(false, false);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(report.unwrap()["accepted"], true);
}
#[test]
fn unchanged_candidate_is_rejected_with_a_report() {
    let (out, report) = run(true, false);
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(report.unwrap()["accepted"], false);
}
#[test]
fn missing_independent_observation_is_invalid_despite_legacy_success() {
    let (out, report) = run(false, true);
    assert_eq!(out.status.code(), Some(2));
    assert!(report.is_none());
}
