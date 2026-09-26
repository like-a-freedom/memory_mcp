//! Explanation service — resolves episodes and facts for explain items,
//! computes shared graph insights, and builds citation-ready explain output.
//!
//! Reduces the God Object.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};

use crate::error::MemoryError;
use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{
    AccessPayload, ExplainItem, ExplainRequest, GraphHubEntity, GraphInsights, Provenance,
    ProvenanceSource,
};
use crate::service::apps::graph::GraphContext;
use crate::service::{log_event, normalize_dt, now};
use crate::storage::{BoundDbClient, DbClient};

use crate::service::value_helpers::string_from_value;

/// How an item-level provenance lookup failure is handled.
///
/// A wrong-kind id is a legitimate outcome for `explain`: callers
/// pass whatever id they hold, and an item whose `source_episode`
/// names another kind simply has no episode provenance. That is
/// the same outcome as an id that no longer exists, so it is
/// absorbed rather than failing the whole pack.
///
/// Every other error is real and must propagate. Suppressing them
/// would turn a storage outage into a silently degraded response,
/// so this classification is the whole safety property and is
/// tested directly.
fn provenance_lookup_error(
    error: MemoryError,
) -> Result<Option<serde_json::Map<String, Value>>, MemoryError> {
    match error {
        MemoryError::Validation(_) => Ok(None),
        other => Err(other),
    }
}

/// Handles `explain` orchestration: episode/fact resolution, provenance
/// collection, graph insights, and explain item construction.
#[derive(Clone)]
pub struct ExplanationService {
    db: BoundDbClient,
    logger: StdoutLogger,
}

impl ExplanationService {
    pub fn new(
        db_client: Arc<dyn DbClient>,
        logger: StdoutLogger,
        active_namespace: String,
    ) -> Self {
        Self {
            db: BoundDbClient::new(db_client, active_namespace),
            logger,
        }
    }
}

impl GraphContext for ExplanationService {
    fn knowledge_graph_store(&self) -> crate::storage::KnowledgeGraphStore {
        crate::storage::KnowledgeGraphStore::from_bound(self.db.clone())
    }
    fn logger(&self) -> &StdoutLogger {
        &self.logger
    }
}

impl ExplanationService {
    pub async fn explain(
        &self,
        request: ExplainRequest,
        access: Option<AccessPayload>,
    ) -> Result<Vec<ExplainItem>, MemoryError> {
        // --- Phase 1: resolve episodes / facts, collect all entity_links ---
        struct ResolvedItem {
            item: ExplainItem,
            episode: Option<crate::models::Episode>,
            entity_links: Vec<String>,
        }

        let mut resolved = Vec::with_capacity(request.context_pack.len());
        let mut all_entity_links: HashSet<String> = HashSet::new();

        for item in request.context_pack {
            if item.source_episode.is_empty() {
                return Err(MemoryError::Validation(
                    "source_episode is required for explain items".into(),
                ));
            }
            // A source_episode that is not an episode is not an
            // error: explain reports what it can and leaves the
            // item's provenance empty, the same as it does for an
            // episode id that no longer exists. The owner-scoped
            // accessor refuses the cross-kind read; this absorbs
            // the refusal rather than failing the whole pack.
            let record = match self.find_episode_record(&item.source_episode).await {
                Ok(record) => record,
                Err(error) => provenance_lookup_error(error)?,
            };
            let episode = record
                .as_ref()
                .and_then(crate::service::episode::episode_from_record);

            let entity_links = if let Some(ref fact_id) = item.fact_id {
                // Same policy for the fact side: an id that names
                // another kind contributes no entity links.
                let fact_record = match self.find_fact_record(fact_id).await {
                    Ok(record) => record,
                    Err(error) => provenance_lookup_error(error)?,
                };
                let links = fact_record
                    .and_then(|r| {
                        r.get("entity_links").and_then(|v| v.as_array()).map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect::<Vec<_>>()
                        })
                    })
                    .unwrap_or_default();
                for link in &links {
                    all_entity_links.insert(link.clone());
                }
                links
            } else {
                Vec::new()
            };

