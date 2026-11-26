use async_trait::async_trait;
use eval_harness::domain::*;
use eval_harness::reducer::{CountReducer, SuiteReducer};
use eval_harness::runner::{EvalSuite, RunContext};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

struct PartialSuite {
    ids: Vec<EvalCaseId>,
    reducer: CountReducer,
    dropped: Arc<AtomicBool>,
}
struct DropMarker(Arc<AtomicBool>);
impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
#[async_trait]
impl EvalSuite for PartialSuite {
    fn id(&self) -> &str {
        "partial"
    }
    fn mode(&self) -> EvalMode {
        EvalMode::Lifecycle
    }
    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.ids
    }
    fn reducer(&self) -> &dyn SuiteReducer {
        &self.reducer
    }
    async fn run(&self, context: &RunContext) -> Vec<EvalCaseOutcome> {
        context.begin_case("partial", "completed");
        let complete = context.completed(EvalCaseOutcome::new(
            "partial",
            "completed",
            EvalMode::Lifecycle,
            CorpusSplit::Test,
            LabelTrust::Reviewed,
            CaseStatus::Passed,
        ));
        context.begin_case("partial", "blocked");
        let _guard = DropMarker(self.dropped.clone());
        std::future::pending::<()>().await;
        vec![complete]
    }
}
#[tokio::test]
async fn same_suite_deadline_preserves_completed_running_and_unstarted_cases() {
    let dropped = Arc::new(AtomicBool::new(false));
    let suite = PartialSuite {
        ids: ["completed", "blocked", "unstarted"]
            .map(|id| EvalCaseId::parse(id).unwrap())
            .to_vec(),
        reducer: CountReducer::new("partial"),
        dropped: dropped.clone(),
    };
    let context = RunContext::new(EvalProfile::Nightly);
    let results = suite
        .run_with_deadline(&context, Instant::now() + Duration::from_millis(100))
        .await;
    assert_eq!(results.len(), 3);
    assert_eq!(results[0].status, CaseStatus::Passed);
    assert_eq!(results[1].status, CaseStatus::Invalid);
    assert_eq!(results[1].attempts, 1);
    assert_eq!(results[2].attempts, 0);
    assert!(
        dropped.load(Ordering::SeqCst),
        "owned work is dropped before returning"
    );
}

struct ExternalEvaluator {
    dropped: Arc<AtomicBool>,
}
#[async_trait]
impl eval_harness::suites::external_retrieval::ExternalCaseEvaluator for ExternalEvaluator {
    async fn evaluate(
        &self,
        case: &eval_harness::corpus::adapters::ExternalCase,
    ) -> EvalCaseOutcome {
        if case.id == "completed" {
            return EvalCaseOutcome::new(
                "external-retrieval",
                &case.id,
                EvalMode::RetrievalOnly,
                CorpusSplit::Test,
                LabelTrust::Weak,
                CaseStatus::Passed,
            );
        }
        let _guard = DropMarker(self.dropped.clone());
        std::future::pending::<EvalCaseOutcome>().await
    }
}
#[tokio::test]
async fn external_deadline_drops_workers_and_preserves_case_progress() {
    use eval_harness::corpus::adapters::{ExternalCase, RetrievalExpectation};
    use eval_harness::suites::external_retrieval::{ExternalRetrievalSuite, WorkerPolicy};
    let dropped = Arc::new(AtomicBool::new(false));
    let cases = ["completed", "blocked", "unstarted"]
        .map(|id| ExternalCase {
            id: id.into(),
            dataset: "test".into(),
            description: "controlled execution".into(),
            query: "test".into(),
            budget: 5,
            facts: vec![],
            expected: RetrievalExpectation {
                tier: "direct".into(),
                must_contain: vec!["test".into()],
                min_recall_at_k: 1.0,
            },
            metadata: serde_json::json!({}),
        })
        .to_vec();
    let suite = ExternalRetrievalSuite::new(cases)
        .with_worker_policy(WorkerPolicy {
            context_workers: std::num::NonZeroUsize::new(1).unwrap(),
        })
        .with_case_evaluator(Arc::new(ExternalEvaluator {
            dropped: dropped.clone(),
        }));
    let results = suite
        .run_with_deadline(
            &RunContext::new(EvalProfile::Nightly),
            Instant::now() + Duration::from_millis(100),
        )
        .await;
    assert_eq!(results[0].status, CaseStatus::Passed);
    assert_eq!(results[1].attempts, 1);
    assert_eq!(results[1].status, CaseStatus::Invalid);
    assert_eq!(results[2].attempts, 0);
    assert!(dropped.load(Ordering::SeqCst));
}
