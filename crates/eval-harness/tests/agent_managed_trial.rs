use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use eval_harness::EvalError;
use eval_harness::paired::{
    AgentTraceEvent, BootstrapConfig, CaseTrialKey, ModelIdentityProtocol, NormalizedErrorClass,
    PairedEvaluationPlan, PairedEvaluationStage, ProductRunIdentity, RunArm, SharedRunIdentity,
    TraceEventKind, TrialStatus,
};
use eval_harness::paired_agent::HiddenTaskOracle;
use eval_harness::paired_agent::managed::{
    AGENT_MANAGED_TASK_SCHEMA_V1, AgentFactEvidenceAudit, AgentManagedOperatorFileImport,
    AgentManagedOperatorImportReceipt, AgentManagedProbeObservation, AgentManagedProbeRequest,
    AgentManagedTaskDefinition, AgentManagedTaskInput, AgentManagedTaskManifest,
    AgentManagedTrialBackend, compare_agent_managed_runs, run_agent_managed_paired_evaluation,
    run_agent_managed_trial,
};
use eval_harness::paired_agent::{
    AgentObservation, ExtractFactEvidenceObservation, TerminalTransportStage,
};

struct TrialState {
    written_episode_id: String,
    probe_succeeds: bool,
    probe_has_unmapped_source: bool,
    operator_imports: Vec<AgentManagedOperatorImportReceipt>,
}

struct ControlledBackend {
    calls: Vec<&'static str>,
    candidate_only_case_b_probes: bool,
    unmapped_candidate_case_b_probe: bool,
    fact_evidence_mode: bool,
    fact_evidence_requested: bool,
    fact_evidence_response_after_invalidation: bool,
    raw_tool_names: bool,
}

impl AgentManagedTrialBackend for ControlledBackend {
    type State = TrialState;

    fn start_trial(
        &mut self,
        arm: RunArm,
        task: &AgentManagedTaskInput,
    ) -> Result<(Self::State, BTreeMap<String, String>), EvalError> {
        self.calls.push("start");
        let case = task.key.case_id.as_str();
        let preloaded_episode_id = task
            .candidate_operator_imports
            .first()
            .filter(|_| arm == RunArm::Candidate)
            .map(|fixture| format!("episode:{}", fixture.event_id));
        let probe_succeeds = !self.candidate_only_case_b_probes
            || case == "case-a"
            || (arm == RunArm::Candidate && case == "case-b");
        let probe_has_unmapped_source =
            self.unmapped_candidate_case_b_probe && arm == RunArm::Candidate && case == "case-b";
        Ok((
            TrialState {
                written_episode_id: preloaded_episode_id
                    .unwrap_or_else(|| format!("episode:formed-{case}")),
                probe_succeeds,
                probe_has_unmapped_source,
                operator_imports: if arm == RunArm::Candidate {
                    task.candidate_operator_imports
                        .iter()
                        .map(|fixture| AgentManagedOperatorImportReceipt {
                            fixture_id: fixture.fixture_id.clone(),
                            import_succeeded: true,
                            extraction_attempted: true,
                            extraction_succeeded: true,
                            elapsed_ms: 12,
                        })
                        .collect()
                } else {
                    vec![]
                },
            },
            task.seed_episodes
                .iter()
                .map(|seed| (format!("episode:{}", seed.event_id), seed.event_id.clone()))
                .chain(if arm == RunArm::Candidate {
                    task.candidate_operator_imports
                        .iter()
                        .map(|fixture| {
                            (
                                format!("episode:{}", fixture.event_id),
                                fixture.event_id.clone(),
                            )
                        })
                        .collect::<Vec<_>>()
                } else {
                    vec![]
                })
                .collect(),
        ))
    }

    fn operator_import_receipts(
        &self,
        state: &Self::State,
    ) -> Vec<AgentManagedOperatorImportReceipt> {
        state.operator_imports.clone()
    }

