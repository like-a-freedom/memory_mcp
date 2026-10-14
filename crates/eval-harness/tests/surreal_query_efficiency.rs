use eval_harness::perf::remote_evidence::{
    QualityCaseEvidence, RemoteEvidence, RemoteScenarioEvidence, validate_remote_evidence,
};
use std::collections::BTreeMap;

fn scenario() -> RemoteScenarioEvidence {
    RemoteScenarioEvidence {
        scenario_id: "recall/2048/warm".to_owned(),
        dimension: Some(2048),
        cache_mode: "warm".to_owned(),
        successful_samples: 3,
        app_cpu_percent: Some(25.0),
        db_cpu_percent: Some(40.0),
        app_rss_bytes: Some(100_000_000),
        db_rss_bytes: Some(200_000_000),
        sql_count: Some(100),
        rpc_count: Some(110),
        signin_count: Some(1),
        latency_us: vec![100, 120, 150],
        throughput_ops_per_sec: Some(1000.0),
        quality_cases: BTreeMap::from([(
            "case-a".to_owned(),
            QualityCaseEvidence {
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

fn evidence() -> RemoteEvidence {
    RemoteEvidence {
        server_digest: "a".repeat(64),
        commit: "1".repeat(40),
        schema_digest: "b".repeat(64),
        model_digest: "c".repeat(64),
        corpus_digest: "d".repeat(64),
        native: false,
        scenarios: vec![scenario()],
    }
}

#[test]
fn remote_evidence_rejects_missing_db_cpu() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.scenarios[0].db_cpu_percent = None;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_cross_schema_comparison() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.schema_digest = "e".repeat(64);

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_lost_quality_case() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.scenarios[0].quality_cases.clear();

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_emulated_as_native() {
    let mut parent = evidence();
    parent.native = true;
    let candidate = evidence();

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_missing_samples_and_resources() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.scenarios[0].successful_samples = 0;
    candidate.scenarios[0].app_rss_bytes = None;

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_rejects_missing_relevant_fact() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.scenarios[0]
        .quality_cases
        .get_mut("case-a")
        .expect("case-a fixture exists")
        .retrieved_ids
        .clear();

    assert!(validate_remote_evidence(&parent, &candidate).is_err());
}

#[test]
fn remote_evidence_accepts_complete_equivalent_pair() {
    let parent = evidence();
    let mut candidate = evidence();
    candidate.commit = "2".repeat(40);

    assert!(validate_remote_evidence(&parent, &candidate).is_ok());
}
