use std::collections::BTreeMap;
use std::num::NonZeroUsize;

use async_trait::async_trait;
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;

use crate::corpus::adapters::ExternalCase;
use crate::domain::*;
use crate::runner::{EvalSuite, RunContext};
use crate::test_support;

pub static ANSWER_PROXY_SPECS: &[crate::reducer::RatioMetricSpec] =
    &[crate::reducer::RatioMetricSpec {
        evidence_key: "answer_presence_proxy",
        metric_name: "answer_presence_proxy_at_5",
    }];

// A lexical diagnostic, not evidence-document recall or answer correctness.
fn answer_proxy(items: &[String], answers: &[String]) -> Result<MetricEvidence, String> {
    fn tokens(text: &str) -> Vec<String> {
        text.split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .map(str::to_lowercase)
            .collect()
    }
    let answers: std::collections::BTreeSet<Vec<String>> =
        answers.iter().map(|a| tokens(a)).collect();
    if answers.is_empty() || answers.iter().any(Vec::is_empty) {
        return Err("unsupported answer-presence proxy: missing reference answer".into());
    }
    let items: Vec<_> = items.iter().take(5).map(|s| tokens(s)).collect();
    let hits = answers
        .iter()
        .filter(|answer| {
            items.iter().any(|item| {
                item.windows(answer.len())
                    .any(|window| window == answer.as_slice())
            })
        })
        .count();
    Ok(MetricEvidence::ratio(hits as u64, answers.len() as u64))
}

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
    reducer: crate::reducer::RatioReducer,
}

impl ExternalRetrievalSuite {
    pub fn new(dataset: crate::corpus::adapters::DatasetKind, cases: Vec<ExternalCase>) -> Self {
        let expected_ids: Vec<EvalCaseId> = cases
            .iter()
            .map(|c| {
                EvalCaseId::parse(&c.id)
                    .expect("external corpus is validated before suite construction")
            })
            .collect();
        Self {
            cases,
            expected_ids,
            dataset: dataset.dataset_name().to_string(),
            worker_policy: WorkerPolicy::default(),
            reducer: crate::reducer::RatioReducer::new("external-retrieval", ANSWER_PROXY_SPECS),
        }
    }

    pub fn with_worker_policy(mut self, policy: WorkerPolicy) -> Self {
        self.worker_policy = policy;
        self
    }

    async fn run_case(case: &ExternalCase) -> EvalCaseOutcome {
        let case_id = EvalCaseId::parse(&case.id).unwrap();
        let start = std::time::Instant::now();

        if let Err(reason) = answer_proxy(&[], &case.expected.must_contain) {
            let mut outcome = EvalCaseOutcome::new(
                "external-retrieval",
                case_id.as_str(),
                EvalMode::RetrievalOnly,
                CorpusSplit::Test,
                LabelTrust::Weak,
                CaseStatus::Invalid,
            );
            outcome.invalid_reason = Some(reason);
            return outcome;
        }

        let service = test_support::make_service().await;

        for fact in &case.facts {
            let t_valid = match fact.t_valid.parse::<chrono::DateTime<chrono::Utc>>() {
                Ok(t) => t,
                Err(err) => {
                    return EvalCaseOutcome {
                        case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                        mode: EvalMode::RetrievalOnly,
                        split: CorpusSplit::Test,
                        label_trust: LabelTrust::Weak,
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
            test_support::seed_fact_with_links(&service, &fact.content, t_valid, vec![], None)
                .await;
        }

        let start_query = std::time::Instant::now();
        let context_result = AssembleContextCapability::assemble_context(
            &service.build_context(),
            memory_mcp::models::AssembleContextRequest {
                query: case.query.clone(),
                as_of: Some(chrono::Utc::now()),
                budget: case.budget,
                fact_types: vec![],
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
                compact: false,
            },
        )
        .await;

        let query_ms = start_query.elapsed().as_millis() as u64;
        let total_ms = start.elapsed().as_millis() as u64;

        match context_result {
            Ok(items) => {
                let ranked_ids: Vec<String> = items.iter().map(|i| i.content.clone()).collect();
                let evidence = match answer_proxy(&ranked_ids, &case.expected.must_contain) {
                    Ok(evidence) => evidence,
                    Err(reason) => {
                        let mut outcome = EvalCaseOutcome::new(
                            "external-retrieval",
                            case_id.as_str(),
                            EvalMode::RetrievalOnly,
                            CorpusSplit::Test,
                            LabelTrust::Weak,
                            CaseStatus::Invalid,
                        );
                        outcome.invalid_reason = Some(reason);
                        return outcome;
                    }
                };
                let MetricEvidence::Ratio {
                    numerator,
                    denominator,
                } = &evidence
                else {
                    let mut outcome = EvalCaseOutcome::new(
                        "external-retrieval",
                        case_id.as_str(),
                        EvalMode::RetrievalOnly,
                        CorpusSplit::Test,
                        LabelTrust::Weak,
                        CaseStatus::Invalid,
                    );
                    outcome.invalid_reason =
                        Some("answer proxy produced non-ratio evidence".into());
                    return outcome;
                };
                let mut metric_map = BTreeMap::from([(
                    "answer_presence_proxy_at_5".into(),
                    *numerator as f64 / *denominator as f64,
                )]);
                metric_map.insert("query_ms".into(), query_ms as f64);

                EvalCaseOutcome {
                    case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                    mode: EvalMode::RetrievalOnly,
                    split: CorpusSplit::Test,
                    label_trust: LabelTrust::Weak,
                    status: CaseStatus::Passed,
                    metrics: metric_map,
                    evidence: [("answer_presence_proxy".to_string(), evidence)]
                        .into_iter()
                        .collect(),
                    invalid_reason: None,
                    failures: vec![],
                    duration_ms: total_ms,
                    attempts: 1,
                }
            }
            Err(err) => EvalCaseOutcome {
                case_key: CaseKey::parse("external-retrieval", case_id.as_str()).unwrap(),
                mode: EvalMode::RetrievalOnly,
                split: CorpusSplit::Test,
                label_trust: LabelTrust::Weak,
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
        &self.reducer
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
                        label_trust: LabelTrust::Weak,
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
    fn answer_proxy_matches_tokens_without_claiming_document_recall() {
        let evidence = answer_proxy(&["Alice prefers tea.".into()], &["tea".into()]).unwrap();
        assert_eq!(evidence, MetricEvidence::ratio(1, 1));
        assert_eq!(
            answer_proxy(&["a team".into()], &["tea".into()]).unwrap(),
            MetricEvidence::ratio(0, 1)
        );
        assert!(answer_proxy(&[], &[]).is_err());
        assert!(answer_proxy(&[], &[" ".into()]).is_err());
    }

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
