use serde_json::json;

use crate::models::EntityCandidate;

use super::query::normalize_text;
use super::util::{deterministic_entity_id, validate_entity_candidate};
use crate::error::MemoryError;

/// Resolves and persists entities.
///
/// Entity-table SQL lives in [`crate::storage::EntityStoreClient`]
/// this service expresses intent, not queries.
#[derive(Clone)]
pub struct EntityService {
    entity_store: crate::storage::EntityStoreClient,
    db: crate::storage::BoundDbClient,
}

impl EntityService {
    pub(crate) fn new(
        db_client: std::sync::Arc<dyn crate::storage::DbClient>,
        namespace: impl Into<String>,
    ) -> Self {
        let namespace = namespace.into();
        Self {
            entity_store: crate::storage::EntityStoreClient::new(
                db_client.clone(),
                namespace.clone(),
            ),
            db: crate::storage::BoundDbClient::new(db_client, namespace),
        }
    }

    // -- Fuzzy resolution support methods --

    /// Find an entity ID by its normalized canonical name.
    /// Returns `None` if no entity matches.
    pub async fn find_entity_id_by_name(
        &self,
        normalized_name: &str,
    ) -> Result<Option<String>, MemoryError> {
        self.entity_store
            .find_entity_id_by_name(normalized_name)
            .await
    }

    /// Find an entity ID by searching aliases.
    /// Returns `None` if no entity matches.
    pub async fn find_entity_id_by_alias(
        &self,
        normalized_alias: &str,
    ) -> Result<Option<String>, MemoryError> {
        self.entity_store
            .find_entity_id_by_alias(normalized_alias)
            .await
    }

    /// Find entities whose normalized name starts with the given prefix.
    /// Returns a list of `(entity_id, canonical_name)` pairs.
    pub async fn find_entities_by_prefix(
        &self,
        prefix: &str,
    ) -> Result<Vec<(String, String)>, MemoryError> {
        self.entity_store.find_entities_by_prefix(prefix).await
    }

    /// Add an alias to an existing entity.
    pub async fn add_alias_to_entity(
        &self,
        entity_id: &str,
        alias: &str,
    ) -> Result<(), MemoryError> {
        let normalized_alias = normalize_text(alias);
        self.entity_store
            .add_alias(entity_id, &normalized_alias)
            .await
    }

    /// Create a new entity from a candidate and return its ID.
    pub async fn create_entity(&self, candidate: EntityCandidate) -> Result<String, MemoryError> {
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

        match self.db.create(&entity_id, payload).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                // Race condition — return the existing entity.
                let existing = self.find_entity_id_by_name(&normalized).await?;
                Ok(existing.unwrap_or(entity_id))
            }
            Err(err) => Err(err),
        }
    }
}
