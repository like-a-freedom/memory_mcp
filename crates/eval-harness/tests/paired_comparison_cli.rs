use std::collections::{BTreeMap, BTreeSet};
use std::process::Command;

use eval_harness::paired::{
    AgentTraceEvent, BootstrapConfig, CaseTrialKey, DevelopmentPilotReport, ExploratoryPilotReport,
    ModelIdentityProtocol, PAIRED_ARTIFACT_SCHEMA_V2, PAIRED_PLAN_SCHEMA_V2, PairedEvaluationPlan,
    PairedEvaluationReport, PairedEvaluationStage, PairedRunArtifact, PairedRunComparison,
    PairedRunIdentity, ProductRunIdentity, PromotionCriteria, ResourceNonInferiority, RunArm,
    SharedRunIdentity, TraceEventKind, TrialReceipt, TrialStatus,
};
use eval_harness::paired_reliability::PairedReliabilityReport;

#[test]
fn compare_paired_cli_writes_a_reproducible_rejection_report() {
    let directory = tempfile::tempdir().expect("temporary paired comparison directory");
    let plan_path = directory.path().join("plan.json");
    let incumbent_path = directory.path().join("incumbent.json");
    let candidate_path = directory.path().join("candidate.json");
    let output_path = directory.path().join("comparison.json");

    let shared = shared_identity();
    let incumbent_product = product_identity("incumbent-revision", '1');
    let candidate_product = product_identity("candidate-revision", '2');
    let expected_trials = trials();
    let plan = PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "cli-regression-v1".into(),
        stage: PairedEvaluationStage::Confirmatory,
        pilot_report_sha256: Some(digest('3')),
        power_analysis_sha256: Some(digest('4')),
        planned_independent_families: 2,
        expected_trials: expected_trials.clone(),
        shared_identity: shared.clone(),
        incumbent_product: incumbent_product.clone(),
        candidate_product: candidate_product.clone(),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 7,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        },
        promotion_criteria: Some(PromotionCriteria {
            direct_memory_margin: 0.05,
            task_success_margin: 0.05,
            repair_turn_gain_margin: 0.05,
            task_non_inferiority_by_stratum: BTreeMap::from([("ordinary".into(), 0.05)]),
            ordinary_workflow_strata: BTreeSet::from(["ordinary".into()]),
            resource_non_inferiority: ResourceNonInferiority {
                turns: 0.0,
                tool_calls: 0.0,
                user_clarifications: 0.0,
                input_tokens: 0.0,
                output_tokens: 0.0,
                wall_time_ms: 100.0,
            },
            resource_non_inferiority_by_stratum: BTreeMap::from([(
                "ordinary".into(),
                ResourceNonInferiority {
                    turns: 0.0,
                    tool_calls: 0.0,
                    user_clarifications: 0.0,
                    input_tokens: 0.0,
                    output_tokens: 0.0,
                    wall_time_ms: 0.0,
                },
            )]),
        }),
    };
    let incumbent = run_artifact(
        RunArm::Incumbent,
        shared.clone(),
        incumbent_product,
        expected_trials.clone(),
        false,
        true,
    );
    let candidate = run_artifact(
        RunArm::Candidate,
        shared,
        candidate_product,
        expected_trials,
        true,
        false,
    );
    write_json(&plan_path, &plan);
    write_json(&incumbent_path, &incumbent);
    write_json(&candidate_path, &candidate);

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "compare-paired",
            "--plan",
            plan_path.to_str().expect("UTF-8 plan path"),
            "--incumbent",
            incumbent_path.to_str().expect("UTF-8 incumbent path"),
            "--candidate",
            candidate_path.to_str().expect("UTF-8 candidate path"),
            "--output",
            output_path.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("run paired comparison command");

    assert_eq!(output.status.code(), Some(1));
    let report: PairedEvaluationReport = serde_json::from_slice(
        &std::fs::read(&output_path).expect("comparison report should be written on rejection"),
    )
    .expect("comparison report should be valid JSON");
    report.validate().expect("report should reproduce its gate");
    assert_eq!(
        report.promotion.decision,
        eval_harness::paired::PromotionDecision::Reject
    );
    assert_eq!(
        report.comparison.incumbent_artifact_sha256,
        incumbent.sha256().expect("hash incumbent run")
    );
    assert_eq!(
        report.comparison.candidate_artifact_sha256,
        candidate.sha256().expect("hash candidate run")
    );
    assert_eq!(
        report.comparison.overall.direct_memory_success.estimate,
        1.0
    );
    assert_eq!(report.comparison.overall.task_success.estimate, -1.0);
}

#[test]
fn summarize_pilot_cli_writes_family_level_variance_without_a_promotion_gate() {
    let (_directory, output, output_path) = run_pilot_summary(
        PairedEvaluationStage::DevelopmentPilot,
        "development-pilot-v1",
    );
    assert!(output.status.success());
    let report: DevelopmentPilotReport =
        serde_json::from_slice(&std::fs::read(&output_path).expect("pilot report is written"))
            .expect("pilot report is valid JSON");
    report
        .validate()
        .expect("pilot report reproduces from run summaries");
    assert_eq!(
        report.schema_version,
        eval_harness::paired::PAIRED_PILOT_REPORT_SCHEMA_V3
    );
    assert_eq!(report.independent_family_count, 2);
    assert_eq!(report.paired_trial_count_per_run, 2);
    assert_eq!(report.metrics.len(), 9);
}