    fn form_memory(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError> {
        let candidate_setup_prompt = prompt.starts_with("The operator imported");
        self.calls.push(if candidate_setup_prompt {
            "form_candidate"
        } else {
            "form"
        });
        let mut observation = observation(vec![]);
        if !candidate_setup_prompt {
            observation.memory_write_episode_ids = vec![state.written_episode_id.clone()];
        }
        if self.fact_evidence_mode {
            let extract_tool = if self.raw_tool_names {
                "extract"
            } else {
                "memory_decisions/extract"
            };
            let invalidate_tool = if self.raw_tool_names {
                "invalidate"
            } else {
                "memory_decisions/invalidate"
            };
            observation.tool_calls = 3;
            observation.extract_fact_evidence_telemetry_complete = true;
            observation.extract_fact_evidence = vec![ExtractFactEvidenceObservation {
                request_sequence: 3,
                response_sequence: Some(if self.fact_evidence_response_after_invalidation {
                    6
                } else {
                    4
                }),
                requested: self.fact_evidence_requested,
                returned: self.fact_evidence_requested,
                response_wire_bytes: Some(256),
            }];
            let extract_request = trace(
                3,
                TraceEventKind::MemoryWrite,
                Some("extract-call"),
                Some(extract_tool),
                Some("c"),
            );
            let extract_response = trace(
                if self.fact_evidence_response_after_invalidation {
                    6
                } else {
                    4
                },
                TraceEventKind::ToolResponse,
                Some("extract-call"),
                Some(extract_tool),
                Some("d"),
            );
            let invalidation_request = trace(
                if self.fact_evidence_response_after_invalidation {
                    4
                } else {
                    5
                },
                TraceEventKind::MemoryWrite,
                Some("invalidate-call"),
                Some(invalidate_tool),
                Some("e"),
            );
            let invalidation_response = trace(
                if self.fact_evidence_response_after_invalidation {
                    5
                } else {
                    6
                },
                TraceEventKind::ToolResponse,
                Some("invalidate-call"),
                Some(invalidate_tool),
                Some("f"),
            );
            let mut trace_events = vec![
                trace(
                    1,
                    TraceEventKind::MemoryRead,
                    Some("old-fact-read"),
                    Some("memory_decisions/assemble_context"),
                    Some("a"),
                ),
                trace(
                    2,
                    TraceEventKind::ToolResponse,
                    Some("old-fact-read"),
                    Some("memory_decisions/assemble_context"),
                    Some("b"),
                ),
            ];
            if self.fact_evidence_response_after_invalidation {
                trace_events.extend([
                    extract_request,
                    invalidation_request,
                    invalidation_response,
                    extract_response,
                ]);
            } else {
                trace_events.extend([
                    extract_request,
                    extract_response,
                    invalidation_request,
                    invalidation_response,
                ]);
            }
            observation.trace = trace_events;
        } else {
            let ingest_tool = if self.raw_tool_names {
                "ingest"
            } else {
                "memory_decisions/ingest"
            };
            observation.trace = vec![
                AgentTraceEvent {
                    sequence: 1,
                    turn: 1,
                    kind: TraceEventKind::MemoryWrite,
                    call_id: Some("ingest-call".into()),
                    tool_name: Some(ingest_tool.into()),
                    error_class: None,
                    payload_sha256: Some("a".repeat(64)),
                    input_tokens: None,
                    output_tokens: None,
                },
                AgentTraceEvent {
                    sequence: 2,
                    turn: 1,
                    kind: TraceEventKind::ToolResponse,
                    call_id: Some("ingest-call".into()),
                    tool_name: Some(ingest_tool.into()),
                    error_class: None,
                    payload_sha256: Some("b".repeat(64)),
                    input_tokens: None,
                    output_tokens: None,
                },
            ];
        }
        Ok(observation)
    }

    fn recall_in_fresh_session(
        &mut self,
        state: &mut Self::State,
        _prompt: &str,
    ) -> Result<AgentObservation, EvalError> {
        self.calls.push("fresh_recall");
        let mut recalled = observation(vec![state.written_episode_id.clone()]);
        recalled.final_answer = Some(
            serde_json::json!({
                "answer": "North City",
                "citations": [state.written_episode_id],
                "needs_clarification": false
            })
            .to_string(),
        );
        recalled.trace = vec![
            trace(1, TraceEventKind::AgentTurnStarted, None, None, None),
            AgentTraceEvent {
                sequence: 2,
                turn: 1,
                kind: TraceEventKind::AgentTurn,
                call_id: None,
                tool_name: None,
                error_class: None,
                payload_sha256: None,
                input_tokens: Some(1),
                output_tokens: Some(1),
            },
            trace(
                3,
                TraceEventKind::MemoryRead,
                Some("read-call"),
                Some("memory_decisions/assemble_context"),
                Some("c"),
            ),
            trace(
                4,
                TraceEventKind::ToolResponse,
                Some("read-call"),
                Some("memory_decisions/assemble_context"),
                Some("d"),
            ),
            trace(5, TraceEventKind::FinalAnswer, None, None, Some("e")),
        ];
        Ok(recalled)
    }

    fn independent_probe(
        &mut self,
        state: &mut Self::State,
        _probe: &AgentManagedProbeRequest,
    ) -> Result<AgentManagedProbeObservation, EvalError> {
        self.calls.push("probe");
        Ok(AgentManagedProbeObservation {
            source_episode_ids: if state.probe_has_unmapped_source {
                vec!["episode:unmapped".into()]
            } else if state.probe_succeeds {
                vec![state.written_episode_id.clone()]
            } else {
                vec![]
            },
            known_revision: Some(2),
            retained_store_bytes: 4096,
            request_sha256: "a".repeat(64),
            response_sha256: "b".repeat(64),
        })
    }
}

struct FormationFailureBackend {
    inner: ControlledBackend,
}

impl AgentManagedTrialBackend for FormationFailureBackend {
    type State = TrialState;

    fn start_trial(
        &mut self,
        arm: RunArm,
        task: &AgentManagedTaskInput,
    ) -> Result<(Self::State, BTreeMap<String, String>), EvalError> {
        self.inner.start_trial(arm, task)
    }

    fn operator_import_receipts(
        &self,
        state: &Self::State,
    ) -> Vec<AgentManagedOperatorImportReceipt> {
        self.inner.operator_import_receipts(state)
    }

