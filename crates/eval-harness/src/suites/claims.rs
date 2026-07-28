use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::models::{ContradictionWarning, IngestRequest};
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Deserialize)]
struct ClaimCase {
    id: String,
    #[allow(dead_code)]
    corpus_version: String,
    split: ClaimCorpusSplit,
    #[allow(dead_code)]
    origin: String,
    #[allow(dead_code)]
    language: String,
    #[serde(default)]
    setup: Vec<SourceSample>,
    source: SourceSample,
    expected: ExpectedCase,
    #[serde(default)]
    #[allow(dead_code)]
    coverage: Vec<String>,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ClaimCorpusSplit {
    Development,
    Test,
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
    #[allow(dead_code)]
    claims: Vec<ExpectedClaim>,
    #[serde(default)]
    relations: Vec<ExpectedRelation>,
    #[serde(default)]
    skip_reason_codes: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
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
    #[allow(dead_code)]
    reason_code: String,
    #[serde(default)]
    #[allow(dead_code)]
    predecessor_source_id: Option<String>,
    #[serde(default)]
    #[allow(dead_code)]
    successor_source_id: Option<String>,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/evals/claim_reconciliation_cases.json")
}

fn load_cases() -> Result<Vec<ClaimCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

fn warning_matches_exact(expected: &ExpectedRelation, actual: &ContradictionWarning) -> bool {
    actual.new_fact_id == expected.source_id
        || actual.conflicting_fact_id == expected.setup_source_id
}

fn parse_reference_time(raw: &str) -> Result<DateTime<Utc>, EvalError> {
    raw.parse::<DateTime<Utc>>()
        .map_err(|e| EvalError::InvalidInput(format!("invalid reference time '{raw}': {e}")))
}

fn count_isolation_violations(
    expected_skips: &[String],
    actual_relations: &[ContradictionWarning],
) -> usize {
    let skip_set: BTreeSet<&str> = expected_skips.iter().map(String::as_str).collect();
    let mut violations = 0;
    for warning in actual_relations {
        if skip_set.contains(warning.fact_type.as_str()) {
            continue;
        }
        if warning.new_fact_id != warning.conflicting_fact_id {
            violations += 1;
        }
    }
    violations
}

struct IngestParams<'a> {
    scope: &'a str,
    project: Option<&'a str>,
    source_type: &'a str,
    source_id: &'a str,
    content: &'a str,
    t_ref: &'a str,
    policy_tags: &'a [String],
}

async fn ingest_and_extract(
    service: &memory_mcp::service::MemoryService,
    params: &IngestParams<'_>,
) -> Result<memory_mcp::models::ExtractResult, EvalError> {
    let t_ref_datetime = parse_reference_time(params.t_ref)?;

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: params.source_type.to_string(),
                source_id: params.source_id.to_string(),
                content: params.content.to_string(),
                t_ref: t_ref_datetime,
                scope: params.scope.to_string(),
                project: params.project.map(str::to_string),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: params.policy_tags.to_vec(),
            },
            None,
        )
        .await
        .map_err(|e| EvalError::Suite(format!("ingest failed for {}: {e}", params.source_id)))?;

    service
        .extract(&episode_id, None, None)
        .await
        .map_err(|e| EvalError::Suite(format!("extract failed for {}: {e}", params.source_id)))
}

struct CaseMetrics {
    expected_contradictions: usize,
    matched_warnings: usize,
    predicted_warnings: usize,
    isolation_violations: usize,
}

fn evaluate_case(case: &ClaimCase, extraction: &memory_mcp::models::ExtractResult) -> CaseMetrics {
    let expected_contradictions = case
        .expected
        .relations
        .iter()
        .filter(|r| r.outcome == "contradiction")
        .count();

    let matched_warnings: usize = case
        .expected
        .relations
        .iter()
        .filter(|r| r.outcome == "contradiction")
        .filter(|expected| {
            extraction
                .warnings
                .iter()
                .any(|actual| warning_matches_exact(expected, actual))
        })
        .count();

    let isolation_violations =
        count_isolation_violations(&case.expected.skip_reason_codes, &extraction.warnings);

    CaseMetrics {
        expected_contradictions,
        matched_warnings,
        predicted_warnings: extraction.warnings.len(),
        isolation_violations,
    }
}

pub struct ClaimReconciliationSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl ClaimReconciliationSuite {
    pub fn new() -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { expected_ids })
    }
}

