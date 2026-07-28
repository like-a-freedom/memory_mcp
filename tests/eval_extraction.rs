use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ExtractionEvalCase {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    setup_episodes: Vec<SetupEpisode>,
    expected: ExtractionExpectation,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SetupEpisode {
    source_type: String,
    source_id: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct ExtractionExpectation {
    #[serde(default)]
    fact_types: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    entities: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    warnings: Vec<ExpectedWarning>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ExpectedWarning {
    fact_type: String,
    existing_content: String,
    new_content: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("extraction_cases.json")
}

fn load_cases() -> Vec<ExtractionEvalCase> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read extraction fixture");
    serde_json::from_str(&raw).expect("parse extraction fixture")
}

#[test]
fn extraction_fixture_provides_contradiction_warning_coverage() {
    let raw = std::fs::read_to_string(fixture_path()).expect("read extraction fixture");
    let cases = serde_json::from_str::<Vec<serde_json::Value>>(&raw)
        .expect("parse extraction fixture as json");

    let contradiction_cases = cases
        .iter()
        .filter(|case| {
            case.get("expected")
                .and_then(|expected| expected.get("warnings"))
                .and_then(|warnings| warnings.as_array())
                .is_some_and(|warnings| !warnings.is_empty())
        })
        .count();

    assert!(
        contradiction_cases >= 5,
        "expected at least 5 contradiction extraction cases, got {contradiction_cases}"
    );
}

#[test]
fn extraction_fixture_provides_experience_fact_type_coverage() {
    let cases = load_cases();
    let experience_cases = cases
        .iter()
        .filter(|case| {
            case.expected
                .fact_types
                .iter()
                .any(|fact_type| fact_type == "experience")
        })
        .count();

    assert!(
        experience_cases >= 1,
        "expected at least 1 experience extraction case, got {experience_cases}"
    );
}

#[test]
fn extraction_fixture_provides_document_action_item_coverage() {
    let cases = load_cases();
    let action_item_cases = cases
        .iter()
        .filter(|case| {
            case.expected
                .fact_types
                .iter()
                .any(|fact_type| fact_type == "promise")
        })
        .count();

    assert!(
        action_item_cases >= 1,
        "expected at least 1 document-style action-item extraction case, got {action_item_cases}"
    );
}

/// Thin launcher that delegates to the eval-harness extraction suite.
/// Run with: cargo test --test eval_extraction harness_extraction_suite -- --ignored --exact
#[tokio::test]
#[ignore]
async fn harness_extraction_suite() {
    use eval_harness::{EvalProfile, EvalSuite, ExtractionSuite, RunContext};

    let suite = ExtractionSuite::new().expect("load extraction suite");
    let context = RunContext {
        profile: EvalProfile::Pr,
    };
    let outcomes = suite.run(&context).await;

    let passed = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Passed)
        .count();
    let failed = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::QualityFailed)
        .count();
    let invalid = outcomes
        .iter()
        .filter(|o| o.status == eval_harness::CaseStatus::Invalid)
        .count();

    eprintln!(
        "harness_extraction: total={} passed={} failed={} invalid={}",
        outcomes.len(),
        passed,
        failed,
        invalid
    );

    assert!(
        invalid == 0,
        "harness extraction suite has {invalid} invalid cases"
    );
}
