use eval_harness::perf::remote_evidence::{
    QualityCaseEvidence, RemoteEvidence, RemoteEvidenceSchema, RemoteProcessSnapshot,
    RemoteResourceSample, RemoteScenarioEvidence, RemoteScenarioManifest, read,
    validate_remote_evidence, write,
};
use std::collections::BTreeMap;

fn scenario() -> RemoteScenarioEvidence {
    RemoteScenarioEvidence {
        scenario_id: "recall/2048/warm".to_owned(),
        dimension: Some(2048),
        cache_mode: "warm".to_owned(),
        successful_samples: 3,
        measured_duration_ms: 3,
        resource_samples: vec![resource_sample()],
        sql_count: Some(100),
        rpc_count: Some(110),
        session_attach_count: Some(3),
        session_create_count: Some(1),
        session_reuse_count: Some(2),
        session_eviction_count: Some(0),
        signin_count: Some(1),
        latency_us: vec![100, 120, 150],
        throughput_ops_per_sec: Some(1000.0),
        quality_cases: BTreeMap::from([(
            "case-a".to_owned(),
            QualityCaseEvidence {
                retrieval_limit: 1,
                relevant_ids: vec!["fact:a".to_owned()],
                retrieved_ids: vec!["fact:a".to_owned()],
                recall: 1.0,
                precision: 1.0,
                mrr: 1.0,
                ndcg: 1.0,
                answer_support: 1.0,
            },
        )]),
    }
}

fn resource_sample() -> RemoteResourceSample {
    let snapshot = |cpu_percent, rss_bytes| RemoteProcessSnapshot {
        cpu_percent: Some(cpu_percent),
        rss_bytes: Some(rss_bytes),
        peak_rss_bytes: Some(rss_bytes * 2),
        cgroup_memory_bytes: Some(rss_bytes * 3),
        swap_bytes: Some(0),
        physical_read_bytes: Some(4_096),
        physical_write_bytes: Some(8_192),
    };
    RemoteResourceSample {
        pair_id: "pair-1".to_owned(),
        interval_ms: 3,
        app: snapshot(25.0, 100_000_000),
        database: snapshot(40.0, 200_000_000),
    }
}

fn evidence() -> RemoteEvidence {
    let mut evidence = RemoteEvidence {
        schema: RemoteEvidenceSchema::V2,
        server_digest: "a".repeat(64),
        commit: "1".repeat(40),
        schema_digest: "b".repeat(64),
        model_digest: "c".repeat(64),
        corpus_digest: "d".repeat(64),
        scenario_manifest: Vec::new(),
        scenario_manifest_digest: String::new(),
        run_id: "run-1".to_owned(),
        started_at: "2026-10-14T10:00:00Z".to_owned(),
        as_of: "2026-10-14T00:00:00Z".to_owned(),
        native: false,
        scenarios: vec![scenario()],
    };
    refresh_manifest(&mut evidence);
    evidence
}

fn refresh_manifest(evidence: &mut RemoteEvidence) {
    evidence.scenario_manifest = evidence
        .scenarios
        .iter()
        .map(|scenario| RemoteScenarioManifest {
            scenario_id: scenario.scenario_id.clone(),
            dimension: scenario.dimension,
            cache_mode: scenario.cache_mode.clone(),
            quality_case_ids: scenario.quality_cases.keys().cloned().collect(),
        })
        .collect();
    evidence
        .scenario_manifest
        .sort_by(|left, right| left.scenario_id.cmp(&right.scenario_id));
    let encoded = serde_json::to_vec(&evidence.scenario_manifest).expect("manifest encodes");
    evidence.scenario_manifest_digest = eval_harness::perf::artifact::sha256_hex(&encoded);
}

fn evidence_pair() -> (RemoteEvidence, RemoteEvidence) {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.run_id = "run-2".to_owned();
    candidate.started_at = "2026-10-14T10:05:00Z".to_owned();
    (parent, candidate)
}

