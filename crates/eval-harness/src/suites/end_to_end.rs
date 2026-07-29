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
    sources: Vec<SourceSpec>,
    query: String,
    expected_evidence: Vec<String>,
    min_context_items: usize,
    #[allow(dead_code)]
    label_trust: String,
}

#[derive(Debug, Deserialize)]
struct SourceSpec {
    source_type: String,
    source_id: String,
    content: String,
    scope: String,
    t_ref: String,
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

    for source in &case.sources {
        let t_ref = match source.t_ref.parse::<chrono::DateTime<chrono::Utc>>() {
            Ok(t) => t,
            Err(err) => {
                return EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("invalid timestamp: {err}")),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                };
            }
        };

        let episode_id = match service
            .ingest(
                memory_mcp::models::IngestRequest {
                    source_type: source.source_type.clone(),
                    source_id: source.source_id.clone(),
                    content: source.content.clone(),
                    t_ref,
                    scope: source.scope.clone(),
                    project: None,
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

        if let Err(err) = service.extract(&episode_id, None, None).await {
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

    let query_start = std::time::Instant::now();
    let context_result = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: case.query.clone(),
            scope: "org".into(),
            as_of: Some(chrono::Utc::now()),
            budget: 5,
            project: None,
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
            let matched = case
                .expected_evidence
                .iter()
                .filter(|needle| {
                    items
                        .iter()
                        .any(|item| item.content.contains(needle.as_str()))
                })
                .count();

            let all_matched = matched == case.expected_evidence.len();
            let enough_items = items.len() >= case.min_context_items;

            let mut metrics = std::collections::BTreeMap::new();
            metrics.insert("context_items_returned".into(), items.len() as f64);
            metrics.insert("evidence_matched".into(), matched as f64);
            metrics.insert("evidence_total".into(), case.expected_evidence.len() as f64);

            let status = if all_matched && enough_items {
                CaseStatus::Passed
            } else {
                CaseStatus::QualityFailed
            };

            let mut failures = Vec::new();
            if !all_matched {
                failures.push(format!(
                    "evidence: {}/{}",
                    matched,
                    case.expected_evidence.len()
                ));
            }
            if !enough_items {
                failures.push(format!(
                    "context_items: {} < {}",
                    items.len(),
                    case.min_context_items
                ));
            }

            EvalCaseOutcome {
                case_key: CaseKey::parse("end-to-end", &case.id).unwrap(),
                mode: EvalMode::EndToEnd,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status,
                metrics,
                evidence: std::collections::BTreeMap::new(),
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
