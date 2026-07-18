#![allow(dead_code)]

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use memory_mcp::models::{ContradictionWarning, IngestRequest};
use memory_mcp::storage::DbClient;
use serde::Deserialize;

// ─── Fixture contract types ────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct ClaimCase {
    id: String,
    corpus_version: String,
    split: CorpusSplit,
    origin: CorpusOrigin,
    language: String,
    #[serde(default)]
    setup: Vec<SourceSample>,
    source: SourceSample,
    expected: ExpectedCase,
    #[serde(default)]
    coverage: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CorpusSplit {
    Development,
    Test,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CorpusOrigin {
    AnonymizedReal,
    ExternalPublic,
    SyntheticAdversarial,
}

#[derive(Debug, Deserialize)]
struct SourceSample {
    source_type: String,
    source_id: String,
    content: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    policy_tags: Vec<String>,
    t_ref: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedCase {
    #[serde(default)]
    claims: Vec<ExpectedClaim>,
    #[serde(default)]
    relations: Vec<ExpectedRelation>,
    #[serde(default)]
    skip_reason_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedClaim {
    schema: String,
    subject: String,
    #[serde(default)]
    comparison_key: BTreeMap<String, String>,
    value: serde_json::Value,
    #[serde(default)]
    qualifiers: BTreeMap<String, String>,
    cardinality: String,
    #[serde(default)]
    valid_from: Option<String>,
    #[serde(default)]
    valid_to: Option<String>,
    source_span: String,
}

#[derive(Debug, Deserialize)]
struct ExpectedRelation {
    setup_source_id: String,
    source_id: String,
    outcome: String,
    reason_code: String,
    #[serde(default)]
    predecessor_source_id: Option<String>,
    #[serde(default)]
    successor_source_id: Option<String>,
}

// ─── Fixture loading ──────────────────────────────────────────────────────────

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("claim_reconciliation_cases.json")
}

fn load_cases() -> Vec<ClaimCase> {
    let raw = std::fs::read_to_string(fixture_path()).expect("read claim reconciliation fixture");
    serde_json::from_str(&raw).expect("parse claim reconciliation fixture")
}

// ─── Integrity test ────────────────────────────────────────────────────────────

#[test]
fn claim_fixture_covers_every_schema_outcome_and_isolation_boundary() {
    let cases = load_cases();

    // Collect schemas, outcomes, splits, languages from the fixture
    let mut schemas_seen = BTreeSet::new();
    let mut outcomes_seen = BTreeSet::new();
    let mut has_duplicate = false;
    let mut has_coexistence = false;
    let mut has_not_comparable = false;
    let mut has_not_same_slot = false;
    let mut languages_seen = BTreeSet::new();
    let mut source_types_seen = BTreeSet::new();
    let mut has_dev_split = false;
    let mut has_test_split = false;
    let mut has_non_synthetic_positive = false;
    let mut has_non_synthetic_negative = false;

    // Collect all coverage tags
    let mut all_coverage = BTreeSet::new();

    for case in &cases {
        for claim in &case.expected.claims {
            schemas_seen.insert(claim.schema.clone());
        }

        for relation in &case.expected.relations {
            outcomes_seen.insert(relation.outcome.clone());
            match relation.outcome.as_str() {
                "duplicate" => has_duplicate = true,
                "coexistence" => has_coexistence = true,
                _ => {}
            }
        }

        for code in &case.expected.skip_reason_codes {
            match code.as_str() {
                "not_comparable" => has_not_comparable = true,
                "not_same_slot" => has_not_same_slot = true,
                _ => {}
            }
        }

        languages_seen.insert(case.language.clone());
        source_types_seen.insert(case.source.source_type.clone());

        for tag in &case.coverage {
            all_coverage.insert(tag.clone());
        }

        match case.split {
            CorpusSplit::Development => has_dev_split = true,
            CorpusSplit::Test => has_test_split = true,
        }

        // Origin-based positive/negative
        if !case.expected.claims.is_empty() || !case.expected.relations.is_empty() {
            match case.origin {
                CorpusOrigin::AnonymizedReal | CorpusOrigin::ExternalPublic => {
                    has_non_synthetic_positive = true
                }
                CorpusOrigin::SyntheticAdversarial => {}
            }
        }
        if case.expected.skip_reason_codes.iter().any(|c| {
            c == "not_comparable"
                || c == "not_same_slot"
                || c == "unresolved_subject"
                || c == "missing_comparison_key"
        }) {
            match case.origin {
                CorpusOrigin::AnonymizedReal | CorpusOrigin::ExternalPublic => {
                    has_non_synthetic_negative = true
                }
                CorpusOrigin::SyntheticAdversarial => {}
            }
        }
    }

    // All four schemas
    let required_schemas: BTreeSet<String> = [
        "attribute/v1".to_string(),
        "quantity/v1".to_string(),
        "relation/v1".to_string(),
        "commitment/v1".to_string(),
    ]
    .into_iter()
    .collect();
    assert!(
        schemas_seen.is_superset(&required_schemas),
        "missing schemas: {:?} (have {:?})",
        required_schemas
            .difference(&schemas_seen)
            .collect::<Vec<_>>(),
        schemas_seen
    );

    // All five persisted outcomes
    let required_outcomes: BTreeSet<String> = [
        "duplicate".to_string(),
        "contradiction".to_string(),
        "coexistence".to_string(),
        "supersession".to_string(),
        "correction".to_string(),
    ]
    .into_iter()
    .collect();
    assert!(
        outcomes_seen.is_superset(&required_outcomes),
        "missing outcomes: {:?} (have {:?})",
        required_outcomes
            .difference(&outcomes_seen)
            .collect::<Vec<_>>(),
        outcomes_seen
    );

    // Negative isolation cases
    assert!(has_duplicate, "missing duplicate case");
    assert!(has_coexistence, "missing coexistence case");
    assert!(has_not_comparable, "missing not_comparable skip case");
    assert!(has_not_same_slot, "missing not_same_slot skip case");

    // Language coverage: English + Russian + at least one non-Latin
    assert!(languages_seen.contains("en"), "missing English cases");
    assert!(languages_seen.contains("ru"), "missing Russian cases");
    let has_non_latin = languages_seen.iter().any(|lang| {
        !lang.starts_with("en")
            && !lang.starts_with("ru")
            && !lang.starts_with("de")
            && !lang.starts_with("fr")
            && !lang.starts_with("es")
    });
    assert!(
        has_non_latin,
        "missing non-Latin language case (have {:?})",
        languages_seen
    );

    // Source type coverage
    assert!(
        !source_types_seen.is_empty(),
        "must have at least one source type"
    );

    // Required coverage tags: qualitative markers, domain dimensions, isolation
    let required_coverage: BTreeSet<String> = [
        "alias".to_string(),
        "unit_conversion".to_string(),
        "unknown_unit".to_string(),
        "missing_time".to_string(),
        "overlapping_interval".to_string(),
        "disjoint_interval".to_string(),
        "correction".to_string(),
        "supersession".to_string(),
        "cross_scope".to_string(),
        "cross_project".to_string(),
        "cross_policy".to_string(),
        "unresolved_subject".to_string(),
        "qualifier_mismatch".to_string(),
        "set_valued".to_string(),
        "domain_finance".to_string(),
        "domain_staffing".to_string(),
        "domain_delivery".to_string(),
        "domain_compliance".to_string(),
        "domain_incidents".to_string(),
        "domain_decisions".to_string(),
        "domain_preferences".to_string(),
        "domain_configuration".to_string(),
        "domain_commitments".to_string(),
        "domain_relations".to_string(),
        "structured_source".to_string(),
        "kv_source".to_string(),
        "free_sentence_source".to_string(),
    ]
    .into_iter()
    .collect();

    let missing_coverage: Vec<_> = required_coverage
        .difference(&all_coverage)
        .cloned()
        .collect();
    assert!(
        missing_coverage.is_empty(),
        "missing coverage tags: {:?} (have {:?})",
        missing_coverage,
        all_coverage
    );

    // Split coverage
    assert!(has_dev_split, "missing development split");
    assert!(has_test_split, "missing test split");

    // Non-synthetic positive and negative
    assert!(
        has_non_synthetic_positive,
        "must have at least one non-synthetic positive case"
    );
    assert!(
        has_non_synthetic_negative,
        "must have at least one non-synthetic negative case"
    );
}

// ─── Legacy baseline types ─────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct LegacyBaselineSummary {
    total_cases: usize,
    expected_contradictions: usize,
    predicted_warnings: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    unsupported_schema_cases: usize,
    isolation_violations: usize,
}

impl LegacyBaselineSummary {
    fn precision(&self) -> f64 {
        if self.predicted_warnings == 0 {
            return if self.expected_contradictions == 0 {
                1.0
            } else {
                0.0
            };
        }
        self.true_positives as f64 / self.predicted_warnings as f64
    }

    fn recall(&self) -> f64 {
        if self.expected_contradictions == 0 {
            return 1.0;
        }
        self.true_positives as f64 / self.expected_contradictions as f64
    }
}

#[derive(serde::Serialize)]
struct LegacyBaselineReport {
    corpus_version: String,
    split: String,
    total_cases: usize,
    expected_contradictions: usize,
    predicted_warnings: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    precision: f64,
    recall: f64,
    unsupported_schema_cases: usize,
    isolation_violations: usize,
}

// ─── Current engine evaluation types ───────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct EvaluationReport {
    corpus_version: String,
    engine: &'static str,
    split: &'static str,
    total_cases: usize,
    expected_relations: usize,
    predicted_relations: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    precision: f64,
    recall: f64,
    isolation_violations: usize,
    per_schema: BTreeMap<String, OutcomeCounts>,
    latency_ms_p50: f64,
    latency_ms_p95: f64,
}