#[test]
fn remote_evidence_rejects_missing_db_cpu() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].resource_samples[0]
        .database
        .cpu_percent = None;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_cross_schema_comparison() {
    let (parent, mut candidate) = evidence_pair();
    candidate.schema_digest = "e".repeat(64);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_lost_quality_case() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].quality_cases.clear();

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_emulated_as_native() {
    let (mut parent, candidate) = evidence_pair();
    parent.native = true;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_missing_samples_and_resources() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].successful_samples = 0;
    candidate.scenarios[0].resource_samples[0].app.rss_bytes = None;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_missing_relevant_fact() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists")
        .retrieved_ids
        .clear();
    let candidate_case = candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists");
    candidate_case.recall = 0.0;
    candidate_case.precision = 0.0;
    candidate_case.mrr = 0.0;
    candidate_case.ndcg = 0.0;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_quality_scores_inconsistent_with_ranked_ids() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        run.scenarios[0]
            .quality_cases
            .get_mut("case-a")
            .expect("case-a fixture exists")
            .retrieved_ids
            .clear();
    }

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_zero_throughput_with_successful_samples() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].throughput_ops_per_sec = Some(0.0);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_throughput_inconsistent_with_duration() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].throughput_ops_per_sec = Some(500.0);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_accepts_unchanged_partial_recall() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        let case = run.scenarios[0]
            .quality_cases
            .get_mut("case-a")
            .expect("case-a fixture exists");
        case.relevant_ids = vec!["fact:a".to_owned(), "fact:b".to_owned()];
        case.retrieval_limit = 2;
        case.retrieved_ids = vec!["fact:a".to_owned()];
        case.recall = 0.5;
        case.precision = 1.0;
        case.ndcg = 0.613_147_192_765_458_4;
    }
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_ok());
}

#[test]
fn remote_evidence_rejects_replacing_a_relevant_result() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        let case = run.scenarios[0]
            .quality_cases
            .get_mut("case-a")
            .expect("case-a fixture exists");
        case.relevant_ids = vec!["fact:a".to_owned(), "fact:b".to_owned()];
        case.retrieval_limit = 2;
        case.recall = 0.5;
        case.ndcg = 0.613_147_192_765_458_4;
    }
    candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists")
        .retrieved_ids = vec!["fact:b".to_owned()];
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_accepts_complete_equivalent_pair() {
    let (parent, mut candidate) = evidence_pair();
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_ok());
}

#[test]
fn remote_evidence_rejects_reusing_one_run_as_both_sides_of_a_pair() {
    let run = evidence();

    assert!(validate_remote_evidence(&run, &run).is_err());
}

#[test]
fn remote_evidence_rejects_unknown_schema_version() {
    let mut encoded = serde_json::to_value(evidence()).expect("evidence serializes");
    encoded["schema"] = serde_json::json!("memory-mcp-surreal-query-evidence/v999");

    assert!(serde_json::from_value::<RemoteEvidence>(encoded).is_err());
}

#[test]
fn remote_evidence_rejects_incompatible_v1_schema() {
    let mut encoded = serde_json::to_value(evidence()).expect("evidence serializes");
    encoded["schema"] = serde_json::json!("memory-mcp-surreal-query-evidence/v1");

    assert!(serde_json::from_value::<RemoteEvidence>(encoded).is_err());
}

#[test]
fn remote_evidence_reports_unsupported_persisted_schema() {
    let directory = tempfile::tempdir().expect("temporary evidence directory");
    let path = directory.path().join("future-schema.json");
    let mut encoded = serde_json::to_value(evidence()).expect("evidence serializes");
    encoded["schema"] = serde_json::json!("memory-mcp-surreal-query-evidence/v999");
    std::fs::write(
        &path,
        serde_json::to_vec(&encoded).expect("evidence JSON encodes"),
    )
    .expect("future schema fixture writes");

    let error = read(&path).expect_err("unsupported schema is invalid");
    assert!(matches!(error, eval_harness::perf::PerfError::Schema(_)));
}