#[test]
fn summarize_pilot_cli_reports_exploratory_arm_outcomes_without_a_promotion_gate() {
    let (_directory, output, output_path) = run_pilot_summary(
        PairedEvaluationStage::ExploratoryComparison,
        "exploratory-pilot-v1",
    );

    assert!(output.status.success());
    let report: ExploratoryPilotReport =
        serde_json::from_slice(&std::fs::read(&output_path).expect("pilot report is written"))
            .expect("exploratory pilot report is valid JSON");
    report
        .validate()
        .expect("exploratory pilot report reproduces from paired runs");
    assert_eq!(report.independent_family_count, 2);
    assert_eq!(report.incumbent_outcomes.len(), 9);
    assert_eq!(report.candidate_outcomes.len(), 9);
    assert_eq!(
        report.incumbent_outcomes[&eval_harness::paired::PilotOutcome::DirectMemorySuccess]
            .family_means["family-1"],
        1.0
    );
    assert_eq!(
        report.candidate_outcomes[&eval_harness::paired::PilotOutcome::DirectMemorySuccess]
            .family_means["family-1"],
        0.0
    );
    let mut tampered_report = report.clone();
    tampered_report
        .candidate_outcomes
        .get_mut(&eval_harness::paired::PilotOutcome::DirectMemorySuccess)
        .expect("candidate direct-memory outcomes are present")
        .family_means
        .insert("family-1".into(), 1.0);
    assert!(tampered_report.validate().is_err());
}

#[test]
fn plan_paired_power_cli_uses_family_discordance_from_validated_pilot_summary() {
    let (directory, summary_output, summary_path) = run_pilot_summary_with_candidate_task_outcomes(
        PairedEvaluationStage::ExploratoryComparison,
        "power-planning-pilot-v1",
        false,
    );
    assert!(summary_output.status.success());
    let output_path = directory.path().join("power-analysis.json");

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "plan-paired-power",
            "--pilot-report",
            summary_path.to_str().expect("UTF-8 pilot report path"),
            "--stratum-id",
            "ordinary",
            "--target-risk-difference",
            "0.25",
            "--minimum-point-effect",
            "0.10",
            "--one-sided-alpha",
            "0.025",
            "--target-power",
            "0.90",
            "--confidence-level",
            "0.95",
            "--maximum-sample-size",
            "200",
            "--output",
            output_path.to_str().expect("UTF-8 power report path"),
        ])
        .output()
        .expect("run paired power planner");

    assert!(output.status.success());
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&output_path).expect("power report is written"))
            .expect("power report is valid JSON");
    assert_eq!(
        report["schema_version"],
        "memory-mcp-pilot-power-analysis/v1"
    );
    assert_eq!(report["stratum_id"], "ordinary");
    assert_eq!(report["independent_family_count"], 2);
    assert_eq!(report["discordant_family_count"], 2);
    assert_eq!(
        report["discordant_family_ids"],
        serde_json::json!(["family-1", "family-2"])
    );
    assert!(report["power_plan"]["required_sample_size"].is_number());
    assert_eq!(report["promotion_decision"], "not_produced");
}

fn run_pilot_summary(
    stage: PairedEvaluationStage,
    plan_id: &str,
) -> (tempfile::TempDir, std::process::Output, std::path::PathBuf) {
    run_pilot_summary_with_candidate_task_outcomes(stage, plan_id, true)
}

fn run_pilot_summary_with_candidate_task_outcomes(
    stage: PairedEvaluationStage,
    plan_id: &str,
    second_candidate_task_success: bool,
) -> (tempfile::TempDir, std::process::Output, std::path::PathBuf) {
    let directory = tempfile::tempdir().expect("temporary paired pilot directory");
    let first_run_directory = directory.path().join("repeat-one");
    let second_run_directory = directory.path().join("repeat-two");
    std::fs::create_dir(&first_run_directory).expect("first pilot run directory");
    std::fs::create_dir(&second_run_directory).expect("second pilot run directory");
    let plan_path = directory.path().join("pilot-plan.json");
    let output_path = directory.path().join("pilot-report.json");
    let shared = shared_identity();
    let incumbent_product = product_identity("incumbent-revision", '1');
    let candidate_product = if stage == PairedEvaluationStage::ExploratoryComparison {
        product_identity("candidate-revision", '2')
    } else {
        incumbent_product.clone()
    };
    let expected_trials = trials();
    let plan = PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: plan_id.into(),
        stage,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        planned_independent_families: 2,
        expected_trials: expected_trials.clone(),
        shared_identity: shared.clone(),
        incumbent_product: incumbent_product.clone(),
        candidate_product: candidate_product.clone(),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 11,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        },
        promotion_criteria: None,
    };
    let mut first_incumbent = run_artifact(
        RunArm::Incumbent,
        shared.clone(),
        incumbent_product.clone(),
        expected_trials.clone(),
        true,
        true,
    );
    let mut first_candidate = run_artifact(
        RunArm::Candidate,
        shared.clone(),
        candidate_product.clone(),
        expected_trials.clone(),
        stage == PairedEvaluationStage::DevelopmentPilot,
        true,
    );
    let mut second_incumbent = run_artifact(
        RunArm::Incumbent,
        shared.clone(),
        incumbent_product,
        expected_trials.clone(),
        true,
        true,
    );
    let mut second_candidate = run_artifact(
        RunArm::Candidate,
        shared,
        candidate_product,
        expected_trials,
        stage == PairedEvaluationStage::DevelopmentPilot,
        second_candidate_task_success,
    );
    first_incumbent.run_id = Some("repeat-one".into());
    first_candidate.run_id = Some("repeat-one".into());
    second_incumbent.run_id = Some("repeat-two".into());
    second_candidate.run_id = Some("repeat-two".into());
    write_json(&plan_path, &plan);
    write_json(
        &first_run_directory.join("incumbent.json"),
        &first_incumbent,
    );
    write_json(
        &first_run_directory.join("candidate.json"),
        &first_candidate,
    );
    write_json(
        &second_run_directory.join("incumbent.json"),
        &second_incumbent,
    );
    write_json(
        &second_run_directory.join("candidate.json"),
        &second_candidate,
    );

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "summarize-pilot",
            "--plan",
            plan_path.to_str().expect("UTF-8 pilot plan path"),
            "--first-run-directory",
            first_run_directory
                .to_str()
                .expect("UTF-8 first repeat path"),
            "--second-run-directory",
            second_run_directory
                .to_str()
                .expect("UTF-8 second repeat path"),
            "--output",
            output_path.to_str().expect("UTF-8 pilot report path"),
        ])
        .output()
        .expect("run pilot summary command");

    (directory, output, output_path)
}

