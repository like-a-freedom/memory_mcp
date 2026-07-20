//! Fact persistence service — handles fact record creation, validation, and index keys.
//!
//! Extracted from `MemoryService::add_fact` to reduce the God Object.
//! Embedding generation, triple extraction, and claim projection remain
//! orchestrated by `MemoryService` — this service handles only the core
//! fact record lifecycle.

use std::collections::HashSet;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use crate::models::Provenance;
use crate::service::error::MemoryError;
use crate::service::util::{deterministic_fact_id, validate_fact_input};
use crate::storage::DbClient;

use super::{normalize_dt, normalize_text, now};

/// Embedding fields to persist with a fact record.
/// Built by `MemoryService` from its embedding provider state and passed
/// to `FactService::create_fact`.
pub(crate) struct EmbeddingPayload {
    pub embedding: Vec<f64>,
    pub provider: String,
    pub model: Option<String>,
    pub dimension: usize,
    pub signature: Option<String>,
    pub updated_at: String,
}

/// Handles fact record CRUD: validation, ID generation, index key building, and persistence.
#[derive(Clone)]
pub struct FactService {
    db_client: Arc<dyn DbClient>,
}

impl FactService {
    pub fn new(db_client: Arc<dyn DbClient>) -> Self {
        Self { db_client }
    }

    /// Creates a new fact record if it does not already exist.
    ///
    /// Returns the fact ID. If the fact already exists (same deterministic ID),
    /// returns the existing ID without re-writing.
    ///
    /// The caller is responsible for embedding generation, triple extraction,
    /// claim projection, and cache invalidation — this method handles only
    /// the core persistence path.
    #[allow(clippy::too_many_arguments)]
    pub async fn create_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        scope: &str,
        confidence: f64,
        entity_links: &[String],
        policy_tags: &[String],
        provenance: &Provenance,
        namespace: &str,
        project: Option<&str>,
        embedding_fields: Option<EmbeddingPayload>,
        index_keys: Vec<String>,
    ) -> Result<String, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, scope)?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let existing = self.db_client.select_one(&fact_id, namespace).await?;
        if existing.is_some() {
            return Ok(fact_id);
        }

        let t_ingested = now();
        let mut payload = serde_json::Map::from_iter([
            ("fact_id".to_string(), json!(fact_id.clone())),
            ("fact_type".to_string(), json!(fact_type)),
            ("content".to_string(), json!(content)),
            ("quote".to_string(), json!(quote)),
            ("source_episode".to_string(), json!(source_episode)),
            ("t_valid".to_string(), json!(normalize_dt(t_valid))),
            ("t_ingested".to_string(), json!(normalize_dt(t_ingested))),
            ("confidence".to_string(), json!(confidence)),
            ("index_keys".to_string(), json!(index_keys)),
            ("access_count".to_string(), json!(0)),
            ("entity_links".to_string(), json!(entity_links)),
            ("scope".to_string(), json!(scope)),
            ("policy_tags".to_string(), json!(policy_tags)),
            ("provenance".to_string(), provenance.to_json_value()),
        ]);
        if let Some(project) = project {
            payload.insert("project".to_string(), json!(project));
        }
        if let Some(ep) = embedding_fields {
            payload.insert("embedding".to_string(), json!(ep.embedding));
            payload.insert("embedding_provider".to_string(), json!(ep.provider));
            if let Some(model) = ep.model {
                payload.insert("embedding_model".to_string(), json!(model));
            }
            payload.insert("embedding_dimension".to_string(), json!(ep.dimension));
            if let Some(signature) = ep.signature {
                payload.insert("embedding_signature".to_string(), json!(signature));
            }
            payload.insert("embedding_updated_at".to_string(), json!(ep.updated_at));
        }

        let created = self
            .db_client
            .create(&fact_id, Value::Object(payload), namespace)
            .await?;
        if created.is_null() {
            return Err(MemoryError::Storage(
                "failed to persist fact record".to_string(),
            ));
        }
        Ok(fact_id)
    }

    /// Builds the search index keys for a fact from entity links, temporal markers,
    /// and source references.
    ///
    /// `entity_lookup` is a closure that resolves an entity_id to its canonical
    /// name and aliases. This avoids a hard dependency on EntityService.
    #[allow(clippy::too_many_arguments)]
    pub async fn build_index_keys(
        &self,
        content: &str,
        source_episode: &str,
        provenance: &Provenance,
        entity_links: &[String],
        t_valid: DateTime<Utc>,
        entity_lookup: impl Fn(&str) -> Result<Option<(String, Vec<String>)>, MemoryError>,
        source_reference_lookup: impl Fn(&str) -> Result<Option<String>, MemoryError>,
    ) -> Result<Vec<String>, MemoryError> {
        let mut keys = HashSet::new();

        for entity_id in entity_links {
            if let Some((canonical, aliases)) = entity_lookup(entity_id)? {
                let normalized = normalize_text(&canonical);
                if !normalized.is_empty() {
                    keys.insert(normalized);
                }
                for alias in &aliases {
                    let normalized = normalize_text(alias);
                    if !normalized.is_empty() {
                        keys.insert(normalized);
                    }
                }
            }
        }

        keys.extend(crate::service::core::extract_temporal_index_keys(
            content, t_valid,
        ));
        keys.extend(reference_index_terms(content));

        // Collect source references
        let mut seen = HashSet::new();
        if let Some(source_id) = &provenance.source_id {
            let normalized = normalize_text(source_id);
            if !normalized.is_empty() && seen.insert(normalized.clone()) {
                keys.extend(reference_index_terms(source_id));
            }
        }
        if let Some(episode_source_id) = source_reference_lookup(source_episode)? {
            let normalized = normalize_text(&episode_source_id);
            if !normalized.is_empty() && seen.insert(normalized) {
                keys.extend(reference_index_terms(&episode_source_id));
            }
        }

        let mut keys: Vec<_> = keys.into_iter().collect();
        keys.sort();
        Ok(keys)
    }
}