#[test]
fn remote_evidence_serializes_paired_resources_and_session_counters() {
    let encoded = serde_json::to_value(evidence()).expect("evidence serializes");
    let scenario = &encoded["scenarios"][0];

    assert_eq!(
        encoded["scenarioManifest"],
        serde_json::json!([{
            "scenarioId": "recall/2048/warm",
            "dimension": 2048,
            "cacheMode": "warm",
            "qualityCaseIds": ["case-a"]
        }])
    );
    assert!(scenario["resourceSamples"].is_array());
    assert!(scenario["sessionAttachCount"].is_u64());
    assert!(scenario["sessionCreateCount"].is_u64());
    assert!(scenario["sessionReuseCount"].is_u64());
    assert!(scenario["signinCount"].is_u64());
    assert_eq!(scenario["measuredDurationMs"].as_u64(), Some(3));
}

#[test]
fn remote_evidence_round_trips_through_the_persisted_artifact_contract() {
    let directory = tempfile::tempdir().expect("temporary evidence directory");
    let path = directory.path().join("baseline.json");
    let original = evidence();

    write(&original, &path).expect("valid evidence writes");

    assert_eq!(read(&path).expect("persisted evidence reads"), original);
}

#[test]
fn remote_evidence_rejects_a_manifest_digest_that_does_not_match_its_ids() {
    let directory = tempfile::tempdir().expect("temporary evidence directory");
    let path = directory.path().join("invalid.json");
    let mut invalid = evidence();
    invalid.scenario_manifest_digest = "f".repeat(64);

    assert!(write(&invalid, &path).is_err());
}

#[test]
fn remote_evidence_rejects_scenarios_missing_from_the_frozen_manifest() {
    let directory = tempfile::tempdir().expect("temporary evidence directory");
    let path = directory.path().join("incomplete.json");
    let mut incomplete = evidence();
    let mut omitted = scenario();
    omitted.scenario_id = "ingest/2048/warm".to_owned();
    incomplete.scenarios.push(omitted);

    assert!(write(&incomplete, &path).is_err());
}

#[test]
fn remote_evidence_rejects_unpaired_resource_windows() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].resource_samples[0].pair_id = "different-pair".to_owned();

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_changed_rank_order_even_when_scores_match() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        let case = run.scenarios[0]
            .quality_cases
            .get_mut("case-a")
            .expect("case-a fixture exists");
        case.retrieval_limit = 2;
        case.relevant_ids = vec!["fact:a".to_owned(), "fact:b".to_owned()];
        case.retrieved_ids = vec!["fact:a".to_owned(), "fact:b".to_owned()];
        case.recall = 1.0;
        case.precision = 1.0;
        case.mrr = 1.0;
        case.ndcg = 1.0;
    }
    candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists")
        .retrieved_ids
        .reverse();
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_missing_session_attach_counter() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0].session_attach_count = None;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_accepts_the_embeddings_disabled_profile() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        let scenario_id = "assemble/off/warm".to_owned();
        run.scenarios[0].scenario_id = scenario_id.clone();
        run.scenarios[0].dimension = None;
        refresh_manifest(run);
    }
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_ok());
}

#[test]
fn remote_evidence_accepts_idle_resource_windows_without_request_samples() {
    let (mut parent, mut candidate) = evidence_pair();
    for run in [&mut parent, &mut candidate] {
        let scenario = &mut run.scenarios[0];
        scenario.scenario_id = "idle/one-tenant".to_owned();
        scenario.dimension = None;
        scenario.successful_samples = 0;
        scenario.latency_us.clear();
        scenario.throughput_ops_per_sec = Some(0.0);
        scenario.sql_count = Some(0);
        scenario.rpc_count = Some(0);
        scenario.session_attach_count = Some(0);
        scenario.session_create_count = Some(0);
        scenario.session_reuse_count = Some(0);
        scenario.session_eviction_count = Some(0);
        scenario.signin_count = Some(0);
        scenario.quality_cases.clear();
        refresh_manifest(run);
    }

    assert!(validate_remote_evidence(&parent, &candidate).is_ok());
}

#[test]
fn remote_evidence_rejects_zero_embedding_dimension() {
    let (mut parent, mut candidate) = evidence_pair();
    parent.scenarios[0].dimension = Some(0);
    candidate.scenarios[0].dimension = Some(0);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_quality_limit_changes() {
    let (parent, mut candidate) = evidence_pair();
    candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists")
        .retrieval_limit = 2;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}