#[async_trait]
impl EvalSuite for ClaimReconciliationSuite {
    fn id(&self) -> &str {
        "claim-reconciliation"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::EndToEnd
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(cases) => cases,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("claim-reconciliation", "fixture-load-error").unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(err.to_string()),
                    failures: vec![],
                    duration_ms: 0,
                    attempts: 1,
                }];
            }
        };

        let mut outcomes = Vec::with_capacity(cases.len());
        for case in &cases {
            let case_id = EvalCaseId::parse(&case.id).unwrap();
            let start = std::time::Instant::now();

            let corpus_split = match case.split {
                ClaimCorpusSplit::Development => CorpusSplit::Development,
                ClaimCorpusSplit::Test => CorpusSplit::Test,
            };

            let service = test_support::make_service().await;

            let mut setup_failed = false;
            let mut setup_error = String::new();
            for setup in &case.setup {
                if let Err(err) = ingest_and_extract(
                    &service,
                    &IngestParams {
                        scope: &setup.scope,
                        project: setup.project.as_deref(),
                        source_type: &setup.source_type,
                        source_id: &setup.source_id,
                        content: &setup.content,
                        t_ref: &setup.t_ref,
                        policy_tags: &setup.policy_tags,
                    },
                )
                .await
                {
                    setup_failed = true;
                    setup_error = format!(
                        "setup {source_id} failed: {err}",
                        source_id = setup.source_id
                    );
                    break;
                }
            }

            if setup_failed {
                outcomes.push(EvalCaseOutcome {
                    case_key: CaseKey::parse("claim-reconciliation", case_id.as_str()).unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: corpus_split,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(setup_error),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                });
                continue;
            }

            let extraction = match ingest_and_extract(
                &service,
                &IngestParams {
                    scope: &case.source.scope,
                    project: case.source.project.as_deref(),
                    source_type: &case.source.source_type,
                    source_id: &case.source.source_id,
                    content: &case.source.content,
                    t_ref: &case.source.t_ref,
                    policy_tags: &case.source.policy_tags,
                },
            )
            .await
            {
                Ok(result) => result,
                Err(err) => {
                    outcomes.push(EvalCaseOutcome {
                        case_key: CaseKey::parse("claim-reconciliation", case_id.as_str()).unwrap(),
                        mode: EvalMode::EndToEnd,
                        split: corpus_split,
                        label_trust: LabelTrust::Official,
                        status: CaseStatus::Invalid,
                        metrics: std::collections::BTreeMap::new(),
                        evidence: std::collections::BTreeMap::new(),
                        invalid_reason: Some(format!("source extraction failed: {err}")),
                        failures: vec![],
                        duration_ms: start.elapsed().as_millis() as u64,
                        attempts: 1,
                    });
                    continue;
                }
            };

            let metrics_result = evaluate_case(case, &extraction);

            let mut metric_map = std::collections::BTreeMap::new();
            metric_map.insert(
                "expected_contradictions".into(),
                metrics_result.expected_contradictions as f64,
            );
            metric_map.insert(
                "matched_warnings".into(),
                metrics_result.matched_warnings as f64,
            );
            metric_map.insert(
                "predicted_warnings".into(),
                metrics_result.predicted_warnings as f64,
            );
            metric_map.insert(
                "isolation_violations".into(),
                metrics_result.isolation_violations as f64,
            );

            let precision = if metrics_result.predicted_warnings == 0 {
                if metrics_result.expected_contradictions == 0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                metrics_result.matched_warnings as f64 / metrics_result.predicted_warnings as f64
            };
            let recall = if metrics_result.expected_contradictions == 0 {
                1.0
            } else {
                metrics_result.matched_warnings as f64
                    / metrics_result.expected_contradictions as f64
            };

            metric_map.insert("claim_precision".into(), precision);
            metric_map.insert("claim_recall".into(), recall);

            let case_passed = metrics_result.isolation_violations == 0;

            outcomes.push(EvalCaseOutcome {
                case_key: CaseKey::parse("claim-reconciliation", case_id.as_str()).unwrap(),
                mode: EvalMode::EndToEnd,
                split: corpus_split,
                label_trust: LabelTrust::Official,
                status: if case_passed {
                    CaseStatus::Passed
                } else {
                    CaseStatus::QualityFailed
                },
                metrics: metric_map,
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: None,
                failures: if !case_passed {
                    vec![format!(
                        "isolation_violations={}",
                        metrics_result.isolation_violations
                    )]
                } else {
                    vec![]
                },
                duration_ms: start.elapsed().as_millis() as u64,
                attempts: 1,
            });
        }

        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_substring_is_not_an_exact_match() {
        assert!(!warning_matches_exact(
            &ExpectedRelation {
                setup_source_id: "source:12".into(),
                source_id: "source:99".into(),
                outcome: "contradiction".into(),
                reason_code: "same_slot".into(),
                predecessor_source_id: None,
                successor_source_id: None,
            },
            &ContradictionWarning {
                fact_type: "metric".into(),
                existing_content: "old".into(),
                new_content: "new".into(),
                new_fact_id: "source:123".into(),
                conflicting_fact_id: "source:99".into(),
                entity_ids: vec![],
                reason: "test".into(),
            }
        ));
    }

    #[test]
    fn invalid_reference_time_invalidates_the_case() {
        assert!(parse_reference_time("not-a-time").is_err());
    }

    #[test]
    fn expected_isolation_skip_is_not_an_observed_violation() {
        let violations = count_isolation_violations(
            &["not_same_slot".into()],
            &[ContradictionWarning {
                fact_type: "not_same_slot".into(),
                existing_content: "old".into(),
                new_content: "new".into(),
                new_fact_id: "f1".into(),
                conflicting_fact_id: "f2".into(),
                entity_ids: vec![],
                reason: "test".into(),
            }],
        );
        assert_eq!(violations, 0);
    }

    #[test]
    fn fixture_loads() {
        let cases = load_cases().unwrap();
        assert!(!cases.is_empty());
    }

    #[test]
    fn covers_all_schema_outcomes_and_isolation_boundaries() {
        let cases = load_cases().unwrap();
        let mut schemas = BTreeSet::new();
        let mut outcomes = BTreeSet::new();
        let mut has_dev = false;
        let mut has_test = false;

        for case in &cases {
            for claim in &case.expected.claims {
                schemas.insert(claim.schema.clone());
            }
            for relation in &case.expected.relations {
                outcomes.insert(relation.outcome.clone());
            }
            match case.split {
                ClaimCorpusSplit::Development => has_dev = true,
                ClaimCorpusSplit::Test => has_test = true,
            }
        }

        assert!(schemas.contains("attribute/v1"));
        assert!(schemas.contains("quantity/v1"));
        assert!(schemas.contains("relation/v1"));
        assert!(schemas.contains("commitment/v1"));
        assert!(outcomes.contains("duplicate"));
        assert!(outcomes.contains("contradiction"));
        assert!(outcomes.contains("coexistence"));
        assert!(outcomes.contains("supersession"));
        assert!(outcomes.contains("correction"));
        assert!(has_dev);
        assert!(has_test);
    }
}
