use std::path::PathBuf;

use async_trait::async_trait;
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Deserialize)]
struct EndToEndCase {
    id: String,
    #[allow(dead_code)]
    description: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    sources: Vec<SourceSpec>,
    query: String,
    #[serde(default)]
    expected_entities: Vec<EntityExpectation>,
    #[serde(default)]
    expected_context: Vec<ContextExpectation>,
    min_context_items: usize,
    #[allow(dead_code)]
    label_trust: String,
}

#[derive(Debug, Deserialize)]
struct EntityExpectation {
    canonical_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    entity_type: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ContextExpectation {
    content_contains: String,
}

#[derive(Debug, Deserialize)]
struct SourceSpec {
    source_type: String,
    source_id: String,
    content: String,
}

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/evals/end_to_end_cases.json")
}

fn load_cases() -> Result<Vec<EndToEndCase>, EvalError> {
    let raw = std::fs::read_to_string(fixture_path()).map_err(|source| EvalError::Io {
        path: fixture_path(),
        source,
    })?;
    serde_json::from_str(&raw).map_err(EvalError::Artifact)
}

pub struct EndToEndSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl Default for EndToEndSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl EndToEndSuite {
    pub fn new() -> Self {
        let expected_ids = load_cases()
            .unwrap_or_default()
            .iter()
            .filter_map(|c| EvalCaseId::parse(&c.id).ok())
            .collect();
        Self { expected_ids }
    }
}

#[async_trait]
impl EvalSuite for EndToEndSuite {
    fn id(&self) -> &str {
        "end-to-end"
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
        *R.get_or_init(|| &*Box::leak(Box::new(crate::reducer::CountReducer::new("end-to-end"))))
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let cases = match load_cases() {
            Ok(cases) => cases,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", "fixture-load-error").unwrap(),
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
            outcomes.push(run_e2e_case(case).await);
        }
        outcomes
    }
}

async fn run_e2e_case(case: &EndToEndCase) -> EvalCaseOutcome {
    let start = std::time::Instant::now();
    let service = test_support::make_service().await;

    let mut all_entities: Vec<String> = Vec::new();

    for source in &case.sources {
        let episode_id = match service
            .ingest(
                memory_mcp::models::IngestRequest {
                    source_type: source.source_type.clone(),
                    source_id: source.source_id.clone(),
                    content: source.content.clone(),
                    t_ref: chrono::Utc::now(),
                    scope: case.scope.clone(),
                    project: case.project.clone(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
        {
            Ok(id) => id,
            Err(err) => {
                return EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("ingest failed: {err}")),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                };
            }
        };

        match service.extract(&episode_id, None, None).await {
            Ok(extraction) => {
                all_entities.extend(extraction.entities.iter().map(|e| e.canonical_name.clone()));
            }
            Err(err) => {
                return EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
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
        }
    }

    let query_start = std::time::Instant::now();
    let as_of = "2026-07-15T14:00:01Z"
        .parse::<chrono::DateTime<chrono::Utc>>()
        .unwrap();
    let context_result = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: case.query.clone(),
            scope: case.scope.clone(),
            as_of: Some(as_of),
            budget: 5,
            project: case.project.clone(),
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await;

    let query_ms = query_start.elapsed().as_millis() as u64;

    match context_result {
        Ok(items) => {
            let mut failures = Vec::new();

            let mut metrics = std::collections::BTreeMap::new();
            metrics.insert("context_items_returned".into(), items.len() as f64);

            let mut entity_tp = 0u64;
            let mut entity_fn = 0u64;
            for expected in &case.expected_entities {
                let matched = all_entities
                    .iter()
                    .any(|e| e.to_lowercase() == expected.canonical_name.to_lowercase());
                if matched {
                    entity_tp += 1;
                } else {
                    entity_fn += 1;
                    failures.push(format!("missing entity: {}", expected.canonical_name));
                }
            }
            let entity_fp = (all_entities.len() as u64).saturating_sub(entity_tp);

            if !case.expected_entities.is_empty() {
                metrics.insert("entity_tp".into(), entity_tp as f64);
                metrics.insert("entity_fp".into(), entity_fp as f64);
                metrics.insert("entity_fn".into(), entity_fn as f64);
            }

            let mut evidence_map = std::collections::BTreeMap::new();
            if !case.expected_entities.is_empty() || !all_entities.is_empty() {
                evidence_map.insert(
                    "classification".to_string(),
                    MetricEvidence::classification(entity_tp, entity_fp, entity_fn, 0),
                );
            }

            let context_matched = case
                .expected_context
                .iter()
                .filter(|exp| {
                    items
                        .iter()
                        .any(|item| item.content.contains(&exp.content_contains))
                })
                .count();

            let context_all_matched = context_matched == case.expected_context.len();
            let enough_items = items.len() >= case.min_context_items;

            if !context_all_matched {
                failures.push(format!(
                    "context: {}/{}",
                    context_matched,
                    case.expected_context.len()
                ));
            }
            if !enough_items {
                failures.push(format!(
                    "context_items: {} < {}",
                    items.len(),
                    case.min_context_items
                ));
            }

            let all_passed = entity_fn == 0 && context_all_matched && enough_items;

            EvalCaseOutcome {
                case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
                mode: EvalMode::EndToEnd,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: if all_passed {
                    CaseStatus::Passed
                } else {
                    CaseStatus::QualityFailed
                },
                metrics,
                evidence: evidence_map,
                invalid_reason: None,
                failures,
                duration_ms: query_ms,
                attempts: 1,
            }
        }
        Err(err) => EvalCaseOutcome {
            case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
            mode: EvalMode::EndToEnd,
            split: CorpusSplit::Development,
            label_trust: LabelTrust::Official,
            status: CaseStatus::Invalid,
            metrics: std::collections::BTreeMap::new(),
            evidence: std::collections::BTreeMap::new(),
            invalid_reason: Some(format!("assemble_context failed: {err}")),
            failures: vec![],
            duration_ms: query_ms,
            attempts: 1,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expected_ids_equal_all_fixture_ids() {
        let suite = EndToEndSuite::new();
        let cases = load_cases().unwrap();
        assert_eq!(suite.expected_case_ids().len(), cases.len());
    }

    #[tokio::test]
    async fn e2e_pipeline_completes() {
        let suite = EndToEndSuite::new();
        let context = RunContext {
            profile: EvalProfile::Nightly,
        };
        let outcomes = suite.run(&context).await;
        assert!(!outcomes.is_empty());
        assert!(outcomes.iter().all(|o| o.duration_ms > 0));
    }
}
