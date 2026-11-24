use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use chrono::{DateTime, Utc};
use eval_harness::EvalError;
use eval_harness::independent_probe::{
    ProbeApiProfile, ProbeCapture, ProbeInput, ProbeObservation, ProbeOracle,
};
use eval_harness::paired::{
    AgentTraceEvent, BootstrapConfig, CaseTrialKey, ModelIdentityProtocol, PAIRED_PLAN_SCHEMA_V2,
    PairedEvaluationPlan, PairedEvaluationStage, ProductRunIdentity, RunArm, SharedRunIdentity,
    TraceEventKind,
};
use eval_harness::paired_agent::independent::{
    FixedTrialProbe, IndependentProbeBackend, run_with_independent_probes,
};
use eval_harness::paired_agent::{
    AgentObservation, HiddenTaskOracle, PAIRED_TASK_MANIFEST_SCHEMA_V2, PairedTaskDefinition,
    PairedTaskManifest, PairedTrialBackend, PairedTrialExecution, PreparedPairedTask, SeedEpisode,
};
use serde::{Deserialize, Serialize};
use sha2::Digest;

struct NoReadAgent;

impl PairedTrialBackend for NoReadAgent {
    fn execute(
        &mut self,
        _arm: RunArm,
        task: &PreparedPairedTask,
    ) -> Result<PairedTrialExecution, EvalError> {
        let answer = serde_json::json!({
            "answer": "Incorrect Depot",
            "citations": [],
            "needs_clarification": false,
        })
        .to_string();
        let observation = AgentObservation {
            trace: vec![
                event(1, TraceEventKind::AgentTurnStarted, None, None),
                AgentTraceEvent {
                    sequence: 2,
                    turn: 1,
                    kind: TraceEventKind::AgentTurn,
                    call_id: None,
                    tool_name: None,
                    error_class: None,
                    payload_sha256: None,
                    input_tokens: Some(12),
                    output_tokens: Some(6),
                },
                event(3, TraceEventKind::FinalAnswer, None, Some(digest('a'))),
            ],
            memory_reads: None,
            read_capture_status: None,
            final_answer: Some(answer),
            memory_source_episode_ids: Vec::new(),
            memory_write_episode_ids: Vec::new(),
            extract_fact_evidence: Vec::new(),
            extract_fact_evidence_telemetry_complete: false,
            terminal_http_status_code: None,
            terminal_transport_stage: None,
            turns_used: 1,
            tool_calls: 0,
            user_clarifications: 0,
            input_tokens: 12,
            output_tokens: 6,
            wall_time_ms: 100,
            status: eval_harness::paired::TrialStatus::Completed,
        };
        let episode_id_to_event_id = task
            .input
            .seed_episodes
            .iter()
            .map(|episode| {
                (
                    format!("episode:{}", episode.event_id),
                    episode.event_id.clone(),
                )
            })
            .collect();
        Ok(PairedTrialExecution {
            observation,
            episode_id_to_event_id,
        })
    }
}

struct PassingIndependentRead;

impl IndependentProbeBackend for PassingIndependentRead {
    fn capture(
        &mut self,
        _arm: RunArm,
        input: &ProbeInput,
        history: &[SeedEpisode],
    ) -> Result<ProbeCapture, EvalError> {
        let source = history[0].event_id.clone();
        let stage = input.claim_rollout_stage.clone();
        Ok(ProbeCapture {
            schema_version: "memory-mcp-independent-probe-capture/v1".into(),
            launch_profile: BTreeMap::from([
                ("NER_EXTRACTOR".into(), "regex".into()),
                ("EMBEDDINGS_ENABLED".into(), "false".into()),
                ("MEMORY_CLAIM_ROLLOUT_STAGE".into(), stage.clone()),
            ]),
            episode_id_to_event_id: BTreeMap::from([("episode:fixed".into(), source.clone())]),
            transport: "stdio".into(),
            extractor: "regex".into(),
            embeddings_enabled: false,
            observation: ProbeObservation {
                input_sha256: input.sha256()?,
                product_sha256: digest('5'),
                source_event_ids: vec![source],
                claim_annotations: Vec::new(),
                budget_used: 1,
                completed: true,
            },
            request: serde_json::json!({
                "query": input.query,
                "valid_at": input.valid_at.to_rfc3339(),
                "known_revision": input.known_revision,
                "budget": input.context_budget,
                "compact": true,
            }),
            response: serde_json::json!({
                "result": {
                    "structuredContent": {
                        "result": [{"source_episode": "episode:fixed"}],
                        "context": {"snapshot": {
                            "valid_at": input.valid_at,
                            "known_revision": input.known_revision,
                        }},
                    },
                },
            }),
            tool_schema_sha256: digest('6'),
            claim_rollout_stage: stage,
            api_profile: ProbeApiProfile::RevisionBoundV1,
        })
    }
}

