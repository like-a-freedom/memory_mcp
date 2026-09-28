//! Fact persistence service — handles fact record creation, validation, and index keys.
//!
//! Knowledge owns the fact record lifecycle: validation, deterministic
//! IDs, index-key building and persistence. The creation *pipeline* —
//! embedding generation, triple extraction, claim projection — needs
//! consumer-side wiring, so it lives in
//! [`crate::service::fact_orchestration`] and is added to this service
//! as a crate-private inherent method. The split keeps the write path
//! here without dragging the pipeline's dependencies in with it.

use std::collections::HashSet;
use std::sync::LazyLock;

use chrono::{DateTime, Utc};
use regex::Regex;
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::models::Provenance;
use crate::shared::ids::deterministic_fact_id;
use crate::shared::validation::validate_fact_input;

use crate::shared::search::normalize_text;
use crate::shared::temporal::{normalize_dt, now};

/// Embedding fields to persist with a fact record.
/// Built by the fact-creation pipeline from its embedding provider
/// state and passed to [`FactService::create_fact`].
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
    db: crate::knowledge::FactStoreClient,
    #[cfg(feature = "streamable-http")]
    outbox_enabled: bool,
}

/// Result of persisting a fact, reporting whether a new record was created
/// or an identical deterministic record already existed (idempotent repeat).
pub(crate) struct CreateFactOutcome {
    pub(crate) fact_id: String,
    pub(crate) created: bool,
}

impl FactService {
    pub fn new(db_client: crate::knowledge::FactStoreClient) -> Self {
        Self {
            db: db_client,
            #[cfg(feature = "streamable-http")]
            outbox_enabled: false,
        }
    }

    #[cfg(feature = "streamable-http")]
    pub(crate) fn with_outbox(mut self) -> Self {
        self.outbox_enabled = true;
        self
    }

    /// Creates a fact record with a deterministic ID.
    ///
    /// Returns the fact ID and whether a new record was created. When a fact
    /// with the same deterministic ID already exists, it is returned unchanged
    /// (`created == false`) and nothing is re-written.
    ///
    /// The caller is responsible for embedding generation, triple extraction,
    /// claim projection, and cache invalidation — this method handles only
    /// the core persistence path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn create_fact(
        &self,
        fact_type: &str,
        content: &str,
        quote: &str,
        source_episode: &str,
        t_valid: DateTime<Utc>,
        confidence: f64,
        entity_links: &[String],
        policy_tags: &[String],
        provenance: &Provenance,
        embedding_fields: Option<EmbeddingPayload>,
        index_keys: Vec<String>,
    ) -> Result<CreateFactOutcome, MemoryError> {
        validate_fact_input(fact_type, content, quote, source_episode, "")?;

        let fact_id = deterministic_fact_id(fact_type, content, source_episode, t_valid);
        let existing = self.db.select_one(&fact_id).await?;
        if existing.is_some() {
            return Ok(CreateFactOutcome {
                fact_id,
                created: false,
            });
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
            ("policy_tags".to_string(), json!(policy_tags)),
            ("provenance".to_string(), provenance.to_json_value()),
        ]);
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

        #[cfg(feature = "streamable-http")]
        if self.outbox_enabled {
            match self
                .db
                .create_with_event(&fact_id, Value::Object(payload))
                .await
            {
                Ok(()) => {}
                Err(MemoryError::Storage(message)) if message.contains("already exists") => {
                    return Ok(CreateFactOutcome {
                        fact_id,
                        created: false,
                    });
                }
                Err(error) => return Err(error),
            }
        } else {
            let created = self.db.create(&fact_id, Value::Object(payload)).await?;
            if created.is_null() {
                return Err(MemoryError::Storage(
                    "failed to persist fact record".to_string(),
                ));
            }
        }
        #[cfg(not(feature = "streamable-http"))]
        {
            let created = self.db.create(&fact_id, Value::Object(payload)).await?;
            if created.is_null() {
                return Err(MemoryError::Storage(
                    "failed to persist fact record".to_string(),
                ));
            }
        }
        Ok(CreateFactOutcome {
            fact_id,
            created: true,
        })
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

        keys.extend(extract_temporal_index_keys(content, t_valid));
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

/// Extracts temporal index keys from content and `t_valid` date.
pub(crate) fn extract_temporal_index_keys(content: &str, t_valid: DateTime<Utc>) -> Vec<String> {
    static MONTH_YEAR_RE: LazyLock<Result<Regex, regex::Error>> = LazyLock::new(|| {
        Regex::new(
            r"(?i)\b(january|february|march|april|may|june|july|august|september|october|november|december)\s+\d{4}\b",
        )
    });
    static ISO_DATE_RE: LazyLock<Result<Regex, regex::Error>> =
        LazyLock::new(|| Regex::new(r"\b\d{4}-\d{2}(?:-\d{2})?\b"));

    let mut keys = HashSet::from([
        crate::shared::search::normalize_text(&t_valid.format("%B %Y").to_string()),
        t_valid.format("%Y-%m").to_string(),
    ]);

    if let Ok(regex) = MONTH_YEAR_RE.as_ref() {
        for capture in regex.find_iter(content) {
            keys.insert(crate::shared::search::normalize_text(capture.as_str()));
        }
    }
    if let Ok(regex) = ISO_DATE_RE.as_ref() {
        for capture in regex.find_iter(content) {
            keys.insert(capture.as_str().to_lowercase());
        }
    }

    let mut keys = keys
        .into_iter()
        .filter(|v| !v.trim().is_empty())
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

/// Extracts hard-anchor reference terms from free text for the index.
fn reference_index_terms(raw: &str) -> Vec<String> {
    let query_terms = crate::shared::search::search_query_terms(raw);
    let mut keys = crate::shared::search::query_hard_anchor_terms(&query_terms)
        .into_iter()
        .collect::<Vec<_>>();
    keys.sort();
    keys
}

#[cfg(test)]
mod index_key_tests {
    use super::*;

    #[test]
    fn extract_temporal_index_keys_includes_month() {
        let t = chrono::TimeZone::with_ymd_and_hms(&chrono::Utc, 2026, 3, 15, 10, 0, 0).unwrap();
        let keys = extract_temporal_index_keys("test", t);
        assert!(keys.contains(&"2026-03".to_string()));
        assert!(keys.contains(&"march 2026".to_string()));
    }
}