    fn form_memory(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError> {
        if prompt.contains("(case-a)") || prompt.contains("(case-b)") {
            let is_http_rejection = prompt.contains("(case-a)");
            Ok(AgentObservation {
                trace: vec![AgentTraceEvent {
                    sequence: 1,
                    turn: 1,
                    kind: TraceEventKind::ModelFailure,
                    call_id: None,
                    tool_name: None,
                    error_class: Some(NormalizedErrorClass::Transport),
                    payload_sha256: None,
                    input_tokens: None,
                    output_tokens: None,
                }],
                final_answer: Some("PRIVATE_MODEL_RESPONSE_SENTINEL".into()),
                memory_source_episode_ids: vec![],
                memory_write_episode_ids: vec![],
                extract_fact_evidence: vec![],
                extract_fact_evidence_telemetry_complete: false,
                terminal_http_status_code: is_http_rejection.then_some(429),
                terminal_transport_stage: (!is_http_rejection)
                    .then_some(TerminalTransportStage::ResponseBodyRead),
                turns_used: 1,
                tool_calls: 0,
                user_clarifications: 0,
                input_tokens: 0,
                output_tokens: 0,
                wall_time_ms: 0,
                status: TrialStatus::ModelFailure,
            })
        } else {
            self.inner.form_memory(state, prompt)
        }
    }

    fn recall_in_fresh_session(
        &mut self,
        state: &mut Self::State,
        prompt: &str,
    ) -> Result<AgentObservation, EvalError> {
        self.inner.recall_in_fresh_session(state, prompt)
    }

    fn independent_probe(
        &mut self,
        state: &mut Self::State,
        probe: &AgentManagedProbeRequest,
    ) -> Result<AgentManagedProbeObservation, EvalError> {
        self.inner.independent_probe(state, probe)
    }
}

fn trace(
    sequence: u32,
    kind: TraceEventKind,
    call_id: Option<&str>,
    tool_name: Option<&str>,
    payload: Option<&str>,
) -> AgentTraceEvent {
    AgentTraceEvent {
        sequence,
        turn: 1,
        kind,
        call_id: call_id.map(str::to_owned),
        tool_name: tool_name.map(str::to_owned),
        error_class: None,
        payload_sha256: payload.map(|value| value.repeat(64)),
        input_tokens: None,
        output_tokens: None,
    }
}

fn observation(memory_source_episode_ids: Vec<String>) -> AgentObservation {
    AgentObservation {
        trace: vec![],
        final_answer: None,
        memory_source_episode_ids,
        memory_write_episode_ids: vec![],
        extract_fact_evidence: vec![],
        extract_fact_evidence_telemetry_complete: false,
        terminal_http_status_code: None,
        terminal_transport_stage: None,
        turns_used: 1,
        tool_calls: 1,
        user_clarifications: 0,
        input_tokens: 1,
        output_tokens: 1,
        wall_time_ms: 1,
        status: TrialStatus::Completed,
    }
}

fn managed_manifest() -> AgentManagedTaskManifest {
    let task = |case: &str, family: &str, incumbent_first| AgentManagedTaskDefinition {
        key: CaseTrialKey {
            stratum_id: "formation".into(),
            family_id: family.into(),
            case_id: case.into(),
            repetition: 1,
            incumbent_first,
        },
        formation_prompt: format!("Remember that the Cedar Depot is in North City ({case})."),
        candidate_formation_prompt: None,
        candidate_operator_imports: vec![],
        recall_prompt: format!("Where is the Cedar Depot in case {case}?"),
        independent_probe: AgentManagedProbeRequest {
            query: format!("What city is recorded for Cedar Depot case {case}?"),
            valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .expect("fixed time")
                .with_timezone(&chrono::Utc),
            context_budget: 4,
        },
        seed_episodes: vec![],
        target_write_event_id: format!("target-{case}"),
        oracle: HiddenTaskOracle {
            accepted_answers: vec!["North City".into()],
            sufficient_evidence_alternatives: vec![vec![vec![format!("target-{case}")]]],
            forbidden_event_ids: vec![],
            needs_clarification: false,
        },
    };
    AgentManagedTaskManifest {
        schema_version: AGENT_MANAGED_TASK_SCHEMA_V1.into(),
        tasks: vec![
            task("case-b", "family-b", false),
            task("case-a", "family-a", true),
        ],
    }
}

fn paired_plan(manifest: &AgentManagedTaskManifest) -> PairedEvaluationPlan {
    let shared_identity = SharedRunIdentity {
        agent_runner: "codex-cli".into(),
        agent_version: "0.171.0".into(),
        model: "gpt-6.1-sol".into(),
        model_identity_protocol: ModelIdentityProtocol::ProviderRevision {
            revision: "fixed-model-revision".into(),
        },
        system_prompt_sha256: eval_harness::codex_agent::agent_managed_system_prompt_sha256(),
        task_prompt_sha256: eval_harness::codex_agent::agent_managed_task_prompt_sha256(),
        host_policy_sha256: "a".repeat(64),
        transport: "stdio".into(),
        retry_policy_sha256: "b".repeat(64),
        reader_config_sha256: eval_harness::codex_agent::reader_config_sha256_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentManagedMemory,
        )
        .expect("managed profile has pinned protocol config"),
        judge_config_sha256: None,
        corpus_sha256: manifest.corpus_sha256().expect("hash task corpus"),
        oracle_sha256: manifest.oracle_sha256().expect("hash hidden oracle"),
        clock_id: "fixed-utc-clock".into(),
        simulator_sha256: "c".repeat(64),
        provider_fingerprints: BTreeMap::from([("codex_cli".into(), "d".repeat(64))]),
        context_budget_tokens: 4096,
        tool_call_budget: 12,
        turn_budget: 20,
        output_token_budget: 2048,
        wall_time_budget_ms: 120_000,
        hardware_class: "local-arm64".into(),
    };
    let product = |revision: &str, binary: char| ProductRunIdentity {
        revision: revision.into(),
        binary_sha256: binary.to_string().repeat(64),
        product_config_sha256: "e".repeat(64),
        tool_schema_sha256: "f".repeat(64),
        enabled_features: BTreeSet::new(),
        claim_rollout_stage: Some("evidence".into()),
    };
    PairedEvaluationPlan {
        schema_version: eval_harness::paired::PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "agent-managed-test-v1".into(),
        stage: PairedEvaluationStage::ExploratoryComparison,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        expected_trials: manifest.tasks.iter().map(|task| task.key.clone()).collect(),
        planned_independent_families: 2,
        shared_identity,
        incumbent_product: product("incumbent", '1'),
        candidate_product: product("candidate", '2'),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 42,
            draws: 1000,
            confidence_level_basis_points: 9500,
        },
        promotion_criteria: None,
    }
}