#[derive(Serialize, Deserialize)]
struct LocalCriteria {
    ordinary_stratum_id: String,
    expected_trial_count: u64,
    minimum_task_successes: u64,
    minimum_direct_memory_successes: u64,
    minimum_probe_successes: u64,
}

#[test]
fn cli_blocks_confirmation_when_ordinary_workflow_fails_despite_passing_probes() {
    let directory = tempfile::tempdir().expect("qualification fixture directory");
    let manifest = fixture_manifest();
    let plan = fixture_plan(&manifest);
    let probes = fixture_probes(&manifest);
    let first_output = directory.path().join("first");
    let second_output = directory.path().join("second");
    std::fs::create_dir(&first_output).expect("first run directory");
    std::fs::create_dir(&second_output).expect("second run directory");

    let (first_incumbent, first_candidate) = run_with_independent_probes(
        &plan,
        &manifest,
        &probes,
        &mut NoReadAgent,
        &mut PassingIndependentRead,
        &first_output,
    )
    .expect("first paired capture");
    let (second_incumbent, second_candidate) = run_with_independent_probes(
        &plan,
        &manifest,
        &probes,
        &mut NoReadAgent,
        &mut PassingIndependentRead,
        &second_output,
    )
    .expect("second paired capture");
    assert_eq!(first_candidate.independent_memory.len(), 4);
    assert!(
        first_candidate
            .independent_memory
            .iter()
            .all(|receipt| receipt.score.evidence_success)
    );

    let plan_path = directory.path().join("plan.json");
    let tasks_path = directory.path().join("tasks.json");
    let probes_path = directory.path().join("probes.json");
    let criteria_path = directory.path().join("criteria.json");
    let first_incumbent_path = directory.path().join("first-incumbent.json");
    let first_candidate_path = directory.path().join("first-candidate.json");
    let second_incumbent_path = directory.path().join("second-incumbent.json");
    let second_candidate_path = directory.path().join("second-candidate.json");
    let output_path = directory.path().join("qualification.json");
    write_json(&plan_path, &plan);
    write_json(&tasks_path, &manifest);
    write_json(&probes_path, &probes);
    write_json(
        &criteria_path,
        &LocalCriteria {
            ordinary_stratum_id: "ordinary".into(),
            expected_trial_count: 4,
            minimum_task_successes: 3,
            minimum_direct_memory_successes: 3,
            minimum_probe_successes: 4,
        },
    );
    write_json(&first_incumbent_path, &first_incumbent);
    write_json(&first_candidate_path, &first_candidate);
    write_json(&second_incumbent_path, &second_incumbent);
    write_json(&second_candidate_path, &second_candidate);

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "qualify-agent-workflow",
            "--plan",
            path(&plan_path),
            "--tasks",
            path(&tasks_path),
            "--probes",
            path(&probes_path),
            "--criteria",
            path(&criteria_path),
            "--first-incumbent",
            path(&first_incumbent_path),
            "--first-candidate",
            path(&first_candidate_path),
            "--second-incumbent",
            path(&second_incumbent_path),
            "--second-candidate",
            path(&second_candidate_path),
            "--output",
            path(&output_path),
        ])
        .output()
        .expect("run qualification CLI");

    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("NO-GO"));
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_path).expect("report is written"))
            .expect("report is valid JSON");
    assert_eq!(report["eligible"], false);
    for capture in report["captures"].as_array().expect("two captures") {
        assert_eq!(capture["candidate_task_successes"], 0);
        assert_eq!(capture["candidate_task_trials"], 4);
        assert_eq!(capture["candidate_direct_memory_successes"], 0);
        assert_eq!(capture["candidate_independent_probe_successes"], 4);
        assert_eq!(capture["candidate_independent_probe_trials"], 4);
        assert_eq!(capture["candidate_hard_violations"], 0);
        assert!(
            capture["failed_gates"]
                .as_array()
                .expect("failed gates")
                .iter()
                .any(|gate| gate == "ordinary_task_success_floor")
        );
    }
}

fn fixture_manifest() -> PairedTaskManifest {
    let tasks = (0..4)
        .map(|index| {
            let letter = char::from(b'a' + u8::try_from(index).expect("four task indexes"));
            let event_id = format!("event-{letter}");
            PairedTaskDefinition {
                key: CaseTrialKey {
                    stratum_id: "ordinary".into(),
                    family_id: format!("family-{letter}"),
                    case_id: format!("case-{letter}"),
                    repetition: 1,
                    incumbent_first: index % 2 == 0,
                },
                prompt: format!("Where is the North Depot {letter} located?"),
                knowledge_before_event_id: None,
                seed_episodes: vec![SeedEpisode {
                    event_id: event_id.clone(),
                    source_id: format!("source-{event_id}"),
                    source_type: "document".into(),
                    content: format!("The North Depot {letter} is located in North City."),
                    valid_at: at("2026-05-01T10:00:00Z"),
                }],
                oracle: HiddenTaskOracle {
                    sufficient_evidence_alternatives: vec![vec![vec![event_id]]],
                    accepted_answers: vec!["North City".into()],
                    forbidden_event_ids: Vec::new(),
                    needs_clarification: false,
                    read_expectation: None,
                },
            }
        })
        .collect();
    PairedTaskManifest {
        schema_version: PAIRED_TASK_MANIFEST_SCHEMA_V2.into(),
        tasks,
    }
}

