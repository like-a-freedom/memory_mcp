use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionOutcome {
    Correct { evidence_ids: Vec<String> },
    Incorrect { reason: String },
    Refused { reason: String },
}

pub struct ActionGroundingSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl Default for ActionGroundingSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl ActionGroundingSuite {
    pub fn new() -> Self {
        Self {
            expected_ids: vec![
                EvalCaseId::parse("grounding-always-recall").unwrap(),
                EvalCaseId::parse("grounding-selective-shadow").unwrap(),
                EvalCaseId::parse("grounding-selective-enforced").unwrap(),
            ],
        }
    }

    async fn run_case(case_id: &str, mode_name: &str) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(case_id).unwrap();
        let start = std::time::Instant::now();

        let service = test_support::make_service().await;

        let content = "The security review was approved by the CISO on March 15, 2026.";
        let episode_id = match service
            .ingest(
                memory_mcp::models::IngestRequest {
                    source_type: "email".into(),
                    source_id: format!("grounding-{mode_name}"),
                    content: content.into(),
                    t_ref: chrono::Utc::now(),
                    scope: "org".into(),
                    project: Some("security".into()),
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
                    case_id,
                    suite_id: "action-grounding".into(),
                    mode: EvalMode::Lifecycle,
                    split: CorpusSplit::Test,
                    label_trust: LabelTrust::Official,
                    status: CaseStatus::Invalid,
                    metrics: std::collections::BTreeMap::new(),
                    invalid_reason: Some(format!("ingest failed: {err}")),
                    failures: vec![],
                    duration_ms: start.elapsed().as_millis() as u64,
                    attempts: 1,
                };
            }
        };

        let _ = service.extract(&episode_id, None, None).await;

        let context_result = service
            .assemble_context(memory_mcp::models::AssembleContextRequest {
                query: "security review approval".into(),
                scope: "org".into(),
                as_of: Some(chrono::Utc::now()),
                budget: 5,
                project: Some("security".into()),
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
                let has_relevant = items.iter().any(|item| {
                    item.content.contains("CISO") || item.content.contains("security review")
                });
                let _evidence_ids: Vec<String> = items
                    .iter()
                    .filter(|item| {
                        item.content.contains("CISO") || item.content.contains("security review")
                    })
                    .map(|item| item.source_episode.clone())
                    .collect();

                let mut metrics = std::collections::BTreeMap::new();
                metrics.insert("recall_hit".into(), if has_relevant { 1.0 } else { 0.0 });
                metrics.insert("context_items".into(), items.len() as f64);

                EvalCaseOutcome {
                    case_id,
                    suite_id: "action-grounding".into(),
                    mode: EvalMode::Lifecycle,
                    split: CorpusSplit::Test,
                    label_trust: LabelTrust::Official,
                    status: if has_relevant {
                        CaseStatus::Passed
                    } else {
                        CaseStatus::QualityFailed
                    },
                    metrics,
                    invalid_reason: None,
                    failures: if !has_relevant {
                        vec!["no relevant context returned".into()]
                    } else {
                        vec![]
                    },
                    duration_ms,
                    attempts: 1,
                }
            }
            Err(err) => EvalCaseOutcome {
                case_id,
                suite_id: "action-grounding".into(),
                mode: EvalMode::Lifecycle,
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("assemble_context failed: {err}")),
                failures: vec![],
                duration_ms,
                attempts: 1,
            },
        }
    }
}

#[async_trait]
impl EvalSuite for ActionGroundingSuite {
    fn id(&self) -> &str {
        "action-grounding"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Lifecycle
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let mut outcomes = Vec::new();
        for (id, mode_name) in [
            ("grounding-always-recall", "always_recall"),
            ("grounding-selective-shadow", "selective_shadow"),
            ("grounding-selective-enforced", "selective_enforced"),
        ] {
            outcomes.push(Self::run_case(id, mode_name).await);
        }
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn action_grounding_produces_outcomes() {
        let suite = ActionGroundingSuite::new();
        let context = RunContext {
            profile: EvalProfile::Release,
        };
        let outcomes = suite.run(&context).await;
        assert_eq!(outcomes.len(), 3);
        for outcome in &outcomes {
            assert_eq!(outcome.suite_id, "action-grounding");
            assert!(outcome.duration_ms > 0 || outcome.status == CaseStatus::Invalid);
        }
    }
}