#[test]
fn maintenance_task_fingerprint_is_separate_from_formation_profile() {
    let manifest = managed_manifest();
    let formation = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentManagedMemory,
        )
        .expect("formation fingerprint");
    let maintenance = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenance,
        )
        .expect("maintenance fingerprint");
    let maintenance_v2 = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV2,
        )
        .expect("v2 maintenance fingerprint");
    let maintenance_v3 = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV3,
        )
        .expect("v3 maintenance fingerprint");
    let maintenance_v4 = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentMaintenanceV4,
        )
        .expect("v4 maintenance fingerprint");

    assert_eq!(formation.corpus_sha256, maintenance.corpus_sha256);
    assert_eq!(formation.oracle_sha256, maintenance.oracle_sha256);
    assert_ne!(
        formation.system_prompt_sha256,
        maintenance.system_prompt_sha256
    );
    assert_ne!(formation.task_prompt_sha256, maintenance.task_prompt_sha256);
    assert_ne!(formation.host_policy_sha256, maintenance.host_policy_sha256);
    assert_ne!(
        formation.protocol_config_sha256,
        maintenance.protocol_config_sha256
    );
    assert_ne!(formation.simulator_sha256, maintenance.simulator_sha256);
    assert_eq!(
        maintenance.system_prompt_sha256,
        maintenance_v2.system_prompt_sha256
    );
    assert_eq!(
        maintenance.task_prompt_sha256,
        maintenance_v2.task_prompt_sha256
    );
    assert_ne!(
        maintenance.protocol_config_sha256,
        maintenance_v2.protocol_config_sha256
    );
    assert_ne!(
        maintenance.host_policy_sha256,
        maintenance_v2.host_policy_sha256
    );
    assert_ne!(
        maintenance.simulator_sha256,
        maintenance_v2.simulator_sha256
    );
    assert_eq!(maintenance_v3.corpus_sha256, maintenance_v4.corpus_sha256);
    assert_eq!(maintenance_v3.oracle_sha256, maintenance_v4.oracle_sha256);
    assert_eq!(
        maintenance_v3.system_prompt_sha256,
        maintenance_v4.system_prompt_sha256
    );
    assert_ne!(
        maintenance_v3.task_prompt_sha256,
        maintenance_v4.task_prompt_sha256
    );
    assert_ne!(
        maintenance_v3.host_policy_sha256,
        maintenance_v4.host_policy_sha256
    );
    assert_ne!(
        maintenance_v3.protocol_config_sha256,
        maintenance_v4.protocol_config_sha256
    );
    assert_ne!(
        maintenance_v3.simulator_sha256,
        maintenance_v4.simulator_sha256
    );
}

#[test]
fn independent_post_write_probe_runs_after_formation_and_fresh_recall_in_same_trial_state() {
    let task = AgentManagedTaskInput {
        key: CaseTrialKey {
            stratum_id: "formation".into(),
            family_id: "family-1".into(),
            case_id: "case-1".into(),
            repetition: 1,
            incumbent_first: true,
        },
        formation_prompt: "Remember that the Cedar Depot is in North City.".into(),
        candidate_formation_prompt: None,
        candidate_operator_imports: vec![],
        recall_prompt: "Where is the Cedar Depot?".into(),
        seed_episodes: vec![],
    };
    let probe = AgentManagedProbeRequest {
        query: "What city is recorded for the Cedar Depot?".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("valid fixed time")
            .with_timezone(&chrono::Utc),
        context_budget: 4,
    };
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };

    let result = run_agent_managed_trial(&mut backend, RunArm::Incumbent, &task, &probe)
        .expect("complete all three stages");

    assert_eq!(backend.calls, ["start", "form", "fresh_recall", "probe"]);
    assert_eq!(
        result.formation.memory_write_episode_ids,
        ["episode:formed-case-1"]
    );
    assert_eq!(
        result.recall.memory_source_episode_ids,
        ["episode:formed-case-1"]
    );
    assert_eq!(
        result.independent_probe.source_episode_ids,
        ["episode:formed-case-1"]
    );
}

#[test]
fn candidate_operator_import_is_preloaded_and_counted_outside_agent_work() {
    let mut task = AgentManagedTaskInput {
        key: CaseTrialKey {
            stratum_id: "source-import".into(),
            family_id: "cedar-file".into(),
            case_id: "cedar-file-1".into(),
            repetition: 1,
            incumbent_first: true,
        },
        formation_prompt: "Read the supplied file into memory.".into(),
        candidate_formation_prompt: Some(
            "The operator imported the Cedar receipt; extract it.".into(),
        ),
        candidate_operator_imports: vec![AgentManagedOperatorFileImport {
            fixture_id: "cedar-receipt".into(),
            relative_path: "fixtures/cedar.md".into(),
            content: "The Cedar Depot is at 14 North Street.".into(),
            source_type: "note".into(),
            valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                .expect("fixed time")
                .with_timezone(&chrono::Utc),
            event_id: "target-cedar-file-1".into(),
        }],
        recall_prompt: "Where is the Cedar Depot?".into(),
        seed_episodes: vec![],
    };
    task.formation_prompt = "The incumbent must read fixtures/cedar.md into memory.".into();
    let probe = AgentManagedProbeRequest {
        query: "What address is recorded for Cedar Depot?".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc),
        context_budget: 4,
    };
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };

    let result = run_agent_managed_trial(&mut backend, RunArm::Candidate, &task, &probe)
        .expect("candidate imports the registered file before the agent session");

    assert_eq!(
        backend.calls,
        ["start", "form_candidate", "fresh_recall", "probe"]
    );
    assert_eq!(
        result.initial_episode_id_to_event_id,
        BTreeMap::from([(
            "episode:target-cedar-file-1".into(),
            "target-cedar-file-1".into()
        )])
    );
    assert_eq!(
        result.operator_imports,
        [AgentManagedOperatorImportReceipt {
            fixture_id: "cedar-receipt".into(),
            import_succeeded: true,
            extraction_attempted: true,
            extraction_succeeded: true,
            elapsed_ms: 12,
        }]
    );
}

