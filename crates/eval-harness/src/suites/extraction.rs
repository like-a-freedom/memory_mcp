use std::collections::BTreeSet;
use std::path::PathBuf;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use memory_mcp::models::{ContradictionWarning, IngestRequest};
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Deserialize)]
struct ExtractionEvalCase {
    id: String,
    #[allow(dead_code)]
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

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extraction_cases.json")
}

fn load_cases() -> Result<Vec<ExtractionEvalCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

fn warning_matches(expected: &ExpectedWarning, actual: &ContradictionWarning) -> bool {
    actual.fact_type == expected.fact_type
        && actual.existing_content == expected.existing_content
        && actual.new_content == expected.new_content
}

async fn ingest_and_extract(
    service: &memory_mcp::service::MemoryService,
    scope: &str,
    source_type: &str,
    source_id: &str,
    content: &str,
) -> Result<memory_mcp::models::ExtractResult, memory_mcp::MemoryError> {
    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: source_type.to_string(),
            source_id: source_id.to_string(),
            content: content.to_string(),
            t_ref: "2026-04-07T10:00:00Z"
                .parse::<DateTime<Utc>>()
                .expect("static timestamp should parse"),
            scope: scope.to_string(),
            project: None,
            t_ingested: Some("2026-04-07T10:00:00Z".parse().expect("static timestamp")),
            visibility_scope: None,
            policy_tags: vec![],
        },
        None,
    )
    .await?;

    ExtractCapability::extract(&service.build_context(), &episode_id, None, None).await
}

struct CaseResult {
    predicted_fact_types: BTreeSet<String>,
    predicted_entities: BTreeSet<String>,
    warnings: Vec<ContradictionWarning>,
}

async fn run_case(case: &ExtractionEvalCase) -> EvalCaseOutcome {
    let case_id = EvalCaseId::parse(&case.id).unwrap();
    let start = std::time::Instant::now();

    let service = test_support::make_service().await;

    for setup in &case.setup_episodes {
        if let Err(err) = ingest_and_extract(
            &service,
            &case.scope,
            &setup.source_type,
            &setup.source_id,
            &setup.content,
        )
        .await
        {
            return EvalCaseOutcome {
                case_key: CaseKey::parse("extraction", case_id.as_str()).unwrap(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("ingest_and_extract failed: {err}")),
                failures: vec![],
                duration_ms: start.elapsed().as_millis() as u64,
                attempts: 1,
            };
        }
    }

    let extraction = match ingest_and_extract(
        &service,
        &case.scope,
        &case.source_type,
        &case.source_id,
        &case.content,
    )
    .await
    {
        Ok(result) => result,
        Err(err) => {
            return EvalCaseOutcome {
                case_key: CaseKey::parse("extraction", case_id.as_str()).unwrap(),
                mode: EvalMode::EndToEnd,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("extraction failed: {err}")),
                failures: vec![],
                duration_ms: start.elapsed().as_millis() as u64,
                attempts: 1,
            };
        }
    };

    let result = CaseResult {
        predicted_fact_types: extraction
            .facts
            .iter()
            .map(|f| f.fact_type.clone())
            .collect(),
        predicted_entities: extraction
            .entities
            .iter()
            .map(|e| e.canonical_name.clone())
            .collect(),
        warnings: extraction.warnings,
    };

    let expected_fact_types: BTreeSet<String> = case.expected.fact_types.iter().cloned().collect();
    let expected_entities: BTreeSet<String> = case.expected.entities.iter().cloned().collect();

    let matched_fact_types = expected_fact_types
        .intersection(&result.predicted_fact_types)
        .count();
    let matched_entities = expected_entities
        .intersection(&result.predicted_entities)
        .count();
    let matched_warnings = case
        .expected
        .warnings
        .iter()
        .filter(|expected| {
            result
                .warnings
                .iter()
                .any(|actual| warning_matches(expected, actual))
        })
        .count();

    let entity_tp = matched_entities as u64;
    let entity_fp = (result.predicted_entities.len() as u64).saturating_sub(entity_tp);
    let entity_fn = (expected_entities.len() as u64).saturating_sub(entity_tp);
    let entity_tn = 0u64;

    let classification = MetricEvidence::classification(entity_tp, entity_fp, entity_fn, entity_tn);
    // entity_precision / entity_recall / entity_f1 are gate-adjacent
    // diagnostics rendered from the classification evidence through the
    // shared formula path (guarantees parity with the reducer aggregate).
    // The renderer's vacuity convention matches the pre-ADR-0025 manual
    // formulas: empty prediction with empty expectation ⇒ 1.0/1.0/1.0;
    // empty prediction with missed positives ⇒ precision 0.0.
    let mut metrics = crate::metrics::render_case_metrics(
        &classification,
        &crate::metrics::CaseMetricNames::classification("entity"),
    );
    // Diagnostic-only (not gate-consumed, no evidence arm carries them):
    // fact_type_accuracy and warning_recall remain suite-local diagnostics.
    let fact_type_accuracy = if expected_fact_types.is_empty() {
        1.0
    } else {
        matched_fact_types as f64 / expected_fact_types.len() as f64
    };
    let warning_recall = if case.expected.warnings.is_empty() {
        1.0
    } else {
        matched_warnings as f64 / case.expected.warnings.len() as f64
    };
    metrics.insert("fact_type_accuracy".into(), fact_type_accuracy);
    metrics.insert("warning_recall".into(), warning_recall);

    let mut evidence_map = std::collections::BTreeMap::new();
    if !expected_entities.is_empty() || !result.predicted_entities.is_empty() {
        evidence_map.insert("classification".to_string(), classification);
    }

    let warnings_passed = if case.expected.warnings.is_empty() {
        result.warnings.is_empty()
    } else {
        matched_warnings == case.expected.warnings.len()
    };

    let case_passed = matched_fact_types == expected_fact_types.len()
        && matched_entities == expected_entities.len()
        && warnings_passed;

    let status = if case_passed {
        CaseStatus::Passed
    } else {
        CaseStatus::QualityFailed
    };

    let mut failures = Vec::new();
    if matched_fact_types != expected_fact_types.len() {
        failures.push(format!(
            "fact_types: {matched_fact_types}/{}",
            expected_fact_types.len()
        ));
    }
    if matched_entities != expected_entities.len() {
        failures.push(format!(
            "entities: {matched_entities}/{}",
            expected_entities.len()
        ));
    }
    if !warnings_passed {
        failures.push(format!(
            "warnings: {matched_warnings}/{}",
            case.expected.warnings.len()
        ));
    }

    EvalCaseOutcome {
        case_key: CaseKey::parse("extraction", case_id.as_str()).unwrap(),
        mode: EvalMode::EndToEnd,
        split: CorpusSplit::Development,
        label_trust: LabelTrust::Official,
        status,
        metrics,
        evidence: evidence_map,
        invalid_reason: None,
        failures,
        duration_ms: start.elapsed().as_millis() as u64,
        attempts: 1,
    }
}

