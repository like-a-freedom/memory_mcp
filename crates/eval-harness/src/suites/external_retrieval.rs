use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use async_trait::async_trait;

use crate::corpus::adapters::ExternalCase;
use crate::domain::*;
use crate::metrics;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

pub struct WorkerPolicy {
    pub context_workers: NonZeroUsize,
    pub query_workers_per_context: NonZeroUsize,
}

impl Default for WorkerPolicy {
    fn default() -> Self {
        Self {
            context_workers: NonZeroUsize::new(3).unwrap(),
            query_workers_per_context: NonZeroUsize::new(4).unwrap(),
        }
    }
}

pub struct ExternalRetrievalSuite {
    cases: Vec<ExternalCase>,
    expected_ids: Vec<EvalCaseId>,
    #[allow(dead_code)]
    dataset: String,
    worker_policy: WorkerPolicy,
}

impl ExternalRetrievalSuite {
    pub fn new(dataset: crate::corpus::adapters::DatasetKind, cases: Vec<ExternalCase>) -> Self {
        let expected_ids: Vec<EvalCaseId> = cases
            .iter()
            .filter_map(|c| EvalCaseId::parse(&c.id).ok())
            .collect();
        Self {
            cases,
            expected_ids,
            dataset: dataset.dataset_name().to_string(),
            worker_policy: WorkerPolicy::default(),
        }
    }

    pub fn with_worker_policy(mut self, policy: WorkerPolicy) -> Self {
        self.worker_policy = policy;
        self
    }

    async fn run_case(case: &ExternalCase) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(&case.id).unwrap();
        let start = std::time::Instant::now();

        let service = test_support::make_service().await;

        for fact in &case.facts {
            let t_valid = match fact.t_valid.parse::<chrono::DateTime<chrono::Utc>>() {
                Ok(t) => t,
                Err(err) => {
                    return EvalCaseOutcome {
                        case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Test,
                        label_trust: LabelTrust::Official,
                        status: CaseStatus::Invalid,
                        metrics: BTreeMap::new(),
                        evidence: std::collections::BTreeMap::new(),
                        invalid_reason: Some(format!("invalid timestamp: {err}")),
                        failures: vec![],
                        duration_ms: start.elapsed().as_millis() as u64,
                        attempts: 1,
                    };
                }
            };
            test_support::seed_fact_with_links_and_project(
                &service,
                &case.scope,
                &fact.content,
                t_valid,
                vec![],
                None,
                None,
            )
            .await;
        }

        let start_query = std::time::Instant::now();
        let context_result = service
            .assemble_context(memory_mcp::models::AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: Some(chrono::Utc::now()),
                budget: case.budget,
                project: None,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await;

        let query_ms = start_query.elapsed().as_millis() as u64;
        let total_ms = start.elapsed().as_millis() as u64;

        match context_result {
            Ok(items) => {
                let ranked_ids: Vec<String> = items.iter().map(|i| i.content.clone()).collect();
                let relevant_ids: std::collections::BTreeSet<String> =
                    case.expected.must_contain.iter().cloned().collect();

                let mut metric_map = BTreeMap::new();
                if !relevant_ids.is_empty() && !ranked_ids.is_empty() {
                    let obs = metrics::RetrievalObservation {
                        relevant_ids: relevant_ids.clone(),
                        ranked_ids: ranked_ids.clone(),
                    };
                    let cutoff = NonZeroUsize::new(5).unwrap_or(NonZeroUsize::new(1).unwrap());
                    if let Ok(m) = metrics::retrieval_metrics(&[obs], cutoff) {
                        metric_map.insert("recall_at_5".into(), m.recall_at_k);
                        metric_map.insert("mrr".into(), m.mrr);
                        metric_map.insert("top_1_hit_rate".into(), m.top_1_hit_rate);
                    }
                }

                let recall = metric_map.get("recall_at_5").copied().unwrap_or(0.0);
                let meets_recall = recall >= case.expected.min_recall_at_k;
                metric_map.insert("query_ms".into(), query_ms as f64);

                EvalCaseOutcome {
                    case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Test,
                    label_trust: LabelTrust::Official,
                    status: if meets_recall {
                        CaseStatus::Passed
                    } else {
                        CaseStatus::QualityFailed
                    },
                    metrics: metric_map,
                    evidence: std::collections::BTreeMap::new(),
                    invalid_reason: None,
                    failures: if !meets_recall {
                        vec![format!(
                            "recall_at_5={recall:.4} < {}",
                            case.expected.min_recall_at_k
                        )]
                    } else {
                        vec![]
                    },
                    duration_ms: total_ms,
                    attempts: 1,
                }
            }
            Err(err) => EvalCaseOutcome {
                case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Official,
                status: CaseStatus::Invalid,
                metrics: BTreeMap::new(),
                evidence: std::collections::BTreeMap::new(),
                invalid_reason: Some(format!("assemble_context failed: {err}")),
                failures: vec![],
                duration_ms: total_ms,
                attempts: 1,
            },
        }
    }
}