#[test]
fn paired_reliability_cli_counts_only_cases_that_pass_all_preregistered_repetitions() {
    let directory = tempfile::tempdir().expect("temporary paired reliability directory");
    let plan_path = directory.path().join("plan.json");
    let incumbent_path = directory.path().join("incumbent.json");
    let candidate_path = directory.path().join("candidate.json");
    let output_path = directory.path().join("reliability.json");
    let shared = shared_identity();
    let product = product_identity("same-product", '1');
    let expected_trials = trials()
        .into_iter()
        .flat_map(|first| {
            let mut second = first.clone();
            second.repetition = 2;
            [first, second]
        })
        .collect::<Vec<_>>();
    let plan = PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "reliability-contract-v1".into(),
        stage: PairedEvaluationStage::DevelopmentPilot,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        planned_independent_families: 2,
        expected_trials: expected_trials.clone(),
        shared_identity: shared.clone(),
        incumbent_product: product.clone(),
        candidate_product: product.clone(),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 13,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        },
        promotion_criteria: None,
    };
    let mut incumbent = run_artifact(
        RunArm::Incumbent,
        shared.clone(),
        product.clone(),
        expected_trials.clone(),
        true,
        true,
    );
    incumbent.run_id = Some("reliability-run".into());
    let mut candidate = run_artifact(
        RunArm::Candidate,
        shared,
        product,
        expected_trials,
        true,
        true,
    );
    candidate.run_id = Some("reliability-run".into());
    incumbent
        .receipts
        .iter_mut()
        .find(|receipt| receipt.key.family_id == "family-1" && receipt.key.repetition == 2)
        .expect("second attempt for the first case")
        .task_oracle_passed = false;
    write_json(&plan_path, &plan);
    write_json(&incumbent_path, &incumbent);
    write_json(&candidate_path, &candidate);

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .args([
            "summarize-paired-reliability",
            "--plan",
            plan_path.to_str().expect("UTF-8 plan path"),
            "--incumbent",
            incumbent_path.to_str().expect("UTF-8 incumbent path"),
            "--candidate",
            candidate_path.to_str().expect("UTF-8 candidate path"),
            "--output",
            output_path.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("run reliability summary command");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&output_path).expect("reliability report is written"),
    )
    .expect("reliability report is valid JSON");
    assert_eq!(report["schema_version"], "memory-mcp-agent-reliability/v1");
    assert_eq!(report["repetitions_per_case"], 2);
    assert_eq!(
        report["overall"]["direct_memory_success"]["incumbent"]["passed_cases"],
        2
    );
    assert_eq!(
        report["overall"]["direct_memory_success"]["pass_k_gain"],
        0.0
    );
    assert_eq!(
        report["overall"]["task_success"]["incumbent"]["passed_cases"],
        1
    );
    assert_eq!(
        report["overall"]["task_success"]["candidate"]["passed_cases"],
        2
    );
    assert_eq!(
        report["overall"]["task_success"]["candidate"]["case_count"],
        2
    );
    assert_eq!(report["overall"]["task_success"]["pass_k_gain"], 0.5);
    assert_eq!(
        report["by_stratum"]["ordinary"]["task_success"]["pass_k_gain"],
        0.5
    );
    let typed_report: PairedReliabilityReport =
        serde_json::from_value(report.clone()).expect("typed reliability report");
    typed_report
        .validate()
        .expect("report should reproduce from its case outcomes");
    let mut tampered_report = typed_report;
    tampered_report.overall.task_success.pass_k_gain = 0.0;
    assert!(tampered_report.validate().is_err());

    let repeated_output_path = directory.path().join("reliability-repeat.json");
    let repeated_output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .args([
            "summarize-paired-reliability",
            "--plan",
            plan_path.to_str().expect("UTF-8 plan path"),
            "--incumbent",
            incumbent_path.to_str().expect("UTF-8 incumbent path"),
            "--candidate",
            candidate_path.to_str().expect("UTF-8 candidate path"),
            "--output",
            repeated_output_path
                .to_str()
                .expect("UTF-8 repeated output path"),
        ])
        .output()
        .expect("repeat the deterministic reliability command");
    assert!(
        repeated_output.status.success(),
        "{}",
        String::from_utf8_lossy(&repeated_output.stderr)
    );
    assert_eq!(
        std::fs::read(&output_path).expect("first reliability report"),
        std::fs::read(repeated_output_path).expect("repeated reliability report")
    );

    let omitted_repetition = |key: &eval_harness::paired::CaseTrialKey| {
        key.family_id == "family-1" && key.repetition == 2
    };
    let mut incomplete_plan = plan;
    incomplete_plan
        .expected_trials
        .retain(|key| !omitted_repetition(key));
    let mut incomplete_incumbent = incumbent;
    incomplete_incumbent
        .expected_trials
        .retain(|key| !omitted_repetition(key));
    incomplete_incumbent
        .receipts
        .retain(|receipt| !omitted_repetition(&receipt.key));
    let mut incomplete_candidate = candidate;
    incomplete_candidate
        .expected_trials
        .retain(|key| !omitted_repetition(key));
    incomplete_candidate
        .receipts
        .retain(|receipt| !omitted_repetition(&receipt.key));
    let incomplete_plan_path = directory.path().join("incomplete-plan.json");
    let incomplete_incumbent_path = directory.path().join("incomplete-incumbent.json");
    let incomplete_candidate_path = directory.path().join("incomplete-candidate.json");
    let incomplete_output_path = directory.path().join("incomplete-report.json");
    write_json(&incomplete_plan_path, &incomplete_plan);
    write_json(&incomplete_incumbent_path, &incomplete_incumbent);
    write_json(&incomplete_candidate_path, &incomplete_candidate);
    let incomplete_output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .args([
            "summarize-paired-reliability",
            "--plan",
            incomplete_plan_path.to_str().expect("UTF-8 plan path"),
            "--incumbent",
            incomplete_incumbent_path
                .to_str()
                .expect("UTF-8 incumbent path"),
            "--candidate",
            incomplete_candidate_path
                .to_str()
                .expect("UTF-8 candidate path"),
            "--output",
            incomplete_output_path.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("reject a nonuniform repetition manifest");
    assert_eq!(incomplete_output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&incomplete_output.stderr).contains("same contiguous repetitions")
    );
    assert!(!incomplete_output_path.exists());
}

