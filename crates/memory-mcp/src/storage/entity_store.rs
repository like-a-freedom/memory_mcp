//! Narrow entity store: owns the `entity` table's SQL (ADR-0044).
//!
//! Alias resolution reads and the alias-append write live here so the
//! service layer expresses intent instead of supplying SQL.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::service::MemoryError;
use crate::storage::{BoundDbClient, DbClient};

/// One owner for entity-table queries.
#[derive(Clone)]
pub(crate) struct EntityStoreClient {
    db: BoundDbClient,
}

impl EntityStoreClient {
    pub(crate) fn new(db: Arc<dyn DbClient>, namespace: impl Into<String>) -> Self {
        Self {
            db: BoundDbClient::new(db, namespace),
        }
    }

    pub(crate) fn from_bound(db: BoundDbClient) -> Self {
        Self { db }
    }

    /// Find an entity ID by its normalized canonical name, falling back to
    /// alias membership. Returns `None` if no entity matches.
    pub(crate) async fn find_entity_id_by_name(
        &self,
        normalized_name: &str,
    ) -> Result<Option<String>, MemoryError> {
        // Canonical-name index lookup first (fast path), then alias lookup.
        let canonical_sql = "SELECT * FROM entity WHERE canonical_name_normalized = $name LIMIT 1";
        if let Some(record) = self
            .db
            .query_first(canonical_sql, Some(json!({ "name": normalized_name })))
            .await?
        {
            return Ok(entity_id_from_record(&record));
        }

        self.find_entity_id_by_alias(normalized_name).await
    }

    /// Find an entity ID by searching aliases. Returns `None` if no entity
    /// matches.
    ///
    /// NOTE: `entity_aliases` is a plain (non-FULLTEXT) index on the
    /// `aliases` array, so the FTS operator `@1@` would silently match
    /// nothing. `CONTAINS` is SurrealDB's array-membership operator and is
    /// index-aware.
    pub(crate) async fn find_entity_id_by_alias(
        &self,
        normalized_alias: &str,
    ) -> Result<Option<String>, MemoryError> {
        let sql = "SELECT entity_id FROM entity WHERE aliases CONTAINS $alias LIMIT 1";
        let rows = self
            .db
            .query_rows(sql, Some(json!({ "alias": normalized_alias })))
            .await?;
        Ok(rows.first().and_then(entity_id_from_record))
    }

    /// Find entities whose normalized name starts with the given prefix.
    /// Returns `(entity_id, canonical_name)` pairs.
    pub(crate) async fn find_entities_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let sql = "SELECT entity_id, canonical_name FROM entity \
                   WHERE string::starts_with(canonical_name_normalized, $prefix) LIMIT 50";
        let rows = self
            .db
            .query_rows(sql, Some(json!({ "prefix": prefix })))
            .await?;
        Ok(rows
            .iter()
            .filter_map(|record| {
                let id =
                    crate::service::value_helpers::string_from_value(record.get("entity_id")?)?;
                let name = crate::service::value_helpers::string_from_value(
                    record.get("canonical_name")?,
                )?;
                Some((id, name))
            })
            .collect())
    }

    /// Append an alias to an existing entity's alias list.
    pub(crate) async fn add_alias(
        &self,
        entity_id: &str,
        normalized_alias: &str,
    ) -> Result<(), MemoryError> {
        let sql = "UPDATE type::record($id) SET aliases += [$alias]";
        self.db
            .query(
                sql,
                Some(json!({ "id": entity_id, "alias": normalized_alias })),
            )
            .await?;
        Ok(())
    }
}

fn entity_id_from_record(record: &Value) -> Option<String> {
    crate::service::value_helpers::string_from_value(record.get("entity_id")?)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::{Value, json};

    use crate::storage::{DbClient, SurrealDbClient, entity_store::EntityStoreClient};

    async fn make_db() -> Arc<SurrealDbClient> {
        let db_name = format!(
            "entity_store_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        db_client
            .apply_migrations("org")
            .await
            .expect("apply migrations");
        db_client
    }

    async fn seed_entity(db: &Arc<SurrealDbClient>, aliases: &[&str]) {
        db.create(
            "entity:alice",
            json!({
                "entity_id": "entity:alice",
                "entity_type": "person",
                "canonical_name": "Alice",
                "canonical_name_normalized": "alice",
                "aliases": aliases,
            }),
            "org",
        )
        .await
        .expect("seed entity");
    }

    #[tokio::test]
    async fn find_entity_id_by_alias_returns_matching_entity() {
        let db = make_db().await;
        seed_entity(&db, &["ali", "alicia"]).await;
        let store = EntityStoreClient::new(db, "org");

        assert_eq!(
            store.find_entity_id_by_alias("ali").await.expect("find"),
            Some("entity:alice".to_string())
        );
        assert_eq!(
            store
                .find_entity_id_by_alias("unknown")
                .await
                .expect("find"),
            None
        );
    }

    #[tokio::test]
    async fn find_entity_id_by_name_prefers_canonical_then_alias() {
        let db = make_db().await;
        seed_entity(&db, &["ali"]).await;
        let store = EntityStoreClient::new(db, "org");

        assert_eq!(
            store.find_entity_id_by_name("alice").await.expect("find"),
            Some("entity:alice".to_string())
        );
        assert_eq!(
            store.find_entity_id_by_name("ali").await.expect("find"),
            Some("entity:alice".to_string())
        );
        assert_eq!(
            store.find_entity_id_by_name("bob").await.expect("find"),
            None
        );
    }

    #[tokio::test]
    async fn find_entities_by_prefix_returns_id_and_canonical_name() {
        let db = make_db().await;
        seed_entity(&db, &[]).await;
        let store = EntityStoreClient::new(db, "org");

        assert_eq!(
            store.find_entities_by_prefix("ali").await.expect("find"),
            vec![("entity:alice".to_string(), "Alice".to_string())]
        );
        assert!(
            store
                .find_entities_by_prefix("zed")
                .await
                .expect("find")
                .is_empty()
        );
    }

    #[tokio::test]
    async fn add_alias_appends_to_existing_entity() {
        let db = make_db().await;
        seed_entity(&db, &["ali"]).await;
        let store = EntityStoreClient::new(db.clone(), "org");

        store
            .add_alias("entity:alice", "alicia")
            .await
            .expect("add alias");

        let record = db
            .select_one("entity:alice", "org")
            .await
            .expect("read")
            .expect("present");
        let aliases: Vec<Value> = record
            .get("aliases")
            .and_then(Value::as_array)
            .map_or_else(Vec::new, |array| array.to_vec());
        assert!(aliases.contains(&json!("ali")));
        assert!(aliases.contains(&json!("alicia")));
    }

    #[tokio::test]
    async fn reads_degrade_to_empty_before_entity_table_exists() {
        let db_name = format!(
            "entity_store_premigration_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let db = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                &db_name,
                &["org".to_string()],
                "warn",
            )
            .await
            .expect("connect in memory db"),
        );
        // No migrations applied: the `entity` table is absent.
        let store = EntityStoreClient::new(db, "org");
        assert_eq!(
            store.find_entity_id_by_alias("ali").await.expect("find"),
            None
        );
        assert!(
            store
                .find_entities_by_prefix("ali")
                .await
                .expect("find")
                .is_empty()
        );
    }
}
