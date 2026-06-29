use std::sync::Arc;

use serde_json::json;

use crate::models::{AccessPayload, EntityCandidate};

use super::error::MemoryError;
use super::util::{deterministic_entity_id, validate_entity_candidate};
use super::value_helpers::string_from_value;
use super::query::normalize_text;

pub struct EntityService {
    db_client: Arc<dyn crate::storage::DbClient>,
    default_namespace: String,
}

impl EntityService {
    pub fn new(db_client: Arc<dyn crate::storage::DbClient>, default_namespace: String) -> Self {
        Self {
            db_client,
            default_namespace,
        }
    }

    pub async fn resolve(
        &self,
        candidate: EntityCandidate,
    ) -> Result<String, MemoryError> {
        validate_entity_candidate(&candidate)?;
        let namespace = &self.default_namespace;
        let normalized = normalize_text(&candidate.canonical_name);

        let existing = self
            .db_client
            .select_entity_lookup(namespace, &candidate.canonical_name)
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

        match self.db_client.create(&entity_id, payload, namespace).await {
            Ok(_) => Ok(entity_id),
            Err(MemoryError::Storage(msg)) if msg.contains("already exists") => {
                let existing = self
                    .db_client
                    .select_entity_lookup(namespace, &candidate.canonical_name)
                    .await?;
                if let Some(record) = existing {
                    let existing_id = record
                        .get("entity_id")
                        .and_then(string_from_value)
                        .or_else(|| record.get("id").and_then(string_from_value))
                        .unwrap_or_default();
                    return Ok(existing_id);
                }
                Ok(entity_id)
            }
            Err(err) => Err(err),
        }
    }
}
