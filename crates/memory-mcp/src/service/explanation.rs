//! Explanation service — resolves episodes and facts for explain items,
//! computes shared graph insights, and builds citation-ready explain output.
//!
//! Reduces the God Object.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use serde_json::{Value, json};

use crate::logging::{LogLevel, StdoutLogger};
use crate::models::{
    AccessPayload, ExplainItem, ExplainRequest, GraphHubEntity, GraphInsights, Provenance,
    ProvenanceSource,
};
use crate::service::apps::graph::GraphContext;
use crate::service::error::MemoryError;
use crate::service::{log_event, normalize_dt, now, query};
use crate::storage::{AppStoreClient, BoundDbClient, DbClient};

use crate::service::value_helpers::{json_i64, string_from_value};

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
    fn app_store(&self) -> crate::storage::AppStoreClient {
        AppStoreClient::from_bound(self.db.clone())
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
            let (record, _) = self.find_episode_record(&item.source_episode).await?;
            let episode = record
                .as_ref()
                .and_then(crate::service::episode::episode_from_record);

            let entity_links = if let Some(ref fact_id) = item.fact_id {
                let (fact_record, _) = self.find_fact_record(fact_id).await?;
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
                && let Ok((fact_record, _ns)) = self.find_fact_record(fact_id).await
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
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.app_store().find_record_by_id(episode_id).await
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        self.app_store().find_record_by_id(fact_id).await
    }

    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), MemoryError> {
        let (record, _namespace) = self.find_fact_record(fact_id).await?;
        let Some(mut record) = record else {
            return Ok(());
        };

        let access_count = record
            .get("access_count")
            .and_then(json_i64)
            .unwrap_or(0)
            .saturating_add(boost);
        record.insert("access_count".to_string(), json!(access_count));
        record.insert(
            "last_accessed".to_string(),
            json!(normalize_dt(query::now())),
        );

        self.db.update(fact_id, Value::Object(record)).await?;

        Ok(())
    }

    async fn find_episodes_via_entity(
        &self,
        entity_id: &str,
    ) -> Result<Vec<crate::models::Episode>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT 10";
        let result = self
            .db
            .query(sql, Some(json!({"entity_id": entity_id})))
            .await?;

        let episodes: Vec<crate::models::Episode> = result
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| {
                        let obj = v.as_object()?;
                        crate::service::episode::episode_from_record(obj)
                    })
                    .collect()
            })
            .unwrap_or_default();

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
    //! Tests: drive validation into
    //! `ExplanationService::find_record_by_id` and the entry points that
    //! delegate to it (`find_episode_record`, `find_fact_record`).
    //!
    //! `find_fact_record` has its own body and does NOT delegate to
    //! `find_record_by_id`, so its validation must be wired in independently.

    use super::*;
    use crate::service::error::MemoryError;
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
        // `find_fact_record` has its own implementation that does not delegate
        // to `find_record_by_id`, so this test guards that path independently.
        let svc = make_service();
        let result = svc.find_fact_record("072d682d0d467aa94aad684d").await;
        assert!(matches!(result, Err(MemoryError::Validation(_))));
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
