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
    #[allow(dead_code)]
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
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/claim_reconciliation_cases.json")
}

fn load_cases() -> Result<Vec<ClaimCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

#[cfg(test)]
fn warning_matches_exact(expected: &ExpectedRelation, actual: &ContradictionWarning) -> bool {
    actual.new_fact_id == expected.source_id
        || actual.conflicting_fact_id == expected.setup_source_id
}

fn parse_reference_time(raw: &str) -> Result<DateTime<Utc>, EvalError> {
    raw.parse::<DateTime<Utc>>()
        .map_err(|e| EvalError::InvalidInput(format!("invalid reference time '{raw}': {e}")))
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BoundaryKey {
    scope: String,
    project: Option<String>,
    policy_tags: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum WarningLabel {
    TruePositive,
    FalsePositive,
    IsolationViolation,
    UnresolvedLineage,
}

fn classify_warning(
    expected_relations: &[ExpectedRelation],
    lineage: &std::collections::BTreeMap<String, BTreeSet<String>>,
    boundaries: &std::collections::BTreeMap<String, BoundaryKey>,
    warning: &ContradictionWarning,
) -> WarningLabel {
    // Boundary ownership must come from the exact persisted fact ID.  Guessing
    // from the first boundary would turn missing lineage into a false pass or
    // false isolation violation.
    let left_boundary = boundaries.get(&warning.conflicting_fact_id);
    let right_boundary = boundaries.get(&warning.new_fact_id);

    if let (Some(left), Some(right)) = (left_boundary, right_boundary)
        && left != right
    {
        return WarningLabel::IsolationViolation;
    }

    if left_boundary.is_none() || right_boundary.is_none() {
        return WarningLabel::UnresolvedLineage;
    }

    let mut is_expected = false;
    for expected in expected_relations {
        if expected.outcome != "contradiction" {
            continue;
        }
        if matches_expected_relation_by_lineage(expected, warning, lineage) {
            is_expected = true;
            break;
        }
    }

    if is_expected {
        WarningLabel::TruePositive
    } else {
        WarningLabel::FalsePositive
    }
}

fn count_isolation_violations(
    expected_relations: &[ExpectedRelation],
    lineage: &std::collections::BTreeMap<String, BTreeSet<String>>,
    boundaries: &std::collections::BTreeMap<String, BoundaryKey>,
    actual_relations: &[ContradictionWarning],
) -> usize {
    actual_relations
        .iter()
        .filter(|w| {
            classify_warning(expected_relations, lineage, boundaries, w)
                == WarningLabel::IsolationViolation
        })
        .count()
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
) -> Result<(memory_mcp::models::ExtractResult, ExtractedSource), EvalError> {
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
                t_ingested: Some(t_ref_datetime),
                visibility_scope: None,
                policy_tags: params.policy_tags.to_vec(),
            },
            None,
        )
        .await
        .map_err(|e| EvalError::Suite(format!("ingest failed for {}: {e}", params.source_id)))?;

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .map_err(|e| EvalError::Suite(format!("extract failed for {}: {e}", params.source_id)))?;

    let fact_ids: BTreeSet<String> = extraction.facts.iter().map(|f| f.fact_id.clone()).collect();

    let extracted = ExtractedSource {
        source_id: params.source_id.to_string(),
        episode_id,
        fact_ids,
    };

    Ok((extraction, extracted))
}

struct ExtractedSource {
    source_id: String,
    #[allow(dead_code)]
    episode_id: String,
    fact_ids: BTreeSet<String>,
}

struct CaseMetrics {
    expected_contradictions: usize,
    matched_warnings: usize,
    predicted_warnings: usize,
    isolation_violations: usize,
    unresolved_lineage: usize,
}

fn matches_expected_relation_by_lineage(
    expected: &ExpectedRelation,
    actual: &memory_mcp::models::ContradictionWarning,
    lineage: &std::collections::BTreeMap<String, BTreeSet<String>>,
) -> bool {
    let setup_facts = lineage.get(&expected.setup_source_id);
    let source_facts = lineage.get(&expected.source_id);

    let setup_matches = setup_facts
        .map(|f| f.contains(&actual.conflicting_fact_id))
        .unwrap_or(false);
    let source_matches = source_facts
        .map(|f| f.contains(&actual.new_fact_id))
        .unwrap_or(false);

    if setup_matches && source_matches {
        return true;
    }

    let setup_matches_rev = setup_facts
        .map(|f| f.contains(&actual.new_fact_id))
        .unwrap_or(false);
    let source_matches_rev = source_facts
        .map(|f| f.contains(&actual.conflicting_fact_id))
        .unwrap_or(false);

    setup_matches_rev && source_matches_rev
}