#[async_trait]
impl EvalSuite for ExternalRetrievalSuite {
    fn id(&self) -> &str {
        "external-retrieval"
    }

    fn mode(&self) -> EvalMode {
        EvalMode::RetrievalOnly
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn crate::reducer::SuiteReducer {
        use std::sync::OnceLock;
        static R: OnceLock<&dyn crate::reducer::SuiteReducer> = OnceLock::new();
        *R.get_or_init(|| {
            &*Box::leak(Box::new(crate::reducer::CountReducer::new(
                "external-retrieval",
            )))
        })
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let mut outcomes = Vec::with_capacity(self.cases.len());

        let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(
            self.worker_policy.context_workers.get(),
        ));

        let mut handles = Vec::new();
        for case in &self.cases {
            let case = case.clone();
            let permit = semaphore.clone().acquire_owned().await.unwrap();
            handles.push(tokio::spawn(async move {
                let result = Self::run_case(&case).await;
                drop(permit);
                result
            }));
        }

        for handle in handles {
            match handle.await {
                Ok(outcome) => outcomes.push(outcome),
                Err(join_err) => {
                    let case_key = CaseKey::parse("external-retrieval", "join-error").unwrap();
                    outcomes.push(EvalCaseOutcome {
                        case_key,
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Test,
                        label_trust: LabelTrust::Official,
                        status: CaseStatus::Invalid,
                        metrics: BTreeMap::new(),
                        evidence: std::collections::BTreeMap::new(),
                        invalid_reason: Some(format!("task panicked: {join_err}")),
                        failures: vec![],
                        duration_ms: 0,
                        attempts: 1,
                    });
                }
            }
        }

        outcomes.sort_by(|a, b| a.case_id().cmp(b.case_id()));
        outcomes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_policy_default_values() {
        let policy = WorkerPolicy::default();
        assert_eq!(policy.context_workers.get(), 3);
        assert_eq!(policy.query_workers_per_context.get(), 4);
    }

    #[test]
    fn external_suite_expected_ids_equal_loaded_cases() {
        let cases = vec![
            ExternalCase {
                id: "ext-1".into(),
                dataset: "test".into(),
                description: "test".into(),
                query: "query".into(),
                scope: "org".into(),
                budget: 5,
                facts: vec![crate::corpus::adapters::SeedFact {
                    content: "fact".into(),
                    t_valid: "2026-01-01T00:00:00Z".into(),
                }],
                expected: crate::corpus::adapters::RetrievalExpectation {
                    tier: "direct".into(),
                    must_contain: vec!["fact".into()],
                    min_recall_at_k: 1.0,
                },
                metadata: serde_json::json!({}),
            },
            ExternalCase {
                id: "ext-2".into(),
                dataset: "test".into(),
                description: "test".into(),
                query: "query".into(),
                scope: "org".into(),
                budget: 5,
                facts: vec![crate::corpus::adapters::SeedFact {
                    content: "fact".into(),
                    t_valid: "2026-01-01T00:00:00Z".into(),
                }],
                expected: crate::corpus::adapters::RetrievalExpectation {
                    tier: "direct".into(),
                    must_contain: vec!["fact".into()],
                    min_recall_at_k: 1.0,
                },
                metadata: serde_json::json!({}),
            },
        ];
        let suite = ExternalRetrievalSuite::new(
            crate::corpus::adapters::DatasetKind::LongMemEvalCleaned,
            cases,
        );
        assert_eq!(suite.expected_case_ids().len(), 2);
    }

    #[test]
    fn external_case_construction() {
        let case = ExternalCase {
            id: "test:ext-1".into(),
            dataset: "test".into(),
            description: "test".into(),
            query: "test query".into(),
            scope: "org".into(),
            budget: 5,
            facts: vec![crate::corpus::adapters::SeedFact {
                content: "test fact".into(),
                t_valid: "2026-01-01T00:00:00Z".into(),
            }],
            expected: crate::corpus::adapters::RetrievalExpectation {
                tier: "direct".into(),
                must_contain: vec!["test".into()],
                min_recall_at_k: 1.0,
            },
            metadata: serde_json::json!({}),
        };
        assert_eq!(case.id, "test:ext-1");
    }
}
