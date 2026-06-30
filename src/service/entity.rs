use std::sync::Arc;

use serde_json::json;

use crate::models::EntityCandidate;

use super::error::MemoryError;
use super::query::normalize_text;
use super::util::{deterministic_entity_id, validate_entity_candidate};
use super::value_helpers::string_from_value;

/// Resolves and persists entities. Extracted from `MemoryService::resolve`.
#[derive(Clone)]
pub struct EntityService {
    db_client: Arc<dyn crate::storage::DbClient>,
}

impl EntityService {
    pub fn new(db_client: Arc<dyn crate::storage::DbClient>) -> Self {
        Self { db_client }
    }

    // -- Fuzzy resolution support methods --

    /// Find an entity ID by its normalized canonical name.
    /// Returns `None` if no entity matches.
    pub async fn find_entity_id_by_name(
        &self,
        normalized_name: &str,
        namespace: &str,
    ) -> Result<Option<String>, MemoryError> {
        Ok(self
            .db_client
            .select_entity_lookup(namespace, normalized_name)
            .await?
            .and_then(|record| {
                record
                    .as_object()
                    .and_then(|map| map.get("entity_id").and_then(string_from_value))
            }))
    }

    /// Find an entity ID by searching aliases.
    /// Returns `None` if no entity matches.
    pub async fn find_entity_id_by_alias(
        &self,
        normalized_alias: &str,
        namespace: &str,
    ) -> Result<Option<String>, MemoryError> {
        // NOTE: `entity_aliases` is a plain (non-FULLTEXT) index on the `aliases`
        // array, so the FTS operator `@1@` would silently match nothing.
        // `CONTAINS` is SurrealDB's array-membership operator and is index-aware.
        let sql = "SELECT entity_id FROM entity WHERE aliases CONTAINS $alias LIMIT 1";
        let result = self
            .db_client
            .query(sql, Some(json!({"alias": normalized_alias})), namespace)
            .await?;
        Ok(result
            .as_array()
            .and_then(|arr| arr.first())
            .and_then(|v| v.as_object())
            .and_then(|map| map.get("entity_id").and_then(string_from_value)))
    }

    /// Find entities whose normalized name starts with the given prefix.
    /// Returns a list of `(entity_id, canonical_name)` pairs.
    pub async fn find_entities_by_prefix(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        let sql = "SELECT entity_id, canonical_name FROM entity WHERE string::starts_with(canonical_name_normalized, $prefix) LIMIT 50";
        let result = self
            .db_client
            .query(sql, Some(json!({"prefix": prefix})), namespace)
            .await?;
        Ok(result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let map = v.as_object()?;
                        let id = map.get("entity_id").and_then(string_from_value)?;
                        let name = map.get("canonical_name").and_then(string_from_value)?;
                        Some((id, name))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Add an alias to an existing entity.
    pub async fn add_alias_to_entity(
        &self,
        entity_id: &str,
        alias: &str,
        namespace: &str,
    ) -> Result<(), MemoryError> {
        let normalized_alias = normalize_text(alias);
        let sql = "UPDATE type::record($id) SET aliases += [$alias]";
        self.db_client
            .query(
                sql,
                Some(json!({"id": entity_id, "alias": normalized_alias})),
                namespace,
            )
            .await?;
        Ok(())
    }

    /// Create a new entity from a candidate and return its ID.
    pub async fn create_entity(
        &self,
        candidate: EntityCandidate,
        namespace: &str,
    ) -> Result<String, MemoryError> {
        validate_entity_candidate(&candidate)?;
        let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
        let normalized = normalize_text(&candidate.canonical_name);
        let aliases = candidate
            .aliases
            .into_iter()
            .filter(|alias| !alias.trim().is_empty())
            .map(|alias| normalize_text(&alias))
            .collect::<Vec<_>>();

        let payload = json!({
            "entity_id": entity_id,
            "entity_type": candidate.entity_type,
            "canonical_name": candidate.canonical_name,
            "canonical_name_normalized": normalized,
            "aliases": aliases,
        });

        match self.db_client.create(&entity_id, payload, namespace).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                // Race condition — return the existing entity.
                let existing = self.find_entity_id_by_name(&normalized, namespace).await?;
                Ok(existing.unwrap_or(entity_id))
            }
            Err(err) => Err(err),
        }
    }

    /// Execute a query against the triple table.
    /// Helper for conflict resolution.
    pub async fn query_triples(
        &self,
        sql: &str,
        namespace: &str,
        subject: &str,
        predicate: &str,
        object: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        self.db_client
            .query(
                sql,
                Some(json!({
                    "ns": namespace,
                    "subject": subject,
                    "predicate": predicate,
                    "object": object,
                })),
                namespace,
            )
            .await
    }

    /// Invalidate a triple by ID.
    /// Helper for conflict resolution.
    pub async fn invalidate_triple_by_id(
        &self,
        sql: &str,
        namespace: &str,
        triple_id: &str,
    ) -> Result<(), MemoryError> {
        self.db_client
            .query(sql, Some(json!({"id": triple_id})), namespace)
            .await?;
        Ok(())
    }

    /// Execute a raw SQL query with bind variables.
    /// Helper for triple persistence and other operations.
    pub async fn execute_query(
        &self,
        sql: &str,
        vars: serde_json::Value,
        namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        self.db_client.query(sql, Some(vars), namespace).await
    }
}
