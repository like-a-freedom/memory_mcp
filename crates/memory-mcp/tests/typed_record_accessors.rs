//! A record accessor is scoped to one record kind. A caller
//! cannot reach another owner's table through it, and the generic
//! table-deriving accessor is gone.

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::storage::{DbClient, EpisodeStoreClient, FactStoreClient};

/// Refuses any table other than the ones it is asked to read, so a
/// test that passes the wrong record kind fails loudly instead of
/// silently reading the wrong aggregate.
struct TableCheckingDb;

#[async_trait::async_trait]
impl DbClient for TableCheckingDb {
    async fn select_one(
        &self,
        record_id: &str,
        _namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        let table = record_id.split(':').next().unwrap_or_default();
        Ok(Some(serde_json::json!({ "table": table })))
    }

    async fn select_table(
        &self,
        table: &str,
        _namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        Err(MemoryError::Storage(format!("select_table: {table}")))
    }

    async fn create(
        &self,
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({}))
    }

    async fn update(
        &self,
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({}))
    }

    async fn query(
        &self,
        _sql: &str,
        _vars: Option<serde_json::Value>,
        _namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!([]))
    }

    async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
        Ok(())
    }
}

#[tokio::test]
async fn the_episode_accessor_reads_episodes_and_refuses_a_fact() {
    let store = EpisodeStoreClient::new(Arc::new(TableCheckingDb), "org");

    let episode = store
        .select_episode("episode:abc")
        .await
        .expect("an episode id is served");
    assert_eq!(episode.expect("episode present")["table"], "episode");

    let error = store
        .select_episode("fact:abc")
        .await
        .expect_err("a fact id is not an episode");
    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("fact:abc")),
        "the refusal must name the offending id, got {error:?}"
    );
}

#[tokio::test]
async fn the_fact_accessor_reads_facts_and_refuses_an_edge() {
    let store = FactStoreClient::new(Arc::new(TableCheckingDb), "org");

    let fact = store
        .select_fact("fact:abc")
        .await
        .expect("a fact id is served");
    assert_eq!(fact.expect("fact present")["table"], "fact");

    // `edge` carries `t_invalid`, so a cross-kind read here is the
    // case that would otherwise succeed quietly.
    let error = store
        .select_fact("edge:abc")
        .await
        .expect_err("an edge id is not a fact");
    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("edge:abc")),
        "the refusal must name the offending id, got {error:?}"
    );
}

#[tokio::test]
async fn both_accessors_refuse_a_bare_id_before_any_query() {
    let episodes = EpisodeStoreClient::new(Arc::new(TableCheckingDb), "org");
    let facts = FactStoreClient::new(Arc::new(TableCheckingDb), "org");

    assert!(episodes.select_episode("474b2d8b81b3feabf").await.is_err());
    assert!(facts.select_fact("474b2d8b81b3feabf").await.is_err());
}

#[tokio::test]
async fn both_accessors_refuse_an_empty_id() {
    let episodes = EpisodeStoreClient::new(Arc::new(TableCheckingDb), "org");
    let facts = FactStoreClient::new(Arc::new(TableCheckingDb), "org");

    assert!(episodes.select_episode("").await.is_err());
    assert!(facts.select_fact("").await.is_err());
}