fn evaluate_case(
    case: &ClaimCase,
    extraction: &memory_mcp::models::ExtractResult,
    lineage: &std::collections::BTreeMap<String, BTreeSet<String>>,
    boundaries: &std::collections::BTreeMap<String, BoundaryKey>,
) -> CaseMetrics {
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
                .any(|actual| matches_expected_relation_by_lineage(expected, actual, lineage))
        })
        .count();

    let isolation_violations = count_isolation_violations(
        &case.expected.relations,
        lineage,
        boundaries,
        &extraction.warnings,
    );
    let unresolved_lineage = extraction
        .warnings
        .iter()
        .filter(|warning| {
            classify_warning(&case.expected.relations, lineage, boundaries, warning)
                == WarningLabel::UnresolvedLineage
        })
        .count();

    CaseMetrics {
        expected_contradictions,
        matched_warnings,
        predicted_warnings: extraction.warnings.len(),
        isolation_violations,
        unresolved_lineage,
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

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
        *R.get_or_init(|| {
            &*Box::leak(Box::new(crate::reducer::ClassificationReducer::new(
                "claim-reconciliation",
                "claim",
            )))
        })
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
            let mut lineage: std::collections::BTreeMap<String, BTreeSet<String>> =
                std::collections::BTreeMap::new();
            for setup in &case.setup {
                match ingest_and_extract(
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
                    Ok((_extraction, extracted)) => {
                        lineage.insert(extracted.source_id, extracted.fact_ids);
                    }
                    Err(err) => {
                        setup_failed = true;
                        setup_error = format!(
                            "setup {source_id} failed: {err}",
                            source_id = setup.source_id
                        );
                        break;
                    }
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

            let (extraction, source_extracted) = match ingest_and_extract(
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

            lineage.insert(
                source_extracted.source_id.clone(),
                source_extracted.fact_ids.clone(),
            );

            let mut boundaries: std::collections::BTreeMap<String, BoundaryKey> =
                std::collections::BTreeMap::new();
            for setup in &case.setup {
                let key = BoundaryKey {
                    scope: setup.scope.clone(),
                    project: setup.project.clone(),
                    policy_tags: setup.policy_tags.iter().cloned().collect(),
                };
                if let Some(facts) = lineage.get(&setup.source_id) {
                    for fact_id in facts {
                        boundaries.insert(fact_id.clone(), key.clone());
                    }
                }
            }
            let source_key = BoundaryKey {
                scope: case.source.scope.clone(),
                project: case.source.project.clone(),
                policy_tags: case.source.policy_tags.iter().cloned().collect(),
            };
            for fact_id in &source_extracted.fact_ids {
                boundaries.insert(fact_id.clone(), source_key.clone());
            }

            let metrics_result = evaluate_case(case, &extraction, &lineage, &boundaries);

            let mut metric_map = std::collections::BTreeMap::new();
            // Diagnostic-only counts (not gate-consumed): expected/matched/
            // predicted warnings, isolation violations, unresolved lineage.
            // They explain *why* a case failed and do not share a formula
            // with any gate metric, so they stay as explicit diagnostics.
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
            metric_map.insert(
                "unresolved_lineage".into(),
                metrics_result.unresolved_lineage as f64,
            );

            let exact_claim_quality = metrics_result.matched_warnings
                == metrics_result.expected_contradictions
                && metrics_result.predicted_warnings == metrics_result.expected_contradictions;
            let case_invalid = metrics_result.unresolved_lineage > 0;
            let case_passed =
                !case_invalid && metrics_result.isolation_violations == 0 && exact_claim_quality;

            let tp = metrics_result.matched_warnings as u64;
            let fp = (metrics_result.predicted_warnings as u64).saturating_sub(tp);
            let fn_ = (metrics_result.expected_contradictions as u64).saturating_sub(tp);
            let tn = 0u64;

            let classification = MetricEvidence::classification(tp, fp, fn_, tn);
            // Per-case diagnostics stay as counts only. Gate metric keys
            // (`claim_precision`, `claim_recall`) belong to the reducer
            // surface that aggregates evidence across all cases in this
            // suite; per ADR-0025 they are not rendered per-case here.

            let mut evidence_map = std::collections::BTreeMap::new();
            if metrics_result.expected_contradictions > 0 || metrics_result.predicted_warnings > 0 {
                evidence_map.insert("classification".to_string(), classification);
            }

            outcomes.push(EvalCaseOutcome {
                case_key: CaseKey::parse("claim-reconciliation", case_id.as_str()).unwrap(),
                mode: EvalMode::EndToEnd,
                split: corpus_split,
                label_trust: LabelTrust::Official,
                status: if case_invalid {
                    CaseStatus::Invalid
                } else if case_passed {
                    CaseStatus::Passed
                } else {
                    CaseStatus::QualityFailed
                },
                metrics: metric_map,
                evidence: evidence_map,
                invalid_reason: case_invalid.then(|| {
                    format!(
                        "unresolved claim lineage for {} warning(s)",
                        metrics_result.unresolved_lineage
                    )
                }),
                failures: if !case_passed && !case_invalid {
                    vec![format!(
                        "claim_quality mismatch: expected={}, matched={}, predicted={}, isolation_violations={}",
                        metrics_result.expected_contradictions,
                        metrics_result.matched_warnings,
                        metrics_result.predicted_warnings,
                        metrics_result.isolation_violations,
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
    fn lineage_matches_expected_relation_by_fact_ids() {
        let mut lineage = std::collections::BTreeMap::new();
        lineage.insert("setup-1".into(), ["fact:old".into()].into_iter().collect());
        lineage.insert("source-1".into(), ["fact:new".into()].into_iter().collect());

        let expected = ExpectedRelation {
            setup_source_id: "setup-1".into(),
            source_id: "source-1".into(),
            outcome: "contradiction".into(),
            reason_code: "same_slot".into(),
            predecessor_source_id: None,
            successor_source_id: None,
        };

        let actual = ContradictionWarning {
            fact_type: "metric".into(),
            existing_content: "old".into(),
            new_content: "new".into(),
            new_fact_id: "fact:new".into(),
            conflicting_fact_id: "fact:old".into(),
            entity_ids: vec![],
            reason: "test".into(),
        };

        assert!(matches_expected_relation_by_lineage(
            &expected, &actual, &lineage
        ));
    }

    #[test]
    fn lineage_rejects_mismatched_fact_ids() {
        let mut lineage = std::collections::BTreeMap::new();
        lineage.insert("setup-1".into(), ["fact:old".into()].into_iter().collect());
        lineage.insert("source-1".into(), ["fact:new".into()].into_iter().collect());

        let expected = ExpectedRelation {
            setup_source_id: "setup-1".into(),
            source_id: "source-1".into(),
            outcome: "contradiction".into(),
            reason_code: "same_slot".into(),
            predecessor_source_id: None,
            successor_source_id: None,
        };

        let actual = ContradictionWarning {
            fact_type: "metric".into(),
            existing_content: "old".into(),
            new_content: "new".into(),
            new_fact_id: "fact:wrong".into(),
            conflicting_fact_id: "fact:old".into(),
            entity_ids: vec![],
            reason: "test".into(),
        };

        assert!(!matches_expected_relation_by_lineage(
            &expected, &actual, &lineage
        ));
    }

    #[test]
    fn invalid_reference_time_invalidates_the_case() {
        assert!(parse_reference_time("not-a-time").is_err());
    }

    #[test]
    fn same_boundary_warning_is_not_an_isolation_violation() {
        let violations = count_isolation_violations(
            &expected_relation(),
            &same_boundary_lineage(),
            &same_boundary_boundaries(),
            &[test_warning()],
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

    fn same_boundary_lineage() -> std::collections::BTreeMap<String, BTreeSet<String>> {
        let mut m = std::collections::BTreeMap::new();
        m.insert("setup-1".into(), ["fact:old".into()].into_iter().collect());
        m.insert("source-1".into(), ["fact:new".into()].into_iter().collect());
        m
    }

    fn same_boundary_boundaries() -> std::collections::BTreeMap<String, BoundaryKey> {
        let key = BoundaryKey {
            scope: "org".into(),
            project: Some("proj-a".into()),
            policy_tags: BTreeSet::new(),
        };
        let mut m = std::collections::BTreeMap::new();
        m.insert("fact:old".into(), key.clone());
        m.insert("fact:new".into(), key);
        m
    }

    fn cross_project_boundaries() -> std::collections::BTreeMap<String, BoundaryKey> {
        let mut m = std::collections::BTreeMap::new();
        m.insert(
            "fact:old".into(),
            BoundaryKey {
                scope: "org".into(),
                project: Some("proj-a".into()),
                policy_tags: BTreeSet::new(),
            },
        );
        m.insert(
            "fact:new".into(),
            BoundaryKey {
                scope: "org".into(),
                project: Some("proj-b".into()),
                policy_tags: BTreeSet::new(),
            },
        );
        m
    }

    fn test_warning() -> ContradictionWarning {
        ContradictionWarning {
            fact_type: "metric".into(),
            existing_content: "old".into(),
            new_content: "new".into(),
            new_fact_id: "fact:new".into(),
            conflicting_fact_id: "fact:old".into(),
            entity_ids: vec![],
            reason: "test".into(),
        }
    }

    fn expected_relation() -> Vec<ExpectedRelation> {
        vec![ExpectedRelation {
            setup_source_id: "setup-1".into(),
            source_id: "source-1".into(),
            outcome: "contradiction".into(),
            reason_code: "same_slot".into(),
            predecessor_source_id: None,
            successor_source_id: None,
        }]
    }

    #[test]
    fn expected_same_boundary_relation_is_true_positive() {
        let label = classify_warning(
            &expected_relation(),
            &same_boundary_lineage(),
            &same_boundary_boundaries(),
            &test_warning(),
        );
        assert_eq!(label, WarningLabel::TruePositive);
    }

    #[test]
    fn warning_crossing_project_boundary_is_isolation_violation() {
        let label = classify_warning(
            &[],
            &same_boundary_lineage(),
            &cross_project_boundaries(),
            &test_warning(),
        );
        assert_eq!(label, WarningLabel::IsolationViolation);
    }

    #[test]
    fn unexpected_same_boundary_warning_is_false_positive() {
        let label = classify_warning(
            &[],
            &same_boundary_lineage(),
            &same_boundary_boundaries(),
            &test_warning(),
        );
        assert_eq!(label, WarningLabel::FalsePositive);
    }
}
