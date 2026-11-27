use eval_harness::{CaseStatus, EvalProfile, EvalSuite, ExtractionSuite, RunContext};

#[tokio::test]
async fn complete_entity_expectation_rejects_false_positives() {
    let outcome = run_fixture(serde_json::json!({
        "id":"deliberately-incomplete-entities", "description":"adversarial closed expectation omits a clearly named person", "source_type":"chat", "source_id":"person", "content":"Alice Smith prefers weekly updates.", "expected":{}
    })).await;
    assert_eq!(outcome.metrics["entity_precision"], 0.0);
    assert_eq!(outcome.status, CaseStatus::QualityFailed);
    assert!(
        outcome
            .failures
            .iter()
            .any(|reason| reason.contains("unexpected entities"))
    );
}

async fn run_fixture(case: serde_json::Value) -> eval_harness::EvalCaseOutcome {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("extraction.json");
    std::fs::write(&path, serde_json::to_vec(&vec![case]).unwrap()).unwrap();
    let outcomes = ExtractionSuite::from_fixture_path(path)
        .unwrap()
        .run(&RunContext::new(EvalProfile::Pr))
        .await;
    assert_eq!(outcomes.len(), 1);
    outcomes.into_iter().next().unwrap()
}

#[tokio::test]
async fn complete_warning_expectation_rejects_additional_contradictions() {
    let outcome = run_fixture(serde_json::json!({
        "id": "warning-extras", "description": "two changed metrics with only one expected warning",
        "source_type": "chat", "source_id": "current",
        "content": "Alice Smith reports ARR is $7M.",
        "setup_episodes": [{"source_type": "chat", "source_id": "prior", "content": "Alice Smith reports ARR is $5M."}, {"source_type":"chat", "source_id":"prior-2", "content":"Alice Smith reports ARR is $6M."}],
        "expected": {"entities": ["Alice Smith"], "warnings": [{"fact_type":"metric", "existing_content":"Alice Smith reports ARR is $5M.", "new_content":"Alice Smith reports ARR is $7M."}]}
    })).await;
    assert_eq!(outcome.metrics["warning_recall"], 1.0);
    assert_eq!(outcome.status, CaseStatus::QualityFailed);
    assert!(
        outcome
            .failures
            .iter()
            .any(|reason| reason.contains("unexpected warnings")),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn setup_fault_preserves_declared_development_population() {
    use eval_harness::{CorpusSplit, EvalMode, LabelTrust};
    let outcome = run_fixture(serde_json::json!({
        "id":"setup-fault", "description":"invalid setup input", "source_type":"chat", "source_id":"current", "content":"Alice Smith reports ARR is $7M.",
        "setup_episodes":[{"source_type":"chat", "source_id":"setup", "content":""}], "expected":{}
    })).await;
    assert_eq!(outcome.status, CaseStatus::Invalid);
    assert!(
        outcome
            .invalid_reason
            .as_ref()
            .unwrap()
            .contains("content is required")
    );
    assert_eq!(outcome.mode, EvalMode::EndToEnd);
    assert_eq!(outcome.split, CorpusSplit::Development);
    assert_eq!(outcome.label_trust, LabelTrust::Reviewed);
}

#[tokio::test]
async fn primary_fault_retains_reviewed_development_identity() {
    use eval_harness::{CorpusSplit, EvalMode, LabelTrust};
    let outcome = run_fixture(serde_json::json!({
        "id":"primary-fault", "description":"invalid primary input", "source_type":"chat", "source_id":"current", "content":"", "expected":{}
    })).await;
    assert_eq!(outcome.status, CaseStatus::Invalid);
    assert!(
        outcome
            .invalid_reason
            .as_ref()
            .unwrap()
            .contains("content is required")
    );
    assert_eq!(outcome.mode, EvalMode::EndToEnd);
    assert_eq!(outcome.split, CorpusSplit::Development);
    assert_eq!(outcome.label_trust, LabelTrust::Reviewed);
}

#[tokio::test]
async fn complete_empty_expectations_accept_no_predictions() {
    use eval_harness::{CorpusSplit, EvalMode, LabelTrust};
    let outcome = run_fixture(serde_json::json!({
        "id":"empty-control", "description":"no named entities or contradiction", "source_type":"chat", "source_id":"current", "content":"we prefer weekly updates.", "expected":{}
    })).await;
    assert_eq!(outcome.status, CaseStatus::Passed, "{outcome:?}");
    assert_eq!(outcome.mode, EvalMode::EndToEnd);
    assert_eq!(outcome.split, CorpusSplit::Development);
    assert_eq!(outcome.label_trust, LabelTrust::Reviewed);
}

#[tokio::test]
async fn unreadable_fixture_remains_invalid_reviewed_development_evidence() {
    use eval_harness::{CorpusSplit, EvalMode, LabelTrust};
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cases.json");
    std::fs::write(&path, r#"[{"id":"input-case", "description":"source", "source_type":"chat", "source_id":"source", "content":"we prefer weekly updates.", "expected":{}}]"#).unwrap();
    let suite = ExtractionSuite::from_fixture_path(path.clone()).unwrap();
    std::fs::write(path, "broken json").unwrap();
    let outcomes = suite.run(&RunContext::new(EvalProfile::Pr)).await;
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].status, CaseStatus::Invalid);
    assert!(outcomes[0].invalid_reason.is_some());
    assert_eq!(outcomes[0].mode, EvalMode::EndToEnd);
    assert_eq!(outcomes[0].split, CorpusSplit::Development);
    assert_eq!(outcomes[0].label_trust, LabelTrust::Reviewed);
}

#[tokio::test]
async fn deadline_retains_declared_reviewed_development_population() {
    use eval_harness::{CorpusSplit, EvalMode, LabelTrust};
    let suite = ExtractionSuite::new().unwrap();
    let outcomes = suite
        .run_with_deadline(&RunContext::new(EvalProfile::Pr), std::time::Instant::now())
        .await;
    let outcome = outcomes
        .iter()
        .find(|o| o.case_id().as_str() == "ext-009")
        .unwrap();
    assert_eq!(outcome.status, CaseStatus::Invalid);
    assert_eq!(outcome.mode, EvalMode::EndToEnd);
    assert_eq!(outcome.split, CorpusSplit::Development);
    assert_eq!(outcome.label_trust, LabelTrust::Reviewed);
}

/// Source-labelled observation through the public ingestion/extraction boundary.
/// Quality failures are data, not a reason to omit later scheduled cases.
#[tokio::test]
#[ignore = "explicit registered corpus capture; output must be a new file"]
async fn observe_registered_multilingual_admission() {
    use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
    use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
    use std::{collections::BTreeSet, path::PathBuf};
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/entity-admission-multilingual-v1/cases.json");
    let cases: Vec<serde_json::Value> =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let output = PathBuf::from(std::env::var("ENTITY_ADMISSION_CAPTURE").expect("new output file"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)
        .unwrap();
    let mut rows = vec![];
    for case in &cases {
        let service = eval_harness::test_support::make_service().await;
        let started = std::time::Instant::now();
        let observed = async {
            let request = serde_json::from_value(serde_json::json!({"source_type":case["source_type"],"source_id":case["source_id"],"content":case["content"],"t_ref":"2026-04-07T10:00:00Z"})).unwrap();
            let episode = IngestCapability::ingest_from_service(&service, request, None).await?;
            ExtractCapability::extract_from_service(&service, &episode, None, None).await
        }.await;
        let row = match observed {
            Ok(result) => {
                let expected = case["expected"]["entities"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|v| v.as_str().unwrap().to_owned())
                    .collect::<BTreeSet<_>>();
                let predicted = result
                    .entities
                    .iter()
                    .map(|e| e.canonical_name.clone())
                    .collect::<BTreeSet<_>>();
                let linked = result
                    .entities
                    .iter()
                    .filter(|entity| {
                        result.links.iter().any(|link| {
                            link.entity_id == entity.entity_id
                                && link.episode_id == result.episode_id
                        })
                    })
                    .map(|e| e.canonical_name.clone())
                    .collect::<BTreeSet<_>>();
                serde_json::json!({"id":case["id"],"class":case["class"],"language":case["language"],"status":"completed","expected":expected,"predicted":predicted,"linked":linked,"tp":expected.intersection(&predicted).count(),"fp":predicted.difference(&expected).count(),"fn":expected.difference(&predicted).count(),"link_tp":expected.intersection(&linked).count(),"link_fp":linked.difference(&expected).count(),"link_fn":expected.difference(&linked).count(),"passed":expected==predicted,"raw":result,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
            }
            Err(error) => {
                serde_json::json!({"id":case["id"],"class":case["class"],"language":case["language"],"status":"invalid","error":error.to_string(),"elapsed_ms":started.elapsed().as_secs_f64()*1000.0})
            }
        };
        rows.push(row);
    }
    use std::io::Write;
    serde_json::to_writer_pretty(&mut file, &serde_json::json!({"schema":"entity-admission-capture/v1","scheduled":cases.len(),"rows":rows})).unwrap();
    file.flush().unwrap();
    assert_eq!(rows.len(), cases.len());
    assert!(
        rows.iter().all(|r| r["status"] == "completed"),
        "faults retained in output"
    );
}
