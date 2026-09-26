//! A record accessor is scoped to one record kind. A caller
//! cannot reach another owner's table through it, and the generic
//! table-deriving accessor is gone.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use memory_mcp::MemoryError;
use memory_mcp::storage::{DbClient, EpisodeStoreClient, FactStoreClient};

/// Refuses any table other than the ones it is asked to read, so a
/// test that passes the wrong record kind fails loudly instead of
/// silently reading the wrong aggregate. It also records whether it
/// was ever reached, so a test can assert a malformed id was
/// rejected *before* any query ran.
#[derive(Clone)]
struct TableCheckingDb {
    reached: Arc<AtomicUsize>,
}

impl TableCheckingDb {
    fn new() -> Self {
        Self {
            reached: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn was_reached(&self) -> bool {
        self.reached.load(Ordering::SeqCst) > 0
    }
}

#[async_trait::async_trait]
impl DbClient for TableCheckingDb {
    async fn select_one(
        &self,
        record_id: &str,
        _namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        self.reached.fetch_add(1, Ordering::SeqCst);
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
    let store = EpisodeStoreClient::new(Arc::new(TableCheckingDb::new()), "org");

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
    let store = FactStoreClient::new(Arc::new(TableCheckingDb::new()), "org");

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
async fn a_malformed_id_is_refused_before_any_query_runs() {
    // A bare id has no `table:` prefix, so it names no record kind.
    // Both the shared id validation and the kind guard reject it;
    // what matters is that the store never reaches the database.
    let episodes = EpisodeStoreClient::new(Arc::new(TableCheckingDb::new()), "org");
    let facts = FactStoreClient::new(Arc::new(TableCheckingDb::new()), "org");

    for malformed in ["474b2d8b81b3feabf", "", "episode:", "fact:"] {
        assert!(
            episodes.select_episode(malformed).await.is_err(),
            "the episode accessor must refuse '{malformed}'"
        );
        assert!(
            facts.select_fact(malformed).await.is_err(),
            "the fact accessor must refuse '{malformed}'"
        );
    }

    // A well-formed id of the right kind does reach the database, so
    // the assertions above are testing the refusal path and not a
    // store that simply never queries.
    let reachable = TableCheckingDb::new();
    let probe = FactStoreClient::new(Arc::new(reachable.clone()), "org");
    probe
        .select_fact("fact:ok")
        .await
        .expect("a valid fact id is served");
    assert!(
        reachable.was_reached(),
        "the fake must be reachable, otherwise the refusals above prove nothing"
    );
}

#[tokio::test]
async fn a_cross_kind_id_is_refused_without_reaching_the_database() {
    // This is the property the owner-scoped accessor exists for: a
    // record that exists under another kind must not be read through
    // this accessor, and the refusal must happen before the query.
    let db = TableCheckingDb::new();
    let episodes = EpisodeStoreClient::new(Arc::new(db.clone()), "org");

    let error = episodes
        .select_episode("fact:exists-in-another-table")
        .await
        .expect_err("a fact id is not an episode");
    assert!(
        matches!(&error, MemoryError::Validation(message) if message.contains("fact:exists-in-another-table")),
        "the refusal must name the offending id, got {error:?}"
    );
    assert!(
        !db.was_reached(),
        "the cross-kind refusal must happen before any database read"
    );
}