// ─── Free helper functions ───────────────────────────────────────────────────

fn reference_index_terms(raw: &str) -> Vec<String> {
    let query_terms = crate::service::query::search_query_terms(raw);
    let mut keys = crate::service::query::query_hard_anchor_terms(&query_terms)
        .into_iter()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::mock_db::MockDbClient;

    #[tokio::test]
    async fn create_fact_persists_record() {
        let t = Utc::now();
        let fact_id = deterministic_fact_id("note", "hello world", "episode:test", t);
        let db = MockDbClient::new().expect_create(
            &fact_id,
            json!({"fact_id": fact_id.clone(), "status": "ok"}),
        );
        let svc = FactService::new(Arc::new(db));
        let provenance = Provenance::agent_observation("episode:test");

        let fact_id = svc
            .create_fact(
                "note",
                "hello world",
                "hello",
                "episode:test",
                t,
                "org",
                0.9,
                &[],
                &[],
                &provenance,
                "org",
                None,
                None,
                vec![],
            )
            .await
            .expect("create fact");

        assert!(fact_id.starts_with("fact:"));
    }

    #[tokio::test]
    async fn create_fact_returns_existing_id_on_duplicate() {
        let t = Utc::now();
        let fact_id = deterministic_fact_id("note", "dup", "episode:test", t);
        let db = MockDbClient::new()
            .expect_select_one(&fact_id, Some(json!({"fact_id": fact_id.clone()})));
        let svc = FactService::new(Arc::new(db));
        let provenance = Provenance::agent_observation("episode:test");

        let result = svc
            .create_fact(
                "note",
                "dup",
                "dup",
                "episode:test",
                t,
                "org",
                0.9,
                &[],
                &[],
                &provenance,
                "org",
                None,
                None,
                vec![],
            )
            .await
            .expect("create fact dup");

        assert_eq!(result, fact_id);
    }

    #[test]
    fn extract_temporal_index_keys_includes_month() {
        let t = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 3, 15, 10, 0, 0).unwrap();
        let keys = crate::service::core::extract_temporal_index_keys("test", t);
        assert!(keys.contains(&"2026-03".to_string()));
        assert!(keys.contains(&"march 2026".to_string()));
    }
}