#[test]
fn exploratory_cli_compares_distinct_products_for_an_external_runner_without_a_gate() {
    use serde_json::json;

    let directory = tempfile::tempdir().expect("temporary exploratory comparison directory");
    let plan_path = directory.path().join("plan.json");
    let incumbent_path = directory.path().join("incumbent.json");
    let candidate_path = directory.path().join("candidate.json");
    let output_path = directory.path().join("diagnostic.json");

    let mut shared = shared_identity();
    shared.agent_runner = "third-party-harness".into();
    shared.provider_fingerprints = BTreeMap::from([
        ("external_model".into(), digest('9')),
        ("external_runner".into(), digest('a')),
    ]);
    let incumbent_product = product_identity("incumbent-revision", '1');
    let candidate_product = product_identity("candidate-revision", '2');
    let expected_trials = trials();
    let development_plan = PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "external-exploratory-v1".into(),
        stage: PairedEvaluationStage::DevelopmentPilot,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        planned_independent_families: 2,
        expected_trials: expected_trials.clone(),
        shared_identity: shared.clone(),
        incumbent_product: incumbent_product.clone(),
        candidate_product: incumbent_product.clone(),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 7,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        },
        promotion_criteria: None,
    };
    let mut plan = serde_json::to_value(development_plan).expect("serialize source plan");
    plan["plan_id"] = json!("external-exploratory-v1");
    plan["stage"] = json!("exploratory_comparison");
    plan["shared_identity"]["agent_runner"] = json!("third-party-harness");
    plan["shared_identity"]["provider_fingerprints"] =
        serde_json::to_value(&shared.provider_fingerprints).unwrap();
    plan["incumbent_product"] = serde_json::to_value(&incumbent_product).unwrap();
    plan["candidate_product"] = serde_json::to_value(&candidate_product).unwrap();

    let incumbent = run_artifact(
        RunArm::Incumbent,
        shared.clone(),
        incumbent_product,
        expected_trials.clone(),
        true,
        true,
    );
    let candidate = run_artifact(
        RunArm::Candidate,
        shared,
        candidate_product,
        expected_trials,
        true,
        false,
    );
    write_json(&plan_path, &plan);
    write_json(&incumbent_path, &incumbent);
    write_json(&candidate_path, &candidate);

    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .args([
            "compare-paired-diagnostic",
            "--plan",
            plan_path.to_str().expect("UTF-8 plan path"),
            "--incumbent",
            incumbent_path.to_str().expect("UTF-8 incumbent path"),
            "--candidate",
            candidate_path.to_str().expect("UTF-8 candidate path"),
            "--output",
            output_path.to_str().expect("UTF-8 output path"),
        ])
        .output()
        .expect("run external exploratory comparison command");

    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: PairedRunComparison = serde_json::from_slice(
        &std::fs::read(&output_path).expect("diagnostic comparison should be written"),
    )
    .expect("diagnostic output should be a comparison, not a promotion report");
    report
        .validate()
        .expect("comparison should be self-consistent");
    assert_eq!(
        report.plan.stage,
        PairedEvaluationStage::ExploratoryComparison
    );
    assert_eq!(
        report.plan.shared_identity.agent_runner,
        "third-party-harness"
    );
    assert_eq!(report.independent_family_count, 2);
    assert_eq!(report.overall.task_success.estimate, -1.0);
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(output_path).unwrap()).unwrap();
    assert!(json.get("promotion").is_none());
}

fn shared_identity() -> SharedRunIdentity {
    SharedRunIdentity {
        agent_runner: "fixed-fixture-runner".into(),
        agent_version: "1".into(),
        model: "fixture-model".into(),
        model_identity_protocol: ModelIdentityProtocol::ProviderRevision {
            revision: "fixture-model-revision".into(),
        },
        system_prompt_sha256: digest('a'),
        task_prompt_sha256: digest('b'),
        host_policy_sha256: digest('c'),
        transport: "stdio".into(),
        retry_policy_sha256: digest('d'),
        reader_config_sha256: digest('e'),
        judge_config_sha256: Some(digest('f')),
        corpus_sha256: digest('1'),
        oracle_sha256: digest('2'),
        clock_id: "fixed-clock-v1".into(),
        simulator_sha256: digest('5'),
        provider_fingerprints: BTreeMap::from([("fixture".into(), digest('6'))]),
        context_budget_tokens: 8_000,
        tool_call_budget: 10,
        turn_budget: 20,
        output_token_budget: 2_000,
        wall_time_budget_ms: 60_000,
        hardware_class: "test".into(),
    }
}