            resolved.push(ResolvedItem {
                item,
                episode,
                entity_links,
            });
        }

        // --- Phase 2: shared graph insights (computed once for the batch) ---
        let entity_links_vec: Vec<String> = all_entity_links.into_iter().collect();
        let shared_insights = self.build_graph_insights_batched(&entity_links_vec).await?;

        // --- Phase 3: build explain items with cached provenance ---
        let mut episode_via_entity_cache: HashMap<String, Vec<crate::models::Episode>> =
            HashMap::new();
        let mut explanations = Vec::with_capacity(resolved.len());

        for resolved_item in resolved {
            // Track fact access regardless of whether the episode is found
            if let Some(ref fact_id) = resolved_item.item.fact_id
                && let Err(err) = self.record_fact_access(fact_id, 3).await
            {
                self.logger.log(
                    log_event(
                        "explain.access_track_error",
                        json!({"fact_id": fact_id}),
                        json!({"error": err.to_string()}),
                        access.as_ref(),
                        None,
                        None,
                    ),
                    LogLevel::Warn,
                );
            }

            let Some(episode) = resolved_item.episode else {
                explanations.push(resolved_item.item);
                continue;
            };

            let all_sources = self
                .collect_provenance_sources_cached(
                    &episode,
                    &resolved_item.entity_links,
                    &mut episode_via_entity_cache,
                )
                .await?;

            // Build provenance for the explain response: merge episode-level
            // info with the fact's structured provenance (ingestion_method, etc.).
            let mut explain_provenance = json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
            });
            let mut fact_age_days: Option<i64> = None;
            let mut decayed_confidence: Option<f64> = None;
            let mut ingestion_method: Option<String> = None;

            if let Some(fact_id) = &resolved_item.item.fact_id
                && let Ok(fact_record) = self.find_fact_record(fact_id).await
                && let Some(record) = &fact_record
            {
                let prov_value = record.get("provenance").cloned().unwrap_or(Value::Null);
                let fact_prov = Provenance::from_json_value(&prov_value);
                if let Some(map) = explain_provenance.as_object_mut() {
                    if !fact_prov.ingestion_method.is_empty() {
                        map.insert(
                            "ingestion_method".to_string(),
                            json!(fact_prov.ingestion_method),
                        );
                    }
                    if let Some(strategy) = &fact_prov.extraction_strategy {
                        map.insert("extraction_strategy".to_string(), json!(strategy));
                    }
                }
                ingestion_method = Some(fact_prov.ingestion_method);

                // Compute fact_age_days from t_valid
                if let Some(t_valid_str) = record.get("t_valid").and_then(string_from_value)
                    && let Ok(t_valid) = chrono::DateTime::parse_from_rfc3339(&t_valid_str)
                {
                    let age = Utc::now()
                        .signed_duration_since(t_valid.with_timezone(&Utc))
                        .num_days();
                    fact_age_days = Some(age);
                }

                // Compute decayed_confidence
                if let Some(conf) = record.get("confidence").and_then(|v| v.as_f64())
                    && let Some(age) = fact_age_days
                {
                    let half_life_days = if record
                        .get("fact_type")
                        .and_then(string_from_value)
                        .is_some_and(|ft| ft == "metric")
                    {
                        crate::models::Fact::METRIC_HALF_LIFE_DAYS
                    } else {
                        crate::models::Fact::DEFAULT_HALF_LIFE_DAYS
                    };
                    let decay = 2.0_f64.powf(-age as f64 / half_life_days);
                    decayed_confidence = Some(conf * decay);
                }
            }

            let explanation = ExplainItem {
                fact_id: resolved_item.item.fact_id,
                content: if resolved_item.item.content.is_empty() {
                    episode.content.clone()
                } else {
                    resolved_item.item.content
                },
                quote: resolved_item.item.quote,
                source_episode: resolved_item.item.source_episode,
                t_ref: Some(episode.t_ref),
                t_ingested: Some(episode.t_ingested),
                provenance: explain_provenance,
                citation_context: Some(episode.content.clone()),
                all_sources,
                graph_insights: shared_insights.clone(),
                fact_age_days,
                decayed_confidence,
                ingestion_method,
            };

            explanations.push(explanation);
        }

        self.logger.log(
            log_event(
                "explain",
                json!({"count": explanations.len()}),
                json!({"count": explanations.len()}),
                access.as_ref(),
                None,
                None,
            ),
            LogLevel::Info,
        );

        Ok(explanations)
    }

    // ─── Private helpers ───────────────────────────────────────────────────

    pub(crate) async fn find_episode_record(
        &self,
        episode_id: &str,
    ) -> Result<crate::storage::RecordLookup, MemoryError> {
        crate::storage::owner_scoped_read(
            crate::storage::EpisodeStoreClient::from_bound(self.db.clone())
                .select_episode(episode_id)
                .await,
        )
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<crate::storage::RecordLookup, MemoryError> {
        crate::storage::owner_scoped_read(
            crate::storage::FactStoreClient::from_bound(self.db.clone())
                .select_fact(fact_id)
                .await,
        )
    }

    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), MemoryError> {
        crate::storage::FactAccessStore::from_bound(self.db.clone())
            .record_fact_access(fact_id, boost)
            .await
    }

    async fn find_episodes_via_entity(
        &self,
        entity_id: &str,
    ) -> Result<Vec<crate::models::Episode>, MemoryError> {
        let rows = crate::storage::EpisodeContextStore::from_bound(self.db.clone())
            .select_episodes_via_entity(entity_id)
            .await?;

        let episodes: Vec<crate::models::Episode> = rows
            .iter()
            .filter_map(|value| {
                let obj = value.as_object()?;
                crate::service::episode::episode_from_record(obj)
            })
            .collect();

        Ok(episodes)
    }

    /// Computes graph insights once for a batch of entity links (reduced explain budget).
    async fn build_graph_insights_batched(
        &self,
        entity_links: &[String],
    ) -> Result<Option<GraphInsights>, MemoryError> {
        const MAX_GRAPH_INSIGHT_LINKED_ENTITIES: usize = 8;
        const MAX_GRAPH_INSIGHT_HUBS: i32 = 5;
        const MAX_GRAPH_INSIGHT_CONNECTIONS: usize = 5;

        let mut seen_linked_entities = HashSet::new();
        let linked_entities = entity_links
            .iter()
            .filter(|entity_id| entity_id.starts_with("entity:"))
            .filter(|entity_id| seen_linked_entities.insert((**entity_id).clone()))
            .take(MAX_GRAPH_INSIGHT_LINKED_ENTITIES)
            .cloned()
            .collect::<Vec<_>>();
        if linked_entities.is_empty() {
            self.logger.log(
                log_event(
                    "explain.graph_insights.skipped",
                    json!({}),
                    json!({"reason": "no_linked_entities"}),
                    None,
                    None,
                    None,
                ),
                LogLevel::Trace,
            );
            return Ok(None);
        }

        self.logger.log(
            log_event(
                "explain.graph_insights.start",
                json!({
                    "linked_entity_count": linked_entities.len(),
                }),
                json!({}),
                None,
                None,
                None,
            ),
            LogLevel::Debug,
        );

        let budget = crate::service::apps::graph::GraphTraversalBudget::EXPLAIN;
        let cutoff = now();
        let hub_entities = crate::service::apps::graph::find_hub_entities(
            self,
            cutoff,
            MAX_GRAPH_INSIGHT_HUBS,
            budget,
        )
        .await?
        .into_iter()
        .map(|hub| GraphHubEntity {
            entity_id: hub.entity_id,
            canonical_name: hub.canonical_name,
            degree: hub.degree,
        })
        .collect::<Vec<_>>();

        let mut surprising_connections = Vec::new();
        let mut seen_connections = HashSet::new();

        for entity_id in linked_entities {
            for connection in crate::service::apps::graph::find_surprising_connections(
                self, &entity_id, 3, budget,
            )
            .await?
            {
                let key = format!(
                    "{}->{}",
                    connection.source_entity_id, connection.target_entity_id
                );
                if seen_connections.insert(key) {
                    surprising_connections.push(connection);
                }
                if surprising_connections.len() >= MAX_GRAPH_INSIGHT_CONNECTIONS {
                    break;
                }
            }

            if surprising_connections.len() >= MAX_GRAPH_INSIGHT_CONNECTIONS {
                break;
            }
        }

        surprising_connections.sort_by(|left, right| {
            left.hop_count
                .cmp(&right.hop_count)
                .then_with(|| left.target_entity_name.cmp(&right.target_entity_name))
                .then_with(|| left.target_entity_id.cmp(&right.target_entity_id))
        });

        self.logger.log(
            log_event(
                "explain.graph_insights.done",
                json!({}),
                json!({
                    "hub_entities": hub_entities.len(),
                    "surprising_connections": surprising_connections.len(),
                }),
                None,
                None,
                None,
            ),
            LogLevel::Trace,
        );

        Ok(Some(GraphInsights {
            hub_entities,
            surprising_connections,
        }))
    }

    /// Collects provenance sources for an explain item, using an episode-via-entity cache
    /// to avoid redundant `find_episodes_via_entity` calls for the same entity across items.
    async fn collect_provenance_sources_cached(
        &self,
        primary_episode: &crate::models::Episode,
        entity_links: &[String],
        cache: &mut HashMap<String, Vec<crate::models::Episode>>,
    ) -> Result<Vec<ProvenanceSource>, MemoryError> {
        let mut sources = Vec::new();

        // 1. Add direct source episode
        sources.push(ProvenanceSource {
            episode_id: primary_episode.episode_id.clone(),
            episode_content: primary_episode.content.clone(),
            episode_t_ref: normalize_dt(primary_episode.t_ref),
            relationship: "direct".to_string(),
            entity_path: None,
        });

        // 2. Traverse entity_links to find connected episodes (cache-aware)
        for entity_id in entity_links {
            let linked_episodes = if let Some(cached) = cache.get(entity_id) {
                cached.clone()
            } else {
                let episodes = self.find_episodes_via_entity(entity_id).await?;
                cache.insert(entity_id.clone(), episodes.clone());
                episodes
            };

            for ep in linked_episodes {
                // Skip if this is the primary source (already added)
                if ep.episode_id == primary_episode.episode_id {
                    continue;
                }

                sources.push(ProvenanceSource {
                    episode_id: ep.episode_id.clone(),
                    episode_content: ep.content.clone(),
                    episode_t_ref: normalize_dt(ep.t_ref),
                    relationship: "linked".to_string(),
                    entity_path: Some(format!("{} -> {}", primary_episode.episode_id, entity_id)),
                });
            }
        }

        // Sort: direct first, then by t_ref descending
        sources.sort_by(|a, b| {
            if a.relationship == "direct" {
                std::cmp::Ordering::Less
            } else if b.relationship == "direct" {
                std::cmp::Ordering::Greater
            } else {
                b.episode_t_ref.cmp(&a.episode_t_ref)
            }
        });

        Ok(sources)
    }
}