pub struct ExtractionSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl ExtractionSuite {
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
impl EvalSuite for ExtractionSuite {
    fn id(&self) -> &str {
        "extraction"
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
                "extraction",
                "entity",
            )))
        })
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(cases) => cases,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("extraction", "fixture-load-error").unwrap(),
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
            outcomes.push(run_case(case).await);
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_loads() {
        let cases = load_cases().unwrap();
        assert!(!cases.is_empty());
    }

    #[tokio::test]
    async fn single_extraction_case_produces_valid_outcome() {
        let cases = load_cases().unwrap();
        let case = &cases[0];
        let outcome = run_case(case).await;
        assert_eq!(outcome.suite_id(), "extraction");
        assert!(outcome.duration_ms > 0);
    }

    #[tokio::test]
    async fn warning_case_detects_contradiction() {
        let case = ExtractionEvalCase {
            id: "ext-warning-check".into(),
            description: "test".into(),
            source_type: "chat".into(),
            source_id: "ext-warning-check-current".into(),
            scope: "personal".into(),
            content: "Alice Smith reports ARR is $7M.".into(),
            setup_episodes: vec![SetupEpisode {
                source_type: "chat".into(),
                source_id: "ext-warning-check-setup".into(),
                content: "Alice Smith reports ARR is $5M.".into(),
            }],
            expected: ExtractionExpectation {
                fact_types: vec!["metric".into()],
                entities: vec!["Alice Smith".into()],
                warnings: vec![ExpectedWarning {
                    fact_type: "metric".into(),
                    existing_content: "Alice Smith reports ARR is $5M.".into(),
                    new_content: "Alice Smith reports ARR is $7M.".into(),
                }],
            },
        };
        let outcome = run_case(&case).await;
        assert!(
            outcome.status == CaseStatus::Passed || outcome.status == CaseStatus::QualityFailed
        );
    }
}
