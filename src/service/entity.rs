use std::sync::Arc;

use serde_json::json;

use crate::models::{AccessPayload, EntityCandidate};

use super::error::MemoryError;
use super::util::{deterministic_entity_id, validate_entity_candidate, RateLimiter};
use super::value_helpers::string_from_value;
use super::query::normalize_text;

/// Resolves and persists entities. Extracted from `MemoryService::resolve`.
#[derive(Clone)]
pub struct EntityService {
    db_client: Arc<dyn crate::storage::DbClient>,
    default_namespace: String,
    rate_limiter: Arc<RateLimiter>,
}

impl EntityService {
    pub fn new(
        db_client: Arc<dyn crate::storage::DbClient>,
        default_namespace: String,
        rate_limiter: Arc<RateLimiter>,
    ) -> Self {
        Self {
            db_client,
            default_namespace,
            rate_limiter,
        }
    }

    /// Rate limit check matching `MemoryService::enforce_rate_limit`.
    fn enforce_rate_limit(&self, access: Option<&AccessPayload>) -> Result<(), MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.rate_limiter.allow(caller)
        {
            return Err(MemoryError::Validation("rate limit exceeded".into()));
        }
        Ok(())
    }

    /// Looks up an entity record by canonical name (normalized).
    async fn find_entity_record(
        &self,
        name: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Map<String, serde_json::Value>>, MemoryError> {
        let normalized = normalize_text(name);
        Ok(self
            .db_client
            .select_entity_lookup(namespace, &normalized)
            .await?
            .and_then(|record| record.as_object().cloned()))
    }

    /// Resolves an entity candidate, creating it if it does not exist.
    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
        access: Option<AccessPayload>,
    ) -> Result<String, MemoryError> {
        self.enforce_rate_limit(access.as_ref())?;
        validate_entity_candidate(&candidate)?;
        let namespace = self.default_namespace.clone();
        let normalized = normalize_text(&candidate.canonical_name);

        // Check if entity already exists by name
        let existing = self
            .find_entity_record(&candidate.canonical_name, &namespace)
            .await?;
        if let Some(record) = existing {
            let existing_id = record
                .get("entity_id")
                .and_then(string_from_value)
                .or_else(|| record.get("id").and_then(string_from_value))
                .unwrap_or_default();
            return Ok(existing_id);
        }

        let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
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
            "aliases": aliases.clone(),
        });

        // Attempt to create the entity. If it already exists (race condition),
        // fetch and return the existing entity ID.
        match self.db_client.create(&entity_id, payload, &namespace).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                // Race condition: another request created the entity concurrently.
                // Fetch and return the existing entity.
                let existing = self
                    .find_entity_record(&candidate.canonical_name, &namespace)
                    .await?;
                if let Some(record) = existing {
                    let existing_id = record
                        .get("entity_id")
                        .and_then(string_from_value)
                        .or_else(|| record.get("id").and_then(string_from_value))
                        .unwrap_or_default();
                    return Ok(existing_id);
                }
                // Fallback: return the deterministic ID even if we couldn't fetch
                Ok(entity_id)
            }
            Err(err) => Err(err),
        }
    }

    /// Resolves an entity by its type and canonical name.
    pub async fn resolve_typed(
        &self,
        entity_type: &str,
        name: &str,
    ) -> Result<String, MemoryError> {
        self.resolve(
            EntityCandidate {
                entity_type: entity_type.to_string(),
                canonical_name: name.to_string(),
                aliases: Vec::new(),
            },
            None,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EntityCandidate;
    use crate::service::mock_db::MockDbClient;
    use crate::service::util::RateLimiter;
    use crate::service::MemoryError;
    use std::sync::Arc;

    #[tokio::test]
    async fn resolve_creates_new_entity_when_not_exists() {
        let expected_id =
            super::super::util::deterministic_entity_id("person", "Alice Smith");
        let db = MockDbClient::new()
            .expect_entity_lookup("alice smith", None)
            .expect_create(&expected_id, serde_json::json!({"entity_id": &expected_id}));

        let svc = EntityService::new(Arc::new(db), "org".into(), Arc::new(RateLimiter::new(1000, 100)));

        let result = svc
            .resolve(
                EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: "Alice Smith".into(),
                    aliases: vec![],
                },
                None,
            )
            .await;

        assert_eq!(result.unwrap(), expected_id);
    }

    #[tokio::test]
    async fn resolve_returns_existing_entity_id() {
        let db = MockDbClient::new().expect_entity_lookup(
            "alice smith",
            Some(serde_json::json!({"entity_id": "entity:person:existing-alice"})),
        );

        let svc = EntityService::new(Arc::new(db), "org".into(), Arc::new(RateLimiter::new(1000, 100)));

        let result = svc
            .resolve(
                EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: "Alice Smith".into(),
                    aliases: vec![],
                },
                None,
            )
            .await;

        assert_eq!(result.unwrap(), "entity:person:existing-alice");
    }

    #[tokio::test]
    async fn resolve_handles_already_exists_race_condition() {
        let expected_id = super::super::util::deterministic_entity_id("person", "Bob");
        let db = MockDbClient::new()
            .expect_entity_lookup("bob", None)
            .expect_create_with(move || Err(MemoryError::Storage("already exists".into())));

        let svc = EntityService::new(Arc::new(db), "org".into(), Arc::new(RateLimiter::new(1000, 100)));

        let result = svc
            .resolve(
                EntityCandidate {
                    entity_type: "person".into(),
                    canonical_name: "Bob".into(),
                    aliases: vec![],
                },
                None,
            )
            .await;

        // When "already exists" race happens and second lookup also returns None,
        // the fallback returns the deterministic ID.
        assert_eq!(result.unwrap(), expected_id);
    }

    #[tokio::test]
    async fn resolve_typed_delegates_to_resolve() {
        let db = MockDbClient::new().expect_entity_lookup(
            "alice",
            Some(serde_json::json!({"entity_id": "entity:person:alice"})),
        );

        let svc = EntityService::new(Arc::new(db), "org".into(), Arc::new(RateLimiter::new(1000, 100)));

        let result = svc.resolve_typed("person", "Alice").await;
        assert_eq!(result.unwrap(), "entity:person:alice");
    }
}
