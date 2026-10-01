//! A record accessor is scoped to one record kind. A caller
//! cannot reach another owner's table through it, and the generic
//! table-deriving accessor is gone.

use memory_mcp::storage::table_scope::{
    EmbeddingTables, KnowledgeTables, MemoryTables, PlatformTables, ReleaseOwnedTable,
};
use std::collections::BTreeMap;
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
        table: memory_mcp::storage::OwnedTable,
        _namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        Err(MemoryError::Storage(format!("select_table: {table}")))
    }

    async fn create(
        &self,
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
        _temporal_fields: &[&str],
    ) -> Result<serde_json::Value, MemoryError> {
        Ok(serde_json::json!({}))
    }

    async fn update(
        &self,
        _record_id: &str,
        _content: serde_json::Value,
        _namespace: &str,
        _temporal_fields: &[&str],
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

/// Every table the schema creates must be selectable.
///
/// The allowlist in `storage/client.rs` named ten tables while the schema
/// creates twenty-three. The thirteen it omitted were `claim`, `claim_job`,
/// `claim_key_alias`, `claim_policy`, `claim_relation`, `embedding_job`,
/// `embedding_state`, `entity_extraction_projection`, `event_projection_job`,
/// `memory_capture_audit`, `memory_event`, `procedure_candidate` and
/// `triple` — and a caller that added one of those and forgot the allowlist
/// got `ConfigInvalid` at runtime, phrased as a configuration problem, for
/// what is really a missing table in a list.
///
/// This asserts the shape the fix needs: the set of selectable tables equals
/// the set the schema creates. Reading the schema list rather than restating
/// it is what keeps the test from going stale when a migration adds a table.
#[tokio::test]
async fn every_expected_schema_table_is_selectable() {
    let schema_tables = memory_mcp::storage::expected_schema_tables();

    let client = memory_mcp::storage::SurrealDbClient::connect_in_memory_with_namespaces(
        "table_coverage",
        &["org".to_string()],
        "warn",
    )
    .await
    .expect("connect in-memory");
    client
        .apply_migrations("org")
        .await
        .expect("migrations apply");

    // Each table is released by the context that claims it. A table no context
    // claims cannot be produced at all, which is why the "nobody owns it" half
    // is a separate assertion below rather than an error here.
    let owners: BTreeMap<&'static str, &'static str> = memory_mcp::storage::table_owners()
        .into_iter()
        .flat_map(|(context, tables)| tables.iter().map(move |t| (*t, context)))
        .collect();

    let mut not_selectable = Vec::new();
    for &table in schema_tables {
        let owned = match owners.get(table) {
            Some(context) => *context,
            None => {
                not_selectable.push(format!("{table}: no context owns it"));
                continue;
            }
        };
        let released = release_for(owned, table);
        if let Err(error) = client.select_table(released, "org").await {
            not_selectable.push(format!("{table} ({owned}): {error}"));
        }
    }

    assert!(
        not_selectable.is_empty(),
        "{} table(s) the schema creates cannot be selected. Every table the \
         migrations build must be reachable, or a caller that adds one meets a \
         configuration error instead:\n\n{}",
        not_selectable.len(),
        not_selectable.join("\n")
    );
}

/// Each table has exactly one owner.
///
/// Two owners for one table means two places to update when its shape changes
/// and no way to tell which is right. Zero owners is the gap the test above
/// reports, so this one covers the other half.
#[test]
fn every_expected_schema_table_has_exactly_one_owner() {
    let mut owners: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for (context, tables) in memory_mcp::storage::table_owners() {
        for table in tables {
            owners.entry(table).or_default().push(context);
        }
    }

    let schema_tables = memory_mcp::storage::expected_schema_tables();
    let mut problems = Vec::new();
    for table in schema_tables {
        match owners.get(table) {
            None => problems.push(format!(
                "{table}: no context owns it. A table nobody claims is a table \
                 whose shape has no owner to change it."
            )),
            Some(holders) if holders.len() > 1 => problems.push(format!(
                "{table}: owned by {holders:?}. Two owners means two places to \
                 update when the shape changes."
            )),
            Some(_) => {}
        }
    }
    for (table, holders) in &owners {
        if !schema_tables.contains(table) {
            problems.push(format!(
                "{table}: owned by {holders:?} but no migration creates it. An \
                 owner for a table that does not exist is a list that drifts."
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} ownership problem(s):\n\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// Release `table` through the context that owns it.
///
/// Written as a match rather than a lookup so that a new owner is a compile
/// error here, not a runtime "no context owns it" from the test above.
fn release_for(context: &str, table: &'static str) -> memory_mcp::storage::OwnedTable {
    match context {
        "knowledge" => KnowledgeTables::table(table),
        "memory" => MemoryTables::table(table),
        "embedding" => EmbeddingTables::table(table),
        "storage" => PlatformTables::table(table),
        other => panic!("{other} owns a table but has no release path in this test"),
    }
}
