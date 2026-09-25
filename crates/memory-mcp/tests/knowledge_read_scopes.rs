//! Knowledge read policy: application-facing reads are
//! owner-named operations, never a caller-supplied table.

use std::sync::Mutex;

use memory_mcp::MemoryError;
use memory_mcp::knowledge::api::{
    KnowledgeReadPort, KnowledgeReadScope, owned_episode_scan, owned_fact_scan,
    read_through_knowledge_port,
};

/// Records what was asked for so the test can prove the
/// application layer cannot express an arbitrary table.
struct RecordingPort {
    requested: Mutex<Vec<KnowledgeReadScope>>,
    refuse: Option<KnowledgeReadScope>,
}

impl RecordingPort {
    fn serving() -> Self {
        Self {
            requested: Mutex::new(Vec::new()),
            refuse: None,
        }
    }

    fn refusing(scope: KnowledgeReadScope) -> Self {
        Self {
            requested: Mutex::new(Vec::new()),
            refuse: Some(scope),
        }
    }

    fn requested(&self) -> Vec<KnowledgeReadScope> {
        self.requested.lock().expect("requested lock").clone()
    }
}

#[async_trait::async_trait]
impl KnowledgeReadPort for RecordingPort {
    async fn read_scope(
        &self,
        scope: KnowledgeReadScope,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        self.requested.lock().expect("requested lock").push(scope);
        if self.refuse == Some(scope) {
            return Err(MemoryError::Storage("scope unavailable".into()));
        }
        Ok(Vec::new())
    }
}

#[test]
fn a_read_scope_is_named_by_owner_not_by_a_table_string() {
    let facts = KnowledgeReadScope::Facts;
    let episodes = KnowledgeReadScope::Episodes;

    assert_ne!(facts, episodes);
    assert_eq!(facts.owner(), "knowledge");
    assert_eq!(episodes.owner(), "memory");
}

#[tokio::test]
async fn an_owned_fact_scan_reaches_the_port_as_a_fact_scope() {
    let port = RecordingPort::serving();

    owned_fact_scan(&port)
        .await
        .expect("the fact scope is served");

    assert_eq!(port.requested(), vec![KnowledgeReadScope::Facts]);
}

#[tokio::test]
async fn an_owned_episode_scan_reaches_the_port_as_an_episode_scope() {
    let port = RecordingPort::serving();

    owned_episode_scan(&port)
        .await
        .expect("the episode scope is served");

    assert_eq!(port.requested(), vec![KnowledgeReadScope::Episodes]);
}

#[tokio::test]
async fn a_port_error_propagates_to_the_caller() {
    let port = RecordingPort::refusing(KnowledgeReadScope::Facts);

    assert!(owned_fact_scan(&port).await.is_err());
}

#[tokio::test]
async fn a_typed_read_reports_the_owner_that_answered_it() {
    let port = RecordingPort::serving();

    let read = read_through_knowledge_port(&port, KnowledgeReadScope::Facts)
        .await
        .expect("read succeeds");

    assert_eq!(read.owner, "knowledge");
    assert_eq!(read.scope, KnowledgeReadScope::Facts);
    assert_eq!(read.rows, 0);
}
