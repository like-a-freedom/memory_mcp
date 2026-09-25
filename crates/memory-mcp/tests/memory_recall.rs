//! Memory recall: the entry point charges the access policy and
//! then delegates retrieval, so a refused caller never reaches
//! the retrieval pipeline.

use std::sync::{Arc, Mutex};

use memory_mcp::MemoryError;
use memory_mcp::memory::api::{ContextRetrievalPort, RateLimitPort, RecallCommand, recall_context};
use memory_mcp::models::AssembledContextItem;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Recall {
    query: String,
    budget: i32,
}

struct RecordingPort {
    recalls: Mutex<Vec<Recall>>,
    items: Vec<String>,
}

impl RecordingPort {
    fn with_items(items: &[&str]) -> Arc<Self> {
        Arc::new(Self {
            recalls: Mutex::new(Vec::new()),
            items: items.iter().map(|item| (*item).to_owned()).collect(),
        })
    }

    fn recalls(&self) -> Vec<Recall> {
        self.recalls.lock().expect("recalls lock").clone()
    }
}

fn item(fact_id: &str) -> AssembledContextItem {
    AssembledContextItem {
        fact_id: fact_id.to_owned(),
        content: format!("content for {fact_id}"),
        quote: String::new(),
        source_episode: "episode:1".to_owned(),
        confidence: 0.9,
        relevance: Some(0.8),
        grounding: Some(0.7),
        semantic_available: None,
        provenance: serde_json::json!({}),
        rationale: "tier=direct".to_owned(),
        retrieval_tier: Some("direct".to_owned()),
        reconciliation: None,
    }
}

#[async_trait::async_trait]
impl ContextRetrievalPort for RecordingPort {
    async fn retrieve(
        &self,
        command: &RecallCommand,
    ) -> Result<Vec<AssembledContextItem>, MemoryError> {
        self.recalls.lock().expect("recalls lock").push(Recall {
            query: command.query.clone(),
            budget: command.budget,
        });
        Ok(self.items.iter().map(|id| item(id)).collect())
    }
}

struct AllowAll;

impl RateLimitPort for AllowAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Ok(())
    }
}

struct DenyAll;

impl RateLimitPort for DenyAll {
    fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
        Err(MemoryError::Validation("rate limit exceeded".into()))
    }
}

fn command(query: &str) -> RecallCommand {
    RecallCommand {
        query: query.to_owned(),
        budget: 20,
        caller_id: Some("user-1".to_owned()),
    }
}

#[tokio::test]
async fn recall_delegates_the_query_and_budget_to_retrieval() {
    let port = RecordingPort::with_items(&["fact:1", "fact:2"]);

    let items = recall_context(port.as_ref(), &AllowAll, &command("who is ada"))
        .await
        .expect("recall succeeds");

    let fact_ids: Vec<&str> = items.iter().map(|it| it.fact_id.as_str()).collect();
    assert_eq!(fact_ids, vec!["fact:1", "fact:2"]);
    assert_eq!(
        port.recalls(),
        vec![Recall {
            query: "who is ada".to_owned(),
            budget: 20,
        }],
        "retrieval receives the query and budget verbatim"
    );
}

#[tokio::test]
async fn a_refused_caller_never_reaches_retrieval() {
    let port = RecordingPort::with_items(&["fact:1"]);

    let error = recall_context(port.as_ref(), &DenyAll, &command("who is ada"))
        .await
        .expect_err("a refused caller cannot recall");

    assert!(matches!(&error, MemoryError::Validation(message) if message == "rate limit exceeded"));
    assert!(
        port.recalls().is_empty(),
        "the retrieval pipeline must not run for a rate-limited caller"
    );
}

#[tokio::test]
async fn an_empty_result_is_a_successful_empty_recall() {
    let port = RecordingPort::with_items(&[]);

    let items = recall_context(port.as_ref(), &AllowAll, &command("nothing matches"))
        .await
        .expect("an empty recall is not an error");

    assert!(
        items.is_empty(),
        "no matches is an empty list, not a failure"
    );
    assert_eq!(port.recalls().len(), 1, "retrieval still ran once");
}

#[tokio::test]
async fn a_retrieval_failure_propagates_to_the_caller() {
    struct Failing;

    #[async_trait::async_trait]
    impl ContextRetrievalPort for Failing {
        async fn retrieve(
            &self,
            _command: &RecallCommand,
        ) -> Result<Vec<AssembledContextItem>, MemoryError> {
            Err(MemoryError::Storage("storage unavailable".into()))
        }
    }

    let error = recall_context(&Failing, &AllowAll, &command("who is ada"))
        .await
        .expect_err("a storage failure surfaces");
    assert!(matches!(&error, MemoryError::Storage(_)));
}

#[tokio::test]
async fn the_retrieval_port_is_object_safe_for_injection() {
    let port: Arc<dyn ContextRetrievalPort> = RecordingPort::with_items(&["fact:9"]);
    let items = recall_context(port.as_ref(), &AllowAll, &command("q"))
        .await
        .expect("a trait object port is usable");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].fact_id, "fact:9");
}
