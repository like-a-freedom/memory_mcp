mod common;

use std::collections::BTreeSet;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use memory_mcp::models::{ContradictionWarning, IngestRequest};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct ExtractionEvalCase {
    id: String,
    description: String,
    source_type: String,
    source_id: String,
    scope: String,
    content: String,
    #[serde(default)]
    setup_episodes: Vec<SetupEpisode>,
    expected: ExtractionExpectation,
}

#[derive(Debug, Deserialize)]
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
    entities: Vec<String>,
    #[serde(default)]
    warnings: Vec<ExpectedWarning>,
}

#[derive(Debug, Deserialize)]
struct ExpectedWarning {
    fact_type: String,
    existing_content: String,
    new_content: String,
}

#[derive(Debug, Default)]
struct ExtractionSummary {
    total_cases: usize,
    passed_cases: usize,
    expected_fact_types: usize,
    matched_fact_types: usize,
    expected_entities: usize,
    matched_entities: usize,
    predicted_entities: usize,
    expected_warnings: usize,
    matched_warnings: usize,
}

#[derive(Debug)]
struct ExtractionCaseOutcome {
    predicted_fact_types: BTreeSet<String>,
    predicted_entities: BTreeSet<String>,
    warnings: Vec<ContradictionWarning>,
}

impl ExtractionSummary {
    fn entity_precision(&self) -> f64 {
        if self.predicted_entities == 0 {
            return if self.expected_entities == 0 {
                1.0
            } else {
                0.0
            };
        }
        self.matched_entities as f64 / self.predicted_entities as f64
    }

    fn entity_recall(&self) -> f64 {
        if self.expected_entities == 0 {
            return 1.0;
        }
        self.matched_entities as f64 / self.expected_entities as f64
    }

    fn entity_f1(&self) -> f64 {
        let precision = self.entity_precision();
        let recall = self.entity_recall();
        if (precision + recall).abs() < f64::EPSILON {
            return 0.0;
        }
        2.0 * precision * recall / (precision + recall)
    }

    fn fact_type_accuracy(&self) -> f64 {
        if self.expected_fact_types == 0 {
            return 1.0;
        }
        self.matched_fact_types as f64 / self.expected_fact_types as f64
    }

