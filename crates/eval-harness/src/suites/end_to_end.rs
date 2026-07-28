use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

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
        Self {
            expected_ids: vec![],
        }
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
        let service = test_support::make_service().await;

        let test_content = "The quarterly revenue report shows $5.2M in ARR.";
        let episode_id = match service
            .ingest(
                memory_mcp::models::IngestRequest {
                    source_type: "test".into(),
                    source_id: "e2e-test-001".into(),
                    content: test_content.into(),
                    t_ref: chrono::Utc::now(),
                    scope: "org".into(),
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
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", "e2e-ingest-failure").unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("ingest failed: {err}")),
                    failures: vec![],
                    duration_ms: 0,
                    attempts: 1,
                }];
            }
        };

        let _extraction = match service.extract(&episode_id, None, None).await {
            Ok(result) => result,
            Err(err) => {
                return vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", "e2e-extraction-failure").unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("extraction failed: {err}")),
                    failures: vec![],
                    duration_ms: 0,
                    attempts: 1,
                }];
            }
        };

        let start = std::time::Instant::now();
        let context_result = service
            .assemble_context(memory_mcp::models::AssembleContextRequest {
                query: "quarterly revenue".into(),
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

        let duration_ms = start.elapsed().as_millis() as u64;

        match context_result {
            Ok(items) => {
                let mut metrics = std::collections::BTreeMap::new();
                metrics.insert("context_items_returned".into(), items.len() as f64);

                vec![EvalCaseOutcome {
                    case_key: CaseKey::parse("end-to-end", "e2e-pipeline-completes").unwrap(),
                    mode: EvalMode::EndToEnd,
                    split: CorpusSplit::Development,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Passed,
                    metrics,
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: None,
                    failures: vec![],
                    duration_ms,
                    attempts: 1,
                }]
            }
            Err(err) => vec![EvalCaseOutcome {
                case_key: CaseKey::parse("end-to-end", "e2e-pipeline-fails").unwrap(),
                mode: EvalMode::EndToEnd,
                split: CorpusSplit::Development,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("assemble_context failed: {err}")),
                failures: vec![],
                duration_ms,
                attempts: 1,
            }],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn e2e_pipeline_completes() {
        let suite = EndToEndSuite::new();
        let context = RunContext {
            profile: EvalProfile::Nightly,
        };
        let outcomes = suite.run(&context).await;
        assert_eq!(outcomes.len(), 1);
        assert_eq!(outcomes[0].status, CaseStatus::Passed);
    }
}
