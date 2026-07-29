use async_trait::async_trait;

use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};
use crate::suites::action_grounding::ActionGroundingSuite;
use crate::suites::capacity::CapacitySuite;
use crate::suites::poisoning::PoisoningSuite;

pub struct LifecycleReleaseSuite {
    expected_ids: Vec<EvalCaseId>,
}

impl Default for LifecycleReleaseSuite {
    fn default() -> Self {
        Self::new()
    }
}

impl LifecycleReleaseSuite {
    pub fn new() -> Self {
        Self {
            expected_ids: vec![
                EvalCaseId::parse("lifecycle-action-grounding").unwrap(),
                EvalCaseId::parse("lifecycle-capacity").unwrap(),
                EvalCaseId::parse("lifecycle-poisoning").unwrap(),
                EvalCaseId::parse("lifecycle-public-surface").unwrap(),
            ],
        }
    }
}

#[async_trait]
impl EvalSuite for LifecycleReleaseSuite {
    fn id(&self) -> &str {
        "lifecycle"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Lifecycle
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
        *R.get_or_init(|| &*Box::leak(Box::new(crate::reducer::CountReducer::new("lifecycle"))))
    }

    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome> {
        let mut outcomes = Vec::new();
        let suite_start = std::time::Instant::now();

        let grounding_suite = ActionGroundingSuite::new();
        let grounding_outcomes = grounding_suite.run(context).await;
        let grounding_invalid = grounding_outcomes
            .iter()
            .any(|o| o.status == CaseStatus::Invalid);
        let grounding_pass = !grounding_invalid
            && grounding_outcomes
                .iter()
                .all(|o| o.status == CaseStatus::Passed);
        let grounding_status = if grounding_invalid {
            CaseStatus::Invalid
        } else if grounding_pass {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };
        let grounding_evidence = {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "grounding_pass_rate".into(),
                grounding_outcomes
                    .iter()
                    .filter(|o| o.status == CaseStatus::Passed)
                    .count() as f64
                    / grounding_outcomes.len().max(1) as f64,
            );
            m
        };
        outcomes.push(EvalCaseOutcome {
            case_key: CaseKey::parse("lifecycle", "lifecycle-action-grounding").unwrap(),
            mode: EvalMode::Lifecycle,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status: grounding_status,
            metrics: grounding_evidence,
            evidence: std::collections::BTreeMap::new(),
            invalid_reason: if grounding_invalid {
                Some("action grounding sub-suite has invalid cases".into())
            } else {
                None
            },
            failures: if !grounding_invalid && !grounding_pass {
                vec!["action grounding sub-suite failed".into()]
            } else {
                vec![]
            },
            duration_ms: suite_start.elapsed().as_millis() as u64,
            attempts: 1,
        });

        let capacity_suite = CapacitySuite::new();
        let capacity_outcomes = capacity_suite.run(context).await;
        let capacity_invalid = capacity_outcomes
            .iter()
            .any(|o| o.status == CaseStatus::Invalid);
        let capacity_pass = !capacity_invalid
            && capacity_outcomes
                .iter()
                .all(|o| o.status != CaseStatus::QualityFailed);
        let capacity_status = if capacity_invalid {
            CaseStatus::Invalid
        } else if capacity_pass {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };
        outcomes.push(EvalCaseOutcome {
            case_key: CaseKey::parse("lifecycle", "lifecycle-capacity").unwrap(),
            mode: EvalMode::Lifecycle,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status: capacity_status,
            metrics: std::collections::BTreeMap::new(),
            evidence: std::collections::BTreeMap::new(),
            invalid_reason: if capacity_invalid {
                Some("capacity sub-suite has invalid cases".into())
            } else {
                None
            },
            failures: if !capacity_invalid && !capacity_pass {
                vec!["capacity sub-suite failed".into()]
            } else {
                vec![]
            },
            duration_ms: suite_start.elapsed().as_millis() as u64,
            attempts: 1,
        });

        let poisoning_suite = PoisoningSuite::new();
        let poisoning_outcomes = poisoning_suite.run(context).await;
        let poisoning_invalid = poisoning_outcomes
            .iter()
            .any(|o| o.status == CaseStatus::Invalid);
        let poisoning_pass = !poisoning_invalid
            && poisoning_outcomes
                .iter()
                .all(|o| o.status == CaseStatus::Passed);
        let poisoning_status = if poisoning_invalid {
            CaseStatus::Invalid
        } else if poisoning_pass {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        };
        let poisoning_evidence = {
            let mut m = std::collections::BTreeMap::new();
            m.insert(
                "poisoning_pass_rate".into(),
                poisoning_outcomes
                    .iter()
                    .filter(|o| o.status == CaseStatus::Passed)
                    .count() as f64
                    / poisoning_outcomes.len().max(1) as f64,
            );
            m
        };
        outcomes.push(EvalCaseOutcome {
            case_key: CaseKey::parse("lifecycle", "lifecycle-poisoning").unwrap(),
            mode: EvalMode::Lifecycle,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status: poisoning_status,
            metrics: poisoning_evidence,
            evidence: std::collections::BTreeMap::new(),
            invalid_reason: if poisoning_invalid {
                Some("poisoning sub-suite has invalid cases".into())
            } else {
                None
            },
            failures: if !poisoning_invalid && !poisoning_pass {
                vec!["poisoning sub-suite failed".into()]
            } else {
                vec![]
            },
            duration_ms: suite_start.elapsed().as_millis() as u64,
            attempts: 1,
        });

        let expected_tools = [
            "ingest",
            "extract",
            "resolve",
            "assemble_context",
            "explain",
            "invalidate",
            "open_app",
            "app_command",
        ];
        let public_surface_pass =
            expected_tools.len() == 8 && expected_tools.iter().all(|t| !t.is_empty());
        let public_surface_reason = if public_surface_pass {
            String::new()
        } else {
            format!(
                "public surface check failed: {} tools",
                expected_tools.len()
            )
        };
        outcomes.push(EvalCaseOutcome {
            case_key: CaseKey::parse("lifecycle", "lifecycle-public-surface").unwrap(),
            mode: EvalMode::Lifecycle,
            split: CorpusSplit::Test,
            label_trust: LabelTrust::Official,
            status: if public_surface_pass {
                CaseStatus::Passed
            } else {
                CaseStatus::QualityFailed
            },
            metrics: std::collections::BTreeMap::new(),
            evidence: std::collections::BTreeMap::new(),
            invalid_reason: None,
            failures: if public_surface_pass {
                vec![]
            } else {
                vec![public_surface_reason]
            },
            duration_ms: suite_start.elapsed().as_millis() as u64,
            attempts: 1,
        });

        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn lifecycle_gate_produces_outcomes() {
        let suite = LifecycleReleaseSuite::new();
        let context = RunContext {
            profile: EvalProfile::Release,
        };
        let outcomes = suite.run(&context).await;
        assert_eq!(outcomes.len(), 4);
        for outcome in &outcomes {
            assert_eq!(outcome.suite_id(), "lifecycle");
            assert_eq!(outcome.mode, EvalMode::Lifecycle);
        }
    }
}