#[test]
fn managed_paired_run_writes_compatible_artifacts_and_reports_pairwise_probe_outcomes() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: true,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };

    let (incumbent, candidate, incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("agent-managed paired run writes both artifact families");

    assert_eq!(incumbent.receipts.len(), 2);
    assert_eq!(candidate.receipts.len(), 2);
    assert!(directory.path().join("incumbent.json").is_file());
    assert!(directory.path().join("candidate.json").is_file());
    assert!(
        directory
            .path()
            .join("agent-managed-incumbent.json")
            .is_file()
    );
    assert!(
        directory
            .path()
            .join("agent-managed-candidate.json")
            .is_file()
    );
    assert!(incumbent_stages.stages.iter().all(|stage| {
        stage.successful_ingest_count == 1
            && stage.identified_ingest_episode_count == 1
            && stage.write_operations[0].attempts == 1
    }));

    let first = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("compare paired recall, formation and probe outcomes");
    let repeated = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("the comparison is a deterministic function of its artifacts");
    assert_eq!(first, repeated);
    assert_eq!(first.independent_probe_pairs.both_succeeded, 1);
    assert_eq!(first.independent_probe_pairs.candidate_only_succeeded, 1);
    assert_eq!(first.independent_probe_pairs.paired_trials, 2);
    let mut mispaired_sidecar = candidate_stages.clone();
    mispaired_sidecar.run_id = "different-run".into();
    assert!(
        compare_agent_managed_runs(
            &plan,
            &incumbent,
            &candidate,
            &incumbent_stages,
            &mispaired_sidecar,
        )
        .is_err()
    );

    let plan_path = directory.path().join("plan.json");
    let report_path = directory.path().join("comparison.json");
    std::fs::write(
        &plan_path,
        serde_json::to_vec_pretty(&plan).expect("serialize frozen plan"),
    )
    .expect("write plan");
    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "compare-agent-managed",
            "--plan",
            plan_path.to_str().expect("UTF-8 path"),
            "--incumbent",
            directory
                .path()
                .join("incumbent.json")
                .to_str()
                .expect("UTF-8 path"),
            "--candidate",
            directory
                .path()
                .join("candidate.json")
                .to_str()
                .expect("UTF-8 path"),
            "--incumbent-stages",
            directory
                .path()
                .join("agent-managed-incumbent.json")
                .to_str()
                .expect("UTF-8 path"),
            "--candidate-stages",
            directory
                .path()
                .join("agent-managed-candidate.json")
                .to_str()
                .expect("UTF-8 path"),
            "--output",
            report_path.to_str().expect("UTF-8 path"),
        ])
        .output()
        .expect("run agent-managed comparison CLI");
    assert!(
        output.status.success(),
        "comparison CLI failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let cli_report: eval_harness::paired_agent::managed::AgentManagedPairedComparison =
        serde_json::from_slice(&std::fs::read(report_path).expect("comparison report"))
            .expect("valid agent-managed comparison schema");
    assert_eq!(cli_report, first);
}

#[test]
fn managed_phase_artifact_retains_only_content_free_terminal_failure_diagnostics() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = FormationFailureBackend {
        inner: ControlledBackend {
            calls: vec![],
            candidate_only_case_b_probes: false,
            unmapped_candidate_case_b_probe: false,
            fact_evidence_mode: false,
            fact_evidence_requested: false,
            fact_evidence_response_after_invalidation: false,
            raw_tool_names: false,
        },
    };

    let (incumbent_run, candidate_run, incumbent, candidate) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("paired evaluation preserves normalized phase-failure diagnostics");

    for supplement in [&incumbent, &candidate] {
        let serialized = serde_json::to_value(supplement).expect("serialize stage sidecar");
        assert_eq!(
            serialized["schema_version"],
            "memory-mcp-agent-managed-run/v5"
        );
        let failure_class = serialized
            .pointer("/stages/0/formation/terminal_error_class")
            .expect("formation failure class is available in stage artifact");
        assert_eq!(failure_class, "transport");
        let stages = serialized["stages"].as_array().expect("stage list");
        assert!(stages.iter().any(|stage| {
            stage.pointer("/formation/terminal_http_status_code") == Some(&serde_json::json!(429))
        }));
        assert!(stages.iter().any(|stage| {
            stage.pointer("/formation/terminal_transport_stage")
                == Some(&serde_json::json!("response_body_read"))
        }));
        assert!(
            !serde_json::to_string(&serialized)
                .expect("serialize content-free stage artifact")
                .contains("PRIVATE_MODEL_RESPONSE_SENTINEL")
        );
    }

    let mut compatible_v4 = serde_json::to_value(&incumbent).expect("serialize v5 sidecar");
    compatible_v4["schema_version"] = serde_json::json!("memory-mcp-agent-managed-run/v4");
    for stage in compatible_v4["stages"].as_array_mut().expect("stage list") {
        stage["formation"]
            .as_object_mut()
            .expect("phase summary")
            .remove("terminal_transport_stage");
    }
    let compatible_v4: eval_harness::paired_agent::managed::AgentManagedRunSupplement =
        serde_json::from_value(compatible_v4).expect("v4 sidecar shape remains readable");
    compare_agent_managed_runs(
        &plan,
        &incumbent_run,
        &candidate_run,
        &compatible_v4,
        &candidate,
    )
    .expect("v4 artifacts remain valid alongside v5 artifacts");

    let mut compatible_v3 = serde_json::to_value(&compatible_v4).expect("serialize v4 sidecar");
    compatible_v3["schema_version"] = serde_json::json!("memory-mcp-agent-managed-run/v3");
    for stage in compatible_v3["stages"].as_array_mut().expect("stage list") {
        stage["formation"]
            .as_object_mut()
            .expect("phase summary")
            .remove("terminal_http_status_code");
    }
    let compatible_v3: eval_harness::paired_agent::managed::AgentManagedRunSupplement =
        serde_json::from_value(compatible_v3).expect("legacy v3 sidecar shape remains readable");
    compare_agent_managed_runs(
        &plan,
        &incumbent_run,
        &candidate_run,
        &compatible_v3,
        &candidate,
    )
    .expect("v3 artifacts remain valid alongside v4 artifacts");

    let mut mislabeled_v4 = serde_json::to_value(&incumbent).expect("serialize v5 sidecar");
    mislabeled_v4["schema_version"] = serde_json::json!("memory-mcp-agent-managed-run/v4");
    let mislabeled_v4: eval_harness::paired_agent::managed::AgentManagedRunSupplement =
        serde_json::from_value(mislabeled_v4)
            .expect("v5 fields remain structurally deserializable");
    assert!(
        compare_agent_managed_runs(
            &plan,
            &incumbent_run,
            &candidate_run,
            &mislabeled_v4,
            &candidate,
        )
        .is_err()
    );

    let mut conflicting_transport_diagnostics =
        serde_json::to_value(&incumbent).expect("serialize v5 sidecar");
    let rejected_phase = conflicting_transport_diagnostics["stages"]
        .as_array_mut()
        .expect("stage list")
        .iter_mut()
        .find(|stage| {
            stage.pointer("/formation/terminal_http_status_code") == Some(&serde_json::json!(429))
        })
        .expect("HTTP-rejected stage");
    rejected_phase["formation"]["terminal_transport_stage"] = serde_json::json!("request_send");
    let conflicting_transport_diagnostics:
        eval_harness::paired_agent::managed::AgentManagedRunSupplement =
        serde_json::from_value(conflicting_transport_diagnostics)
            .expect("v5 diagnostics remain structurally deserializable");
    assert!(
        compare_agent_managed_runs(
            &plan,
            &incumbent_run,
            &candidate_run,
            &conflicting_transport_diagnostics,
            &candidate,
        )
        .is_err()
    );
}

