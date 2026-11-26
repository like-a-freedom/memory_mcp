use eval_harness::{CaseStatus, EvalProfile, EvalSuite, ExtractionSuite, RunContext};

#[tokio::test]
async fn complete_entity_expectation_rejects_false_positives() {
    let outcomes = ExtractionSuite::new()
        .unwrap()
        .run(&RunContext::new(EvalProfile::Pr))
        .await;
    let outcome = outcomes
        .iter()
        .find(|o| o.case_id().as_str() == "ext-001")
        .unwrap();
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