    fn warning_recall(&self) -> f64 {
        if self.expected_warnings == 0 {
            return 1.0;
        }
        self.matched_warnings as f64 / self.expected_warnings as f64
    }
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

async fn ingest_and_extract(
    service: &memory_mcp::service::MemoryService,
    scope: &str,
    source_type: &str,
    source_id: &str,
    content: &str,
) -> memory_mcp::models::ExtractResult {
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: source_type.to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: "2026-04-07T10:00:00Z"
                    .parse::<DateTime<Utc>>()
                    .expect("static timestamp should parse"),
                scope: scope.to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .unwrap_or_else(|err| panic!("source {source_id} failed to ingest: {err}"));

    service
        .extract(&episode_id, None, None)
        .await
        .unwrap_or_else(|err| panic!("source {source_id} failed to extract: {err}"))
}

async fn run_case(case: &ExtractionEvalCase) -> ExtractionCaseOutcome {
    let service = common::make_service().await;

    for setup in &case.setup_episodes {
        ingest_and_extract(
            &service,
            &case.scope,
            &setup.source_type,
            &setup.source_id,
            &setup.content,
        )
        .await;
    }

    let extraction = ingest_and_extract(
        &service,
        &case.scope,
        &case.source_type,
        &case.source_id,
        &case.content,
    )
    .await;

    ExtractionCaseOutcome {
        predicted_fact_types: extraction
            .facts
            .iter()
            .map(|fact| fact.fact_type.clone())
            .collect::<BTreeSet<_>>(),
        predicted_entities: extraction
            .entities
            .iter()
            .map(|entity| entity.canonical_name.clone())
            .collect::<BTreeSet<_>>(),
        warnings: extraction.warnings,
    }
}

fn warning_matches(expected: &ExpectedWarning, actual: &ContradictionWarning) -> bool {
    actual.fact_type == expected.fact_type
        && actual.existing_content == expected.existing_content
        && actual.new_content == expected.new_content
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
            case.source_type == "email"
                && case.content.to_lowercase().contains("action items")
                && case
                    .expected
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

#[tokio::test]
async fn extraction_runner_supports_setup_episodes_and_warning_expectations() {
    let case = ExtractionEvalCase {
        id: "ext-warning-check".to_string(),
        description: "contradictory metric warning via setup episode".to_string(),
        source_type: "chat".to_string(),
        source_id: "ext-warning-check-current".to_string(),
        scope: "personal".to_string(),
        content: "Alice Smith reports ARR is $7M.".to_string(),
        setup_episodes: vec![SetupEpisode {
            source_type: "chat".to_string(),
            source_id: "ext-warning-check-setup".to_string(),
            content: "Alice Smith reports ARR is $5M.".to_string(),
        }],
        expected: ExtractionExpectation {
            fact_types: vec!["metric".to_string()],
            entities: vec!["Alice Smith".to_string()],
            warnings: vec![ExpectedWarning {
                fact_type: "metric".to_string(),
                existing_content: "Alice Smith reports ARR is $5M.".to_string(),
                new_content: "Alice Smith reports ARR is $7M.".to_string(),
            }],
        },
    };

    let outcome = run_case(&case).await;

    assert!(outcome.predicted_fact_types.contains("metric"));
    assert!(outcome.predicted_entities.contains("Alice Smith"));
    assert!(
        outcome
            .warnings
            .iter()
            .any(|warning| warning_matches(&case.expected.warnings[0], warning))
    );
}

#[tokio::test]
#[ignore]
async fn run_extraction_evals() {
    let cases = load_cases();
    let mut summary = ExtractionSummary::default();

    for case in cases {
        summary.total_cases += 1;
        let outcome = run_case(&case).await;
        let expected_fact_types = case
            .expected
            .fact_types
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_entities = case
            .expected
            .entities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let matched_warnings = case
            .expected
            .warnings
            .iter()
            .filter(|expected| {
                outcome
                    .warnings
                    .iter()
                    .any(|actual| warning_matches(expected, actual))
            })
            .count();

        let matched_fact_types = expected_fact_types
            .intersection(&outcome.predicted_fact_types)
            .count();
        let matched_entities = expected_entities
            .intersection(&outcome.predicted_entities)
            .count();

        summary.expected_fact_types += expected_fact_types.len();
        summary.matched_fact_types += matched_fact_types;
        summary.expected_entities += expected_entities.len();
        summary.matched_entities += matched_entities;
        summary.predicted_entities += outcome.predicted_entities.len();
        summary.expected_warnings += case.expected.warnings.len();
        summary.matched_warnings += matched_warnings;

        let warnings_passed = if case.expected.warnings.is_empty() {
            outcome.warnings.is_empty()
        } else {
            matched_warnings == case.expected.warnings.len()
        };

        let case_passed = matched_fact_types == expected_fact_types.len()
            && matched_entities == expected_entities.len()
            && warnings_passed;
        if case_passed {
            summary.passed_cases += 1;
        }

        assert!(
            case_passed,
            "case {} ({}) failed: expected_fact_types={:?} predicted_fact_types={:?} expected_entities={:?} predicted_entities={:?} expected_warnings={:?} actual_warnings={:?}",
            case.id,
            case.description,
            expected_fact_types,
            outcome.predicted_fact_types,
            expected_entities,
            outcome.predicted_entities,
            case.expected.warnings,
            outcome.warnings,
        );
    }

    println!(
        "suite=eval_extraction total={} passed={} entity_precision={:.2} entity_recall={:.2} entity_f1={:.2} fact_type_accuracy={:.2} warning_recall={:.2}",
        summary.total_cases,
        summary.passed_cases,
        summary.entity_precision(),
        summary.entity_recall(),
        summary.entity_f1(),
        summary.fact_type_accuracy(),
        summary.warning_recall(),
    );
}

/// Diagnostic-only sibling of `run_extraction_evals`.
///
/// The canonical runner above intentionally panics on the first failing case
/// (fail-fast in CI). That makes the suite pass/fail boundary crisp, but it
/// also means the printed summary is never reached when any case fails, so we
/// cannot report an objective breakdown (entity precision/recall/F1, fact-type
/// accuracy, warning recall) for the whole fixture.
///
/// This test mirrors the loop body exactly but does **not** assert — instead it
/// logs which cases failed via `eprintln!` and continues, then prints the same
/// summary line at the end. It is `#[ignore]`d by default to preserve the
/// canonical CI contract; run it explicitly with `--ignored` to collect
/// diagnostic metrics.
#[tokio::test]
#[ignore]
async fn run_extraction_evals_diagnostic() {
    let cases = load_cases();
    let mut summary = ExtractionSummary::default();

    for case in cases {
        summary.total_cases += 1;
        let outcome = run_case(&case).await;
        let expected_fact_types = case
            .expected
            .fact_types
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let expected_entities = case
            .expected
            .entities
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>();
        let matched_warnings = case
            .expected
            .warnings
            .iter()
            .filter(|expected| {
                outcome
                    .warnings
                    .iter()
                    .any(|actual| warning_matches(expected, actual))
            })
            .count();

        let matched_fact_types = expected_fact_types
            .intersection(&outcome.predicted_fact_types)
            .count();
        let matched_entities = expected_entities
            .intersection(&outcome.predicted_entities)
            .count();

        summary.expected_fact_types += expected_fact_types.len();
        summary.matched_fact_types += matched_fact_types;
        summary.expected_entities += expected_entities.len();
        summary.matched_entities += matched_entities;
        summary.predicted_entities += outcome.predicted_entities.len();
        summary.expected_warnings += case.expected.warnings.len();
        summary.matched_warnings += matched_warnings;

        let warnings_passed = if case.expected.warnings.is_empty() {
            outcome.warnings.is_empty()
        } else {
            matched_warnings == case.expected.warnings.len()
        };

        let case_passed = matched_fact_types == expected_fact_types.len()
            && matched_entities == expected_entities.len()
            && warnings_passed;
        if case_passed {
            summary.passed_cases += 1;
        } else {
            eprintln!(
                "[fail] case {} ({}) matched_fact_types={}/{} matched_entities={}/{} matched_warnings={}/{}",
                case.id,
                case.description,
                matched_fact_types,
                expected_fact_types.len(),
                matched_entities,
                expected_entities.len(),
                matched_warnings,
                case.expected.warnings.len(),
            );
        }
    }

    println!(
        "suite=eval_extraction total={} passed={} entity_precision={:.2} entity_recall={:.2} entity_f1={:.2} fact_type_accuracy={:.2} warning_recall={:.2}",
        summary.total_cases,
        summary.passed_cases,
        summary.entity_precision(),
        summary.entity_recall(),
        summary.entity_f1(),
        summary.fact_type_accuracy(),
        summary.warning_recall(),
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