#[test]
fn managed_paired_run_tracks_unqualified_extract_and_invalidation_names() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: true,
        fact_evidence_requested: true,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: true,
    };

    let (_, _, incumbent_stages, _) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("public MCP tool names should produce valid managed receipts");

    assert!(incumbent_stages.stages.iter().all(|stage| {
        stage.successful_ingest_count == 1
            && stage.identified_ingest_episode_count == 1
            && stage
                .write_operations
                .iter()
                .any(|operation| operation.tool_name == "memory_decisions/extract")
            && stage.fact_evidence_audit.is_some_and(|audit| {
                audit.telemetry_trials == 1
                    && audit.extract_calls == 1
                    && audit.evidence_requests == 1
            })
    }));
}

#[test]
fn managed_paired_run_counts_unqualified_ingest_as_a_successful_write() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: true,
    };

    let (_, _, incumbent_stages, _) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("unqualified ingest names should produce valid managed receipts");

    assert!(incumbent_stages.stages.iter().all(|stage| {
        stage.successful_ingest_count == 1
            && stage.identified_ingest_episode_count == 1
            && stage.write_operations[0].tool_name == "memory_decisions/ingest"
    }));
}

#[test]
fn operator_file_import_uses_versioned_artifact_without_leaking_fixture_data() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let mut manifest = managed_manifest();
    manifest.schema_version = "memory-mcp-agent-managed-tasks/v2".into();
    let task = &mut manifest.tasks[0];
    task.candidate_formation_prompt = Some(
        "The operator imported and extracted the Cedar receipt; verify its location in memory."
            .into(),
    );
    task.candidate_operator_imports = vec![AgentManagedOperatorFileImport {
        fixture_id: "cedar-receipt".into(),
        relative_path: "fixtures/cedar.txt".into(),
        content: "Fact: The Cedar Depot is in North City.".into(),
        source_type: "doc".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc),
        event_id: task.target_write_event_id.clone(),
    }];
    manifest
        .validate_definition()
        .expect("v2 import manifest is valid");
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };

    let (incumbent, candidate, incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("operator setup should run as part of the candidate trial");
    let comparison = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("versioned operator receipts should compare with legacy receipts");

    assert_eq!(
        comparison.schema_version,
        "memory-mcp-agent-managed-comparison/v2"
    );
    assert_eq!(
        incumbent_stages.schema_version,
        "memory-mcp-agent-managed-run/v5"
    );
    assert_eq!(
        candidate_stages.schema_version,
        "memory-mcp-agent-managed-run/v5"
    );
    assert_eq!(comparison.candidate.operator_import_fixtures_total, 1);
    assert_eq!(comparison.candidate.operator_setup_actions_total, 2);
    assert_eq!(comparison.candidate.operator_setup_failures_total, 0);
    assert_eq!(comparison.candidate.independent_probe_successes, 2);
    let artifact = serde_json::to_string(&candidate_stages).expect("serialize candidate receipts");
    assert!(!artifact.contains("fixtures/cedar.txt"));
    assert!(!artifact.contains("Cedar Depot is in North City"));

    let mut legacy_incumbent = serde_json::to_value(&incumbent_stages).expect("serialize v3");
    legacy_incumbent["schema_version"] = serde_json::json!("memory-mcp-agent-managed-run/v1");
    let mut legacy_candidate = serde_json::to_value(&candidate_stages).expect("serialize v3");
    legacy_candidate["schema_version"] = serde_json::json!("memory-mcp-agent-managed-run/v2");
    for value in [&mut legacy_incumbent, &mut legacy_candidate] {
        for stage in value["stages"].as_array_mut().expect("stages array") {
            stage["formation"]
                .as_object_mut()
                .expect("formation object")
                .remove("terminal_error_class");
        }
    }
    let legacy_incumbent =
        serde_json::from_value(legacy_incumbent).expect("v1 artifacts remain readable");
    let legacy_candidate =
        serde_json::from_value(legacy_candidate).expect("v2 artifacts remain readable");
    compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &legacy_incumbent,
        &legacy_candidate,
    )
    .expect("legacy stage artifacts remain valid comparison inputs");
}