#[cfg(test)]
mod tests {
    //! Tests: drive validation into the owner-scoped accessors
    //! behind `ExplanationService::find_episode_record` and
    //! `find_fact_record`. Each goes through the store that owns
    //! that record kind, so a cross-kind id is refused.

    use super::*;
    use crate::error::MemoryError;
    use crate::service::mock_db::MockDbClient;

    fn make_service() -> ExplanationService {
        ExplanationService::new(
            Arc::new(MockDbClient::new()),
            StdoutLogger::new("warn"),
            "org".to_string(),
        )
    }

    #[tokio::test]
    async fn find_episode_record_rejects_bare_hex() {
        let svc = make_service();
        let result = svc.find_episode_record("474b2d8b81b3feabf832ef08").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_episode_record_rejects_empty_id_part() {
        let svc = make_service();
        let result = svc.find_episode_record("episode:").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_fact_record_rejects_bare_hex() {
        let svc = make_service();
        let result = svc.find_fact_record("072d682d0d467aa94aad684d").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[test]
    fn a_wrong_kind_lookup_is_absorbed_but_a_storage_failure_is_not() {
        // The cross-kind refusal from an owner-scoped accessor is an
        // expected outcome and yields "no provenance for this item".
        let absorbed = provenance_lookup_error(MemoryError::Validation(
            "record_id 'task:obj' is not a episode record id".into(),
        ));
        assert!(
            matches!(absorbed, Ok(None)),
            "a wrong-kind id must degrade to no provenance, got {absorbed:?}"
        );

        // Anything else is a real failure. If a storage error were
        // absorbed here, a database outage would look like a
        // successful explain with empty provenance, which is exactly
        // the kind of silent degradation this policy must not allow.
        assert!(
            matches!(
                provenance_lookup_error(MemoryError::Storage("connection refused".into())),
                Err(MemoryError::Storage(message)) if message == "connection refused"
            ),
            "a storage failure must propagate, not degrade to no provenance"
        );
        assert!(
            matches!(
                provenance_lookup_error(MemoryError::ConfigInvalid("missing key".into())),
                Err(MemoryError::ConfigInvalid(message)) if message == "missing key"
            ),
            "a configuration failure must propagate, not degrade to no provenance"
        );
    }

    #[tokio::test]
    async fn find_fact_record_rejects_empty_id_part() {
        let svc = make_service();
        let result = svc.find_fact_record("fact:").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
    }

    #[tokio::test]
    async fn find_episode_record_accepts_wellformed_episode_id() {
        // Sanity: well-formed ids pass validation and reach the DB (mock returns None).
        let svc = make_service();
        let result = svc.find_episode_record("episode:doesnotexist").await;
        assert!(
            result.is_ok(),
            "well-formed id must pass validation: {result:?}"
        );
    }
}