#[test]
fn scores_external_observations_offline_using_the_private_oracle() {
    use eval_harness::paired_agent::PairedTaskManifest;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    let directory = tempfile::tempdir().unwrap();
    let plan_path = directory.path().join("plan.json");
    let tasks_path = directory.path().join("tasks.json");
    let observations_path = directory.path().join("observations.json");
    let expected = trials();
    let manifest: PairedTaskManifest = serde_json::from_value(json!({
        "schema_version": "memory-mcp-agent-tasks/v2",
        "tasks": expected.iter().map(|key| json!({
            "key": key,
            "prompt": "Which room is assigned at revision {{KNOWN_REVISION}}?",
            "knowledge_before_event_id": "boundary",
            "seed_episodes": [{
                "event_id": "assignment",
                "source_id": "schedule",
                "source_type": "document",
                "content": "The room is Juniper.",
                "valid_at": fixed_run_start()
            }, {
                "event_id":"boundary",
                "source_id":"schedule",
                "source_type":"document",
                "content":"A later schedule was filed.",
                "valid_at":fixed_run_start()
            }],
            "oracle": {
                "accepted_answers": ["Juniper"],
                "forbidden_event_ids": [],
                "sufficient_evidence_alternatives": [[["assignment"]]],
                "needs_clarification": false,
                "read_expectation": {
                    "valid_at": "2026-05-01T11:00:00+01:00",
                    "required_claim_annotation_alternatives": [[{
                        "source_event_id":"assignment","evidence_status":"conflicting"
                    }]]
                }
            }
        })).collect::<Vec<_>>()
    }))
    .unwrap();
    let mut shared = shared_identity();
    shared.corpus_sha256 = manifest.corpus_sha256().unwrap();
    shared.oracle_sha256 = manifest.oracle_sha256().unwrap();
    let product = product_identity("frozen", '1');
    let plan = PairedEvaluationPlan {
        schema_version: PAIRED_PLAN_SCHEMA_V2.into(),
        plan_id: "external-observations".into(),
        stage: PairedEvaluationStage::DevelopmentPilot,
        pilot_report_sha256: None,
        power_analysis_sha256: None,
        planned_independent_families: 2,
        expected_trials: expected.clone(),
        shared_identity: shared.clone(),
        incumbent_product: product.clone(),
        candidate_product: product.clone(),
        declared_tool_schema_change: None,
        bootstrap: BootstrapConfig {
            seed: 7,
            draws: 1_000,
            confidence_level_basis_points: 9_500,
        },
        promotion_criteria: None,
    };
    let arm = |incumbent: bool, answer: &str| {
        let final_answer = json!({"answer": answer, "citations": ["episode-1"]}).to_string();
        let trace = json!([
            {"sequence": 1, "turn": 1, "kind": "agent_turn_started"},
            {"sequence": 2, "turn": 1, "kind": "memory_read", "call_id": "read", "tool_name": "assemble_context", "payload_sha256": digest('a')},
            {"sequence": 3, "turn": 1, "kind": "tool_response", "call_id": "read", "tool_name": "assemble_context", "payload_sha256": digest('b')},
            {"sequence": 4, "turn": 1, "kind": "final_answer", "payload_sha256": hex::encode(Sha256::digest(final_answer.as_bytes()))},
            {"sequence": 5, "turn": 1, "kind": "agent_turn", "input_tokens": 100, "output_tokens": 20}
        ]);
        json!({
            "identity": {"shared": shared, "product": product},
            "trials": expected.iter().map(|key| json!({
                "key": key,
                "arm_order": if incumbent == key.incumbent_first { 1 } else { 2 },
                "known_revision_checkpoint": 7,
                "episode_id_to_event_id": {"episode-1": "assignment", "episode-2": "boundary"},
                "observation": {
                    "trace": trace,
                    "final_answer": final_answer,
                    "memory_source_episode_ids": ["episode-1"],
                    "turns_used": 1,
                    "tool_calls": 1,
                    "user_clarifications": 0,
                    "input_tokens": 100,
                    "output_tokens": 20,
                    "wall_time_ms": 100,
                    "status": "completed"
                }
            })).collect::<Vec<_>>()
        })
    };
    let mut observations = json!({
        "schema_version": "memory-mcp-agent-observations/v1",
        "plan_sha256": plan.sha256().unwrap(),
        "run_id": "external-capture-1",
        "run_started_at": fixed_run_start(),
        "run_completed_at": fixed_run_start() + chrono::Duration::seconds(10),
        "incumbent": arm(true, "Juniper"),
        "candidate": arm(false, "Cedar")
    });
    write_json(&plan_path, &plan);
    write_json(&tasks_path, &manifest);
    write_json(&observations_path, &observations);
    let input_path = directory.path().join("runner-inputs.json");
    let export = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .arg("export-paired-inputs")
        .arg("--plan")
        .arg(&plan_path)
        .arg("--tasks")
        .arg(&tasks_path)
        .arg("--output")
        .arg(&input_path)
        .output()
        .unwrap();
    assert!(
        export.status.success(),
        "{}",
        String::from_utf8_lossy(&export.stderr)
    );
    let runner_inputs: serde_json::Value =
        serde_json::from_slice(&std::fs::read(input_path).unwrap()).unwrap();
    assert_eq!(runner_inputs["plan_sha256"], observations["plan_sha256"]);
    assert_eq!(
        runner_inputs["incumbent_identity"],
        observations["incumbent"]["identity"]
    );
    assert_eq!(
        runner_inputs["candidate_identity"],
        observations["candidate"]["identity"]
    );
    assert_eq!(
        runner_inputs["tasks"].as_array().unwrap().len(),
        expected.len()
    );
    assert!(
        runner_inputs["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .all(|task| {
                task.get("oracle").is_none()
                    && task["prompt"] == "Which room is assigned at revision {{KNOWN_REVISION}}?"
            })
    );
    assert!(!runner_inputs.to_string().contains("conflicting"));
    assert!(!runner_inputs.to_string().contains("read_expectation"));
    let score = |folder: &std::path::Path| {
        Command::new(env!("CARGO_BIN_EXE_memory-eval"))
            .env_clear()
            .arg("score-paired-observations")
            .arg("--plan")
            .arg(&plan_path)
            .arg("--tasks")
            .arg(&tasks_path)
            .arg("--observations")
            .arg(&observations_path)
            .arg("--output-directory")
            .arg(folder)
            .output()
            .unwrap()
    };
    let first = directory.path().join("first");
    let second = directory.path().join("second");
    for folder in [&first, &second] {
        let output = score(folder);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for (file, task_passed) in [("incumbent.json", true), ("candidate.json", false)] {
        let first_bytes = std::fs::read(first.join(file)).unwrap();
        assert_eq!(first_bytes, std::fs::read(second.join(file)).unwrap());
        let artifact: PairedRunArtifact = serde_json::from_slice(&first_bytes).unwrap();
        assert_eq!(artifact.run_id.as_deref(), Some("external-capture-1"));
        assert!(artifact.receipts.iter().all(|receipt| {
            receipt.direct_memory_success && receipt.task_oracle_passed == task_passed
        }));
        let legacy = String::from_utf8_lossy(&first_bytes);
        assert!(!legacy.contains("Juniper"));
        assert!(!legacy.contains("read_capture_status"));
        assert!(!legacy.contains("read_diagnostics"));
    }
    observations["schema_version"] = json!("memory-mcp-agent-observations/v2");
    write_json(&observations_path, &observations);
    let v2_run = directory.path().join("observations-v2");
    let output = score(&v2_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v2_artifact: PairedRunArtifact =
        serde_json::from_slice(&std::fs::read(v2_run.join("candidate.json")).unwrap()).unwrap();
    assert_eq!(v2_artifact.schema_version, "memory-mcp-agent-paired/v6");
    let valid_observations = observations.clone();
    observations["schema_version"] = json!("memory-mcp-agent-observations/v3");
    let failed_observation = &mut observations["incumbent"]["trials"][0]["observation"];
    failed_observation["trace"] = json!([
        {"sequence": 1, "turn": 1, "kind": "agent_turn_started"},
        {"sequence": 2, "turn": 1, "kind": "agent_turn", "input_tokens": 4, "output_tokens": 2},
        {"sequence": 3, "turn": 1, "kind": "model_failure", "error_class": "final_answer_not_json"}
    ]);
    failed_observation["final_answer"] = json!(null);
    failed_observation["memory_source_episode_ids"] = json!([]);
    failed_observation["turns_used"] = json!(1);
    failed_observation["tool_calls"] = json!(0);
    failed_observation["input_tokens"] = json!(4);
    failed_observation["output_tokens"] = json!(2);
    failed_observation["status"] = json!("model_failure");
    write_json(&observations_path, &observations);
    let v3_run = directory.path().join("observations-v3");
    let output = score(&v3_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v3_artifact: PairedRunArtifact =
        serde_json::from_slice(&std::fs::read(v3_run.join("incumbent.json")).unwrap()).unwrap();
    assert_eq!(v3_artifact.schema_version, "memory-mcp-agent-paired/v6");
    assert_eq!(v3_artifact.receipts[0].status, TrialStatus::ModelFailure);
    assert_eq!(
        serde_json::to_value(v3_artifact.receipts[0].trace.last().unwrap().error_class).unwrap(),
        json!("final_answer_not_json")
    );

    observations["schema_version"] = json!("memory-mcp-agent-observations/v2");
    write_json(&observations_path, &observations);
    let invalid_v2 = directory.path().join("invalid-v2-granular-diagnostic");
    let output = score(&invalid_v2);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("observations v2 cannot contain v3 final-answer diagnostics")
    );
    assert!(!invalid_v2.exists());

    let mut v4 = valid_observations.clone();
    v4["schema_version"] = json!("memory-mcp-agent-observations/v4");
    for arm in ["incumbent", "candidate"] {
        for trial in v4[arm]["trials"].as_array_mut().unwrap() {
            trial["observation"]["read_capture_status"] = json!("observed");
            trial["observation"]["memory_reads"] = json!([{
                "request_sequence":2,"response_sequence":3,"outcome":"nonempty","item_count":1
            }]);
        }
    }
    write_json(&observations_path, &v4);
    let v4_run = directory.path().join("v4-linked");
    let output = score(&v4_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v4_bytes = std::fs::read(v4_run.join("incumbent.json")).unwrap();
    let artifact: serde_json::Value = serde_json::from_slice(&v4_bytes).unwrap();
    assert_eq!(artifact["schema_version"], "memory-mcp-agent-paired/v7");
    let replay = directory.path().join("v4-replay");
    assert!(score(&replay).status.success());
    assert_eq!(
        v4_bytes,
        std::fs::read(replay.join("incumbent.json")).unwrap()
    );

    assert_eq!(artifact["receipts"][0]["read_capture_status"], "observed");
    assert_eq!(
        artifact["receipts"][0]["read_diagnostics"][0]["item_count"],
        1
    );
    let mut scored_read = v4.clone();
    scored_read["incumbent"]["trials"][0]["observation"]["memory_reads"][0]["evidence"] = json!({
        "source_episode_ids":["episode-1"],
        "claim_annotations":[],
        "completeness":"partial",
        "requested_cutoff_sha256": hex::encode(Sha256::digest(br#"["2026-05-01T10:00:00Z",7]"#)),
        "returned_cutoff_sha256": hex::encode(Sha256::digest(br#"["2026-05-01T10:00:00Z",8]"#))
    });
    let mut adequate =
        scored_read["incumbent"]["trials"][0]["observation"]["memory_reads"][0].clone();
    adequate["evidence"]["claim_annotations"] = json!([
        {"source_event_id":"episode-1", "evidence_status":"conflicting"}
    ]);
    adequate["evidence"]["returned_cutoff_sha256"] =
        adequate["evidence"]["requested_cutoff_sha256"].clone();
    scored_read["candidate"]["trials"][0]["observation"]["memory_reads"][0] = adequate.clone();

    let no_citations = &mut scored_read["incumbent"]["trials"][1]["observation"];
    let answer = json!({"answer":"Juniper", "citations":[]}).to_string();
    no_citations["trace"][3]["payload_sha256"] =
        json!(hex::encode(Sha256::digest(answer.as_bytes())));
    no_citations["final_answer"] = json!(answer);
    no_citations["memory_reads"][0] = adequate;

    let empty = &mut scored_read["candidate"]["trials"][1]["observation"];
    empty["memory_source_episode_ids"] = json!([]);
    empty["memory_reads"][0] = json!({
        "request_sequence":2,"response_sequence":3,"outcome":"empty","item_count":0,
        "evidence":{"source_episode_ids":[],"claim_annotations":[],"completeness":"partial"}
    });
    write_json(&observations_path, &scored_read);
    let scored_run = directory.path().join("v4-scored-read");
    let output = score(&scored_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let scored: serde_json::Value =
        serde_json::from_slice(&std::fs::read(scored_run.join("incumbent.json")).unwrap()).unwrap();
    let read = &scored["receipts"][0]["read_diagnostics"][0];
    assert_eq!(read["source_coverage"], true);
    assert_eq!(read["annotation_match"], false);
    assert_eq!(read["requested_cutoff_match"], true);
    assert_eq!(read["returned_cutoff_match"], false);
    assert_eq!(read["cited_returned_source_count"], 1);
    assert_eq!(scored["receipts"][0]["task_oracle_passed"], true);
    assert_eq!(read["completeness"], "partial");
    assert!(read.get("evidence").is_none());

    let candidate: serde_json::Value =
        serde_json::from_slice(&std::fs::read(scored_run.join("candidate.json")).unwrap()).unwrap();
    let wrong_answer = &candidate["receipts"][0];
    let read = &wrong_answer["read_diagnostics"][0];
    assert_eq!(read["source_coverage"], true);
    assert_eq!(read["annotation_match"], true);
    assert_eq!(read["returned_cutoff_match"], true);
    assert_eq!(read["cited_returned_source_count"], 1);
    assert_eq!(
        wrong_answer["task_oracle_diagnostics"]["accepted_answer"],
        false
    );
    assert_eq!(wrong_answer["task_oracle_passed"], false);
    let uncited = &scored["receipts"][1];
    assert_eq!(
        uncited["task_oracle_diagnostics"]["required_evidence_cited"],
        false
    );
    assert_eq!(
        uncited["read_diagnostics"][0]["cited_returned_source_count"],
        0
    );
    assert_eq!(uncited["task_oracle_passed"], false);
    let empty = &candidate["receipts"][1]["read_diagnostics"][0];
    assert_eq!(empty["outcome"], "empty");
    assert_eq!(empty["completeness"], "partial");
    assert_eq!(empty["source_coverage"], false);
    assert!(empty.get("returned_cutoff_match").is_none());

    for arm in ["incumbent", "candidate"] {
        let published = std::fs::read_to_string(scored_run.join(format!("{arm}.json"))).unwrap();
        for private in [
            "episode-1",
            "assignment",
            "Juniper",
            "requested_cutoff_sha256",
            "conflicting",
        ] {
            assert!(!published.contains(private), "receipt leaked {private}");
        }
    }

    let mut missing = v4.clone();
    missing["incumbent"]["trials"][0]["observation"]["memory_reads"] = json!([]);
    write_json(&observations_path, &missing);
    let missing_run = directory.path().join("v4-missing-read");
    assert_eq!(score(&missing_run).status.code(), Some(2));
    assert!(!missing_run.exists());

    missing["incumbent"]["trials"][0]["observation"]["read_capture_status"] = json!("unavailable");
    write_json(&observations_path, &missing);
    let unavailable_run = directory.path().join("v4-unavailable");
    assert!(score(&unavailable_run).status.success());
    let unavailable: serde_json::Value =
        serde_json::from_slice(&std::fs::read(unavailable_run.join("incumbent.json")).unwrap())
            .unwrap();
    assert_eq!(
        unavailable["receipts"][0]["read_capture_status"],
        "unavailable"
    );
    assert_eq!(unavailable["receipts"][0]["read_diagnostics"], json!([]));

    let mut retried = v4.clone();
    let retry = &mut retried["incumbent"]["trials"][0]["observation"];
    let trace = retry["trace"].as_array_mut().unwrap();
    let mut request = trace[1].clone();
    let mut response = trace[2].clone();
    request["call_id"] = json!("retry");
    response["call_id"] = json!("retry");
    trace[2]["kind"] = json!("tool_error");
    trace[2]["error_class"] = json!("unknown");
    trace.splice(3..3, [request, response]);
    for (index, event) in trace.iter_mut().enumerate() {
        event["sequence"] = json!(index + 1);
    }
    retry["tool_calls"] = json!(2);
    retry["memory_reads"] = json!([
        {"request_sequence":2,"response_sequence":3,"outcome":"failed","item_count":null},
        {"request_sequence":4,"response_sequence":5,"outcome":"nonempty","item_count":1}
    ]);
    write_json(&observations_path, &retried);
    let retry_run = directory.path().join("v4-retry");
    let output = score(&retry_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let retry_artifact: serde_json::Value =
        serde_json::from_slice(&std::fs::read(retry_run.join("incumbent.json")).unwrap()).unwrap();
    let receipt = &retry_artifact["receipts"][0];
    assert_eq!(receipt["task_oracle_passed"], true);
    assert_eq!(receipt["read_diagnostics"][0]["outcome"], "failed");
    assert_eq!(receipt["read_diagnostics"][1]["outcome"], "nonempty");

    let reads = &mut retried["incumbent"]["trials"][0]["observation"]["memory_reads"];
    reads[0]["response_sequence"] = json!(5);
    reads[1]["response_sequence"] = json!(3);
    write_json(&observations_path, &retried);
    let swapped_run = directory.path().join("v4-swapped");
    let output = score(&swapped_run);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("read/reply association"));
    assert!(!swapped_run.exists());

    v4["incumbent"]["trials"][0]["observation"]["trace"][2]["call_id"] = json!("different-read");
    write_json(&observations_path, &v4);
    let mismatch = directory.path().join("v4-mismatch");
    let output = score(&mismatch);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("read/reply association"));
    assert!(!mismatch.exists());

    observations = valid_observations;
    observations["schema_version"] = json!("memory-mcp-agent-observations/v1");
    let malformed = r#"{"answer":"Juniper","citations":["episode-1",42]}"#;
    observations["incumbent"]["trials"][0]["observation"]["final_answer"] = json!(malformed);
    observations["incumbent"]["trials"][0]["observation"]["trace"][3]["payload_sha256"] =
        json!(hex::encode(Sha256::digest(malformed.as_bytes())));
    write_json(&observations_path, &observations);
    let malformed_run = directory.path().join("malformed-answer");
    let output = score(&malformed_run);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: PairedRunArtifact =
        serde_json::from_slice(&std::fs::read(malformed_run.join("incumbent.json")).unwrap())
            .unwrap();
    assert!(artifact.receipts[0].direct_memory_success);
    assert!(!artifact.receipts[0].task_oracle_passed);

    observations["incumbent"]["trials"][0]["observation"]["final_answer"] =
        json!(r#"{"answer":"Changed","citations":["episode-1"]}"#);
    write_json(&observations_path, &observations);
    let rejected = directory.path().join("rejected");
    let output = score(&rejected);
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("final answer fingerprint"));
    assert!(!rejected.exists());

    observations["incumbent"]["trials"][0]["observation"]["trace"][0]["error_class"] =
        json!("invalid_final_answer");
    write_json(&observations_path, &observations);
    let invalid_legacy = directory.path().join("invalid-legacy-diagnostic");
    let output = score(&invalid_legacy);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(
            "observations v1 cannot contain model-response diagnostics from newer schemas"
        )
    );
    assert!(!invalid_legacy.exists());
}

fn product_identity(revision: &str, fingerprint: char) -> ProductRunIdentity {
    ProductRunIdentity {
        revision: revision.into(),
        binary_sha256: digest(fingerprint),
        product_config_sha256: digest('7'),
        tool_schema_sha256: digest('8'),
        enabled_features: BTreeSet::new(),
        claim_rollout_stage: None,
    }
}

fn trials() -> Vec<CaseTrialKey> {
    (1..=2)
        .map(|family| CaseTrialKey {
            stratum_id: "ordinary".into(),
            family_id: format!("family-{family}"),
            case_id: format!("case-{family}"),
            repetition: 1,
            incumbent_first: family % 2 == 1,
        })
        .collect()
}

fn run_artifact(
    arm: RunArm,
    shared: SharedRunIdentity,
    product: ProductRunIdentity,
    expected_trials: Vec<CaseTrialKey>,
    memory_success: bool,
    task_success: bool,
) -> PairedRunArtifact {
    let receipts = expected_trials
        .iter()
        .map(|key| TrialReceipt {
            read_capture_status: None,
            read_diagnostics: None,
            key: key.clone(),
            arm_order: if (arm == RunArm::Incumbent) == key.incumbent_first {
                1
            } else {
                2
            },
            known_revision_checkpoint: None,
            status: TrialStatus::Completed,
            direct_memory_success: memory_success,
            task_oracle_passed: task_success,
            task_oracle_diagnostics: None,
            turns_used: 1,
            tool_calls: 0,
            repair_turns: 0,
            user_clarifications: 0,
            input_tokens: 100,
            output_tokens: 20,
            wall_time_ms: 100,
            hard_violations: Vec::new(),
            trace: vec![
                AgentTraceEvent {
                    sequence: 1,
                    turn: 1,
                    kind: TraceEventKind::AgentTurnStarted,
                    call_id: None,
                    tool_name: None,
                    error_class: None,
                    payload_sha256: None,
                    input_tokens: None,
                    output_tokens: None,
                },
                AgentTraceEvent {
                    sequence: 2,
                    turn: 1,
                    kind: TraceEventKind::AgentTurn,
                    call_id: None,
                    tool_name: None,
                    error_class: None,
                    payload_sha256: None,
                    input_tokens: Some(100),
                    output_tokens: Some(20),
                },
                AgentTraceEvent {
                    sequence: 3,
                    turn: 1,
                    kind: TraceEventKind::FinalAnswer,
                    call_id: None,
                    tool_name: None,
                    error_class: None,
                    payload_sha256: Some(digest('9')),
                    input_tokens: None,
                    output_tokens: None,
                },
            ],
        })
        .collect();
    PairedRunArtifact {
        schema_version: PAIRED_ARTIFACT_SCHEMA_V2.into(),
        arm,
        run_id: None,
        run_started_at: fixed_run_start(),
        run_completed_at: fixed_run_start() + chrono::Duration::seconds(10),
        identity: PairedRunIdentity { shared, product },
        expected_trials,
        receipts,
    }
}

fn write_json(path: &std::path::Path, value: &impl serde::Serialize) {
    std::fs::write(
        path,
        serde_json::to_vec_pretty(value).expect("serialize test fixture"),
    )
    .expect("write test fixture");
}

fn fixed_run_start() -> chrono::DateTime<chrono::Utc> {
    "2026-11-21T00:00:00Z"
        .parse()
        .expect("parse fixed test time")
}

fn digest(value: char) -> String {
    std::iter::repeat_n(value, 64).collect()
}

#[test]
fn recall_capture_rejects_unregistered_read_mode() {
    let directory = tempfile::tempdir().unwrap();
    let output_path = directory.path().join("artifacts");
    let plan_path = directory.path().join("plan.json");
    let mut plan: serde_json::Value = serde_json::from_str(include_str!(
        "../../../evals/paired/source-authority-local-chat-completions-v4-qualification/plan.json"
    ))
    .unwrap();
    plan["stage"] = serde_json::json!("exploratory_comparison");
    plan["shared_identity"]["tool_call_budget"] = serde_json::json!(1);
    write_json(&plan_path, &plan);
    let output = Command::new(env!("CARGO_BIN_EXE_memory-eval"))
        .env_clear()
        .args([
            "run-paired-chat-completions",
            "--recall-read-mode",
            "live",
            "--plan",
        ])
        .arg(plan_path)
        .args([
            "--tasks",
            "unopened.json",
            "--model-endpoint",
            "http://127.0.0.1:1",
            "--incumbent-memory-server",
            "unopened",
            "--candidate-memory-server",
            "unopened",
            "--output-directory",
        ])
        .arg(&output_path)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unregistered recall read mode"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!output_path.exists());
}