#[test]
fn operator_import_manifest_rejects_traversal_before_runner_setup() {
    let mut manifest = managed_manifest();
    manifest.schema_version = "memory-mcp-agent-managed-tasks/v2".into();
    let task = &mut manifest.tasks[0];
    task.candidate_formation_prompt = Some("Use the operator-prepared memory.".into());
    task.candidate_operator_imports = vec![AgentManagedOperatorFileImport {
        fixture_id: "cedar-receipt".into(),
        relative_path: "../outside.md".into(),
        content: "The Cedar Depot is in North City.".into(),
        source_type: "note".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc),
        event_id: task.target_write_event_id.clone(),
    }];

    assert!(manifest.validate_definition().is_err());
}

#[test]
fn source_authority_fingerprints_require_the_import_specific_workflow_profile() {
    let mut manifest = managed_manifest();
    manifest.schema_version = "memory-mcp-agent-managed-tasks/v2".into();
    let task = &mut manifest.tasks[0];
    task.candidate_formation_prompt = Some(
        "The operator imported and extracted the document. Use assemble_context to verify its fact; do not write.".into(),
    );
    task.candidate_operator_imports = vec![AgentManagedOperatorFileImport {
        fixture_id: "cedar-document".into(),
        relative_path: "fixtures/cedar.txt".into(),
        content: "Fact: The Cedar Depot is in North City.".into(),
        source_type: "doc".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc),
        event_id: task.target_write_event_id.clone(),
    }];

    assert!(
        manifest
            .validate_for_profile(
                eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority,
            )
            .is_ok()
    );
    let fingerprints = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority,
        )
        .expect("operator setup, arm-specific prompt rules and tool surfaces are fingerprinted");
    assert_eq!(fingerprints.task_count, 2);
    assert_eq!(fingerprints.independent_family_count, 2);
    assert_ne!(
        fingerprints.protocol_config_sha256,
        eval_harness::codex_agent::reader_config_sha256_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentManagedMemory,
        )
        .expect("standard managed protocol fingerprint")
    );
    let local_runner_fingerprints = manifest
        .fingerprints_for_chat_completions()
        .expect("loopback runner has independent host and retry policy identities");
    assert_eq!(
        local_runner_fingerprints.protocol_config_sha256,
        eval_harness::codex_agent::reader_config_sha256_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority,
        )
        .expect("legacy source-authority workflow has a registered protocol identity")
    );
    assert_eq!(
        local_runner_fingerprints.system_prompt_sha256,
        fingerprints.system_prompt_sha256
    );
    assert_eq!(
        local_runner_fingerprints.task_prompt_sha256,
        fingerprints.task_prompt_sha256
    );
    assert_ne!(
        local_runner_fingerprints.host_policy_sha256,
        fingerprints.host_policy_sha256
    );

    let mut literal_input_manifest = manifest.clone();
    literal_input_manifest.schema_version =
        eval_harness::paired_agent::managed::AGENT_MANAGED_TASK_SCHEMA_V3.into();
    let literal_input_fingerprints = literal_input_manifest
        .fingerprints_for_chat_completions()
        .expect("literal-input source authority has a registered provider-neutral protocol");
    assert_eq!(
        literal_input_fingerprints.protocol_config_sha256,
        eval_harness::codex_agent::reader_config_sha256_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthorityLiteralInput,
        )
        .expect("literal-input source authority has a registered protocol identity")
    );
    assert!(
        manifest
            .fingerprints_for_profile(
                eval_harness::codex_agent::CodexMemoryToolProfile::AgentManagedMemory,
            )
            .is_err(),
        "an import cohort must not be fingerprinted as the ordinary writer workflow"
    );

    let mut missing_import = manifest.clone();
    for task in &mut missing_import.tasks {
        task.candidate_operator_imports.clear();
    }
    assert!(
        missing_import
            .validate_for_profile(
                eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority,
            )
            .is_err(),
        "source-authority profile must not silently become an ordinary writer cohort"
    );
}

#[test]
fn literal_source_input_profile_has_its_own_protocol_identity() {
    let profile = eval_harness::codex_agent::CodexMemoryToolProfile::from_name(
        "agent_source_authority_literal_input",
    )
    .expect("literal source input profile is registered");
    let mut manifest = managed_manifest();
    manifest.schema_version = "memory-mcp-agent-managed-tasks/v2".into();
    let task = &mut manifest.tasks[0];
    task.formation_prompt =
        "Ingest the literal source content included here; do not pass a file path.".into();
    task.candidate_formation_prompt =
        Some("Verify the pre-imported source fact with read-only recall.".into());
    task.candidate_operator_imports = vec![AgentManagedOperatorFileImport {
        fixture_id: "literal-source".into(),
        relative_path: "fixtures/literal-source.txt".into(),
        content: "Fact: The Cedar Depot is in North City.".into(),
        source_type: "doc".into(),
        valid_at: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
            .expect("fixed time")
            .with_timezone(&chrono::Utc),
        event_id: task.target_write_event_id.clone(),
    }];

    let literal = manifest
        .fingerprints_for_profile(profile)
        .expect("literal-source workflow fingerprints");
    let legacy = manifest
        .fingerprints_for_profile(
            eval_harness::codex_agent::CodexMemoryToolProfile::AgentSourceAuthority,
        )
        .expect("legacy workflow fingerprints");

    assert_ne!(
        literal.protocol_config_sha256,
        legacy.protocol_config_sha256
    );
    assert_ne!(literal.simulator_sha256, legacy.simulator_sha256);
    assert_eq!(
        profile.formation_profile_for_arm(RunArm::Incumbent, true),
        Some(eval_harness::codex_agent::CodexMemoryToolProfile::AgentFormation)
    );
    assert_eq!(
        profile.formation_profile_for_arm(RunArm::Candidate, true),
        Some(eval_harness::codex_agent::CodexMemoryToolProfile::AgentOperatorPrepared)
    );
}