#[derive(Debug, Default, serde::Serialize)]
struct OutcomeCounts {
    expected: usize,
    predicted: usize,
    matched: usize,
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn warning_matches_legacy(expected: &ExpectedRelation, actual: &ContradictionWarning) -> bool {
    // Legacy warnings compare fact_type and content overlap
    // We match on the new_fact_id containing the expected source_id fragment
    // and the fact_type being compatible
    actual.new_fact_id.contains(&expected.source_id)
        || actual
            .conflicting_fact_id
            .contains(&expected.setup_source_id)
}

#[allow(clippy::too_many_arguments)]
async fn ingest_and_extract(
    service: &memory_mcp::service::MemoryService,
    scope: &str,
    project: Option<&str>,
    source_type: &str,
    source_id: &str,
    content: &str,
    t_ref: &str,
    policy_tags: &[String],
) -> memory_mcp::models::ExtractResult {
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: source_type.to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: t_ref
                    .parse::<DateTime<Utc>>()
                    .unwrap_or_else(|_| Utc::now()),
                scope: scope.to_string(),
                project: project.map(str::to_string),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: policy_tags.to_vec(),
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

#[tokio::test]
#[ignore]
async fn run_claim_reconciliation_evals() {
    let cases = load_cases();
    let mut dev_summary = LegacyBaselineSummary::default();
    let mut test_summary = LegacyBaselineSummary::default();

    for case in &cases {
        let summary = match case.split {
            CorpusSplit::Development => &mut dev_summary,
            CorpusSplit::Test => &mut test_summary,
        };
        summary.total_cases += 1;

        let service = common::make_service().await;

        // Ingest setup episodes
        for setup in &case.setup {
            let _ = ingest_and_extract(
                &service,
                &setup.scope,
                setup.project.as_deref(),
                &setup.source_type,
                &setup.source_id,
                &setup.content,
                &setup.t_ref,
                &setup.policy_tags,
            )
            .await;
        }

        // Ingest the source episode
        let extraction = ingest_and_extract(
            &service,
            &case.source.scope,
            case.source.project.as_deref(),
            &case.source.source_type,
            &case.source.source_id,
            &case.source.content,
            &case.source.t_ref,
            &case.source.policy_tags,
        )
        .await;

        // Count expected contradictions (relations with outcome "contradiction")
        let expected_contradictions = case
            .expected
            .relations
            .iter()
            .filter(|r| r.outcome == "contradiction")
            .count();
        summary.expected_contradictions += expected_contradictions;

        // Count unsupported schema cases
        let has_unsupported = case
            .expected
            .claims
            .iter()
            .any(|c| c.schema == "unsupported");
        if has_unsupported {
            summary.unsupported_schema_cases += 1;
        }

        // Count isolation violations
        let has_isolation_skip = case
            .expected
            .skip_reason_codes
            .iter()
            .any(|c| c == "not_same_slot" || c == "cross_scope" || c == "cross_project");
        if has_isolation_skip {
            summary.isolation_violations += 1;
        }

        // Compare legacy warnings against expected contradictions
        let matched_warnings: usize = case
            .expected
            .relations
            .iter()
            .filter(|r| r.outcome == "contradiction")
            .filter(|expected| {
                extraction
                    .warnings
                    .iter()
                    .any(|actual| warning_matches_legacy(expected, actual))
            })
            .count();

        summary.predicted_warnings += extraction.warnings.len();
        summary.true_positives += matched_warnings;
        summary.false_positives += extraction.warnings.len() - matched_warnings;
        summary.false_negatives += expected_contradictions - matched_warnings;
    }

    // Print reports
    let print_report = |split: &str, summary: &LegacyBaselineSummary| {
        let report = LegacyBaselineReport {
            corpus_version: "claim-reconciliation/v1".to_string(),
            split: split.to_string(),
            total_cases: summary.total_cases,
            expected_contradictions: summary.expected_contradictions,
            predicted_warnings: summary.predicted_warnings,
            true_positives: summary.true_positives,
            false_positives: summary.false_positives,
            false_negatives: summary.false_negatives,
            precision: summary.precision(),
            recall: summary.recall(),
            unsupported_schema_cases: summary.unsupported_schema_cases,
            isolation_violations: summary.isolation_violations,
        };
        println!(
            "{}",
            serde_json::to_string(&report).expect("serialize baseline report")
        );
    };

    if dev_summary.total_cases > 0 {
        print_report("development", &dev_summary);
    }
    if test_summary.total_cases > 0 {
        print_report("test", &test_summary);
    }
}