fn fixture_plan(manifest: &PairedTaskManifest) -> PairedEvaluationPlan {
    let product = ProductRunIdentity {
        revision: "fixture-product".into(),
        binary_sha256: digest('5'),
        product_config_sha256: digest('7'),
        tool_schema_sha256: digest('6'),
        enabled_features: BTreeSet::new(),
        claim_rollout_stage: Some("evidence".into()),
    };
    PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "agent-workflow-cli-gate-test".into(),
        stage: PairedEvaluationStage::DevelopmentPilot,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        expected_trials: manifest.tasks.iter().map(|task| task.key.clone()).collect(),
        planned_independent_families: 4,
        shared_identity: SharedRunIdentity {
            agent_runner: "scripted-test-runner".into(),
            agent_version: "1".into(),
            model: "fixed-fixture".into(),
            model_identity_protocol: ModelIdentityProtocol::ProviderRevision {
                revision: "fixed-fixture".into(),
            },
            system_prompt_sha256: digest('a'),
            task_prompt_sha256: digest('b'),
            host_policy_sha256: digest('c'),
            transport: "stdio".into(),
            retry_policy_sha256: digest('d'),
            reader_config_sha256: digest('e'),
            judge_config_sha256: None,
            corpus_sha256: manifest.corpus_sha256().expect("hash fixture corpus"),
            oracle_sha256: manifest.oracle_sha256().expect("hash fixture oracle"),
            clock_id: "fixed-clock".into(),
            simulator_sha256: digest('f'),
            provider_fingerprints: BTreeMap::from([("fixture".into(), digest('8'))]),
            context_budget_tokens: 4096,
            tool_call_budget: 12,
            turn_budget: 20,
            output_token_budget: 1024,
            wall_time_budget_ms: 60_000,
            hardware_class: "test".into(),
        },
        incumbent_product: product.clone(),
        candidate_product: product,
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 1,
            draws: 1000,
            confidence_level_basis_points: 9500,
        },
        promotion_criteria: None,
    }
}

fn fixture_probes(manifest: &PairedTaskManifest) -> Vec<FixedTrialProbe> {
    manifest
        .tasks
        .iter()
        .map(|task| {
            let episode = &task.seed_episodes[0];
            let input = ProbeInput {
                schema_version: "memory-mcp-independent-probe/v1".into(),
                query: task.prompt.clone(),
                principal: "fixture-reader".into(),
                valid_at: episode.valid_at,
                known_revision: 3,
                context_budget: 2,
                history_sha256: hex::encode(sha2::Sha256::digest(
                    serde_json::to_vec(&task.seed_episodes).expect("serialize history"),
                )),
                claim_rollout_stage: "evidence".into(),
            };
            FixedTrialProbe {
                key: task.key.clone(),
                input,
                launch_profile: BTreeMap::from([
                    ("NER_EXTRACTOR".into(), "regex".into()),
                    ("EMBEDDINGS_ENABLED".into(), "false".into()),
                    ("MEMORY_CLAIM_ROLLOUT_STAGE".into(), "evidence".into()),
                ]),
                oracle: ProbeOracle {
                    sufficient_evidence_alternatives: vec![vec![vec![episode.event_id.clone()]]],
                    forbidden_event_ids: Vec::new(),
                    required_claim_annotations: Vec::new(),
                    required_claim_annotation_alternatives: Vec::new(),
                },
            }
        })
        .collect()
}

fn event(
    sequence: u32,
    kind: TraceEventKind,
    call_id: Option<&str>,
    payload_sha256: Option<String>,
) -> AgentTraceEvent {
    AgentTraceEvent {
        sequence,
        turn: 1,
        kind,
        call_id: call_id.map(str::to_owned),
        tool_name: None,
        error_class: None,
        payload_sha256,
        input_tokens: None,
        output_tokens: None,
    }
}

fn at(value: &str) -> DateTime<Utc> {
    value.parse().expect("fixed RFC3339 timestamp")
}

fn digest(value: char) -> String {
    std::iter::repeat_n(value, 64).collect()
}

fn write_json(path: &std::path::Path, value: &impl Serialize) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("serialize fixture"),
    )
    .expect("write fixture");
}

fn path(path: &std::path::Path) -> &str {
    path.to_str().expect("UTF-8 temporary path")
}
