use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RetrievalEvalCase {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Vec<String>,
    expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize)]
struct RetrievalExpectation {
    #[allow(dead_code)]
    tier: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("retrieval_cases.json")
}

fn load_cases() -> Vec<RetrievalEvalCase> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read retrieval fixture");
    serde_json::from_str(&raw).expect("parse retrieval fixture")
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct RetrievalDiversityExpectation {
    #[serde(default)]
    min_unique_source_episodes: Option<usize>,
    #[serde(default)]
    max_source_episode_share: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct FullRetrievalEvalCase {
    #[allow(dead_code)]
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Vec<String>,
    expected: FullRetrievalExpectation,
}

#[derive(Debug, Deserialize)]
struct FullRetrievalExpectation {
    #[serde(default)]
    diversity: Option<RetrievalDiversityExpectation>,
    #[serde(default)]
    must_contain: Vec<String>,
}

#[test]
fn retrieval_fixture_provides_minimum_tier_coverage() {
    let cases = load_cases();
    let mut counts = std::collections::BTreeMap::<String, usize>::new();

    for case in &cases {
        *counts.entry(case.expected.tier.clone()).or_insert(0) += 1;
    }

    assert!(
        cases.len() >= 50,
        "expected at least 50 retrieval eval cases, got {}",
        cases.len()
    );

    for tier in ["direct", "alias", "temporal", "graph", "reasoning"] {
        let count = counts.get(tier).copied().unwrap_or_default();
        assert!(
            count >= 10,
            "expected at least 10 retrieval cases for tier {tier}, got {count}"
        );
    }
}

#[test]
fn retrieval_fixture_provides_graph_and_timeline_tag_coverage() {
    let cases = load_cases();

    for tag in ["timeline_auto", "graph_anchor", "first_person_rescue"] {
        let count = cases
            .iter()
            .filter(|case| case.tags.iter().any(|value| value == tag))
            .count();
        assert!(
            count >= 1,
            "expected at least one retrieval eval case tagged {tag}, got {count}"
        );
    }
}

#[test]
fn retrieval_fixture_provides_diversity_coverage() {
    let raw = std::fs::read_to_string(fixture_path()).expect("read retrieval fixture");
    let cases: Vec<FullRetrievalEvalCase> =
        serde_json::from_str(&raw).expect("parse retrieval fixture");
    let diversity_cases = cases
        .iter()
        .filter(|case| case.expected.diversity.is_some())
        .collect::<Vec<_>>();

    assert!(
        diversity_cases.len() >= 2,
        "expected at least 2 diversity-sensitive retrieval cases, got {}",
        diversity_cases.len()
    );
    assert!(
        diversity_cases
            .iter()
            .all(|case| case.expected.must_contain.len() >= 2),
        "diversity-sensitive retrieval cases should exercise multi-hit coverage"
    );
}

#[test]
#[should_panic(expected = "retrieval launcher requires every case to pass")]
fn retrieval_launcher_rejects_a_quality_failed_case() {
    use eval_harness::{CaseStatus, CorpusSplit, EvalCaseOutcome, EvalMode, LabelTrust};

    let failed = EvalCaseOutcome::new(
        "local-retrieval",
        "injected-quality-failure",
        EvalMode::RetrievalOnly,
        CorpusSplit::Development,
        LabelTrust::Weak,
        CaseStatus::QualityFailed,
    );

    assert_retrieval_cases_passed(&[failed], 1);
}

/// Thin launcher that delegates to the eval-harness retrieval suite.
/// Run with: cargo test --test eval_retrieval harness_retrieval_suite -- --ignored --exact
#[tokio::test]
#[ignore]
async fn harness_retrieval_suite() {
    use eval_harness::{EvalProfile, EvalSuite, RetrievalSuite, RunContext};

    let suite = RetrievalSuite::new().expect("load retrieval suite");
    let expected_cases = suite.expected_case_ids().len();
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
        "harness_retrieval: total={} passed={} failed={} invalid={}",
        outcomes.len(),
        passed,
        failed,
        invalid
    );
    for outcome in outcomes
        .iter()
        .filter(|outcome| outcome.status != eval_harness::CaseStatus::Passed)
    {
        eprintln!(
            "harness_retrieval_case: key={:?} status={:?} failures={:?} invalid_reason={:?}",
            outcome.case_key, outcome.status, outcome.failures, outcome.invalid_reason
        );
    }

    assert_retrieval_cases_passed(&outcomes, expected_cases);
}

fn assert_retrieval_cases_passed(
    outcomes: &[eval_harness::EvalCaseOutcome],
    expected_cases: usize,
) {
    assert_eq!(
        outcomes.len(),
        expected_cases,
        "retrieval launcher requires exact declared case coverage"
    );
    assert!(
        !outcomes.is_empty(),
        "retrieval launcher must not pass an empty suite"
    );
    assert!(
        outcomes
            .iter()
            .all(|outcome| outcome.status == eval_harness::CaseStatus::Passed),
        "retrieval launcher requires every case to pass; quality-failed or invalid cases remain visible"
    );
}