#[test]
fn managed_comparison_reports_fact_evidence_received_before_invalidation() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: true,
        fact_evidence_requested: true,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };
    let (incumbent, candidate, incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("agent-managed evidence run writes receipts");

    let report = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("compare normalized fact-evidence observations");

    assert_eq!(
        report.candidate.fact_evidence_audit,
        Some(AgentFactEvidenceAudit {
            telemetry_trials: 2,
            extract_calls: 2,
            evidence_requests: 2,
            evidence_responses: 2,
            evidence_before_invalidation: 2,
            response_sizes: Some(
                eval_harness::paired_agent::managed::AgentFactEvidenceSizeAudit {
                    measured_responses: 2,
                    total_wire_bytes: 512,
                    maximum_wire_bytes: 256,
                }
            ),
        })
    );
    let serialized = serde_json::to_string(&report).expect("report serializes");
    assert!(!serialized.contains("source_quote"));
    assert!(!serialized.contains("Cobalt uses Harbor-91."));
}

#[test]
fn managed_comparison_does_not_credit_evidence_returned_after_invalidation() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: true,
        fact_evidence_requested: true,
        fact_evidence_response_after_invalidation: true,
        raw_tool_names: false,
    };
    let (incumbent, candidate, incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("agent-managed evidence run writes receipts");

    let report = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("compare normalized fact-evidence observations");

    assert_eq!(
        report.candidate.fact_evidence_audit,
        Some(AgentFactEvidenceAudit {
            telemetry_trials: 2,
            extract_calls: 2,
            evidence_requests: 2,
            evidence_responses: 2,
            evidence_before_invalidation: 0,
            response_sizes: Some(
                eval_harness::paired_agent::managed::AgentFactEvidenceSizeAudit {
                    measured_responses: 2,
                    total_wire_bytes: 512,
                    maximum_wire_bytes: 256,
                }
            ),
        })
    );
}

#[test]
fn managed_comparison_distinguishes_measured_zero_from_missing_telemetry() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: true,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };
    let (incumbent, candidate, incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("agent-managed run writes receipts");

    let report = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("compare complete normalized telemetry");

    assert_eq!(
        report.candidate.fact_evidence_audit,
        Some(AgentFactEvidenceAudit {
            telemetry_trials: 2,
            extract_calls: 2,
            evidence_requests: 0,
            evidence_responses: 0,
            evidence_before_invalidation: 0,
            response_sizes: Some(
                eval_harness::paired_agent::managed::AgentFactEvidenceSizeAudit {
                    measured_responses: 2,
                    total_wire_bytes: 512,
                    maximum_wire_bytes: 256,
                }
            ),
        })
    );
}

#[test]
fn paired_run_retains_unmapped_independent_probe_sources_as_negative_evidence() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: true,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };

    let (_, _, _, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("unmapped sources are measured as unsupported evidence");

    let case_b = candidate_stages
        .stages
        .iter()
        .find(|stage| stage.key.case_id == "case-b")
        .expect("case-b probe receipt");
    assert_eq!(case_b.independent_probe.unsupported_source_count, 1);
    assert!(!case_b.independent_probe.evidence_success);
}

#[test]
fn managed_comparison_keeps_formation_budget_separate_from_recall_budget() {
    let directory = tempfile::tempdir().expect("paired output directory");
    let manifest = managed_manifest();
    let plan = paired_plan(&manifest);
    let mut backend = ControlledBackend {
        calls: vec![],
        candidate_only_case_b_probes: false,
        unmapped_candidate_case_b_probe: false,
        fact_evidence_mode: false,
        fact_evidence_requested: false,
        fact_evidence_response_after_invalidation: false,
        raw_tool_names: false,
    };
    let (incumbent, candidate, mut incumbent_stages, candidate_stages) =
        run_agent_managed_paired_evaluation(&plan, &manifest, &mut backend, directory.path())
            .expect("agent-managed run writes recall and formation artifacts");

    let exceeded_stage = incumbent_stages
        .stages
        .first_mut()
        .expect("incumbent has a formation stage");
    exceeded_stage.formation.tool_calls = plan.shared_identity.tool_call_budget + 1;
    exceeded_stage.formation.status = TrialStatus::BudgetExceeded;

    let comparison = compare_agent_managed_runs(
        &plan,
        &incumbent,
        &candidate,
        &incumbent_stages,
        &candidate_stages,
    )
    .expect("compare the distinct recall and formation phases");

    assert_eq!(comparison.candidate.fact_evidence_audit, None);
    assert_eq!(
        comparison
            .paired_agent_comparison
            .budget_exceeded_trial_count,
        0
    );
    assert_eq!(
        comparison
            .incumbent
            .formation_status_counts
            .get("budgetexceeded"),
        Some(&1)
    );
    assert_eq!(
        comparison
            .incumbent
            .formation_status_counts
            .get("completed"),
        Some(&1)
    );
    assert_eq!(
        comparison
            .candidate
            .formation_status_counts
            .get("completed"),
        Some(&2)
    );
}
