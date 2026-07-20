//! Explanation service — resolves episodes and facts for explain items,
//! computes shared graph insights, and builds citation-ready explain output.
//!
//! Extracted from `MemoryService::explain` to reduce the God Object.

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
use crate::storage::DbClient;

use crate::service::value_helpers::{json_i64, string_from_value};

/// Handles `explain` orchestration: episode/fact resolution, provenance
/// collection, graph insights, and explain item construction.
#[derive(Clone)]
pub struct ExplanationService {
    db_client: Arc<dyn DbClient>,
    logger: StdoutLogger,
    namespaces: Vec<String>,
    default_namespace: String,
}

impl ExplanationService {
    pub fn new(
        db_client: Arc<dyn DbClient>,
        logger: StdoutLogger,
        namespaces: Vec<String>,
    ) -> Self {
        let default_namespace = namespaces.first().cloned().unwrap_or_else(|| "org".into());
        Self {
            db_client,
            logger,
            namespaces,
            default_namespace,
        }
    }

    pub fn namespace_for_scope(&self, scope: &str) -> Result<String, MemoryError> {
        for namespace in &self.namespaces {
            if namespace == scope {
                return Ok(namespace.clone());
            }
        }
        Ok(self.default_namespace.clone())
    }
}

impl GraphContext for ExplanationService {
    fn app_store(&self) -> &dyn crate::storage::AppStore {
        &self.db_client
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
            fact_namespace: Option<String>,
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

            let (entity_links, fact_namespace) = if let Some(ref fact_id) = item.fact_id {
                let (fact_record, namespace) = self.find_fact_record(fact_id).await?;
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
                (links, namespace)
            } else {
                (Vec::new(), None)
            };

            resolved.push(ResolvedItem {
                item,
                episode,
                entity_links,
                fact_namespace,
            });
        }

        // --- Phase 2: shared graph insights (computed once for the batch) ---
        let entity_links_vec: Vec<String> = all_entity_links.into_iter().collect();
        let first_namespace = resolved
            .iter()
            .find_map(|r| {
                r.fact_namespace.clone().or_else(|| {
                    r.episode
                        .as_ref()
                        .and_then(|ep| self.namespace_for_scope(&ep.scope).ok())
                })
            })
            .unwrap_or_else(|| self.default_namespace.clone());
        let shared_insights = self
            .build_graph_insights_batched(&entity_links_vec, &first_namespace)
            .await?;

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

            let namespace = resolved_item.fact_namespace.unwrap_or_else(|| {
                self.namespace_for_scope(&episode.scope)
                    .unwrap_or_else(|_| self.default_namespace.clone())
            });

            let all_sources = self
                .collect_provenance_sources_cached(
                    &episode,
                    &resolved_item.entity_links,
                    &namespace,
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
                    decayed_confidence = Some(
                        (conf * decay * crate::models::Fact::CONFIDENCE_SCALE).round()
                            / crate::models::Fact::CONFIDENCE_SCALE,
                    );
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
                scope: Some(episode.scope.clone()),
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
        self.find_record_by_id(episode_id).await
    }

    pub(crate) async fn find_fact_record(
        &self,
        fact_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(fact_id, namespace).await?;
            if let Some(map) = record.and_then(|v| v.as_object().cloned()) {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    async fn find_record_by_id(
        &self,
        record_id: &str,
    ) -> Result<(Option<serde_json::Map<String, Value>>, Option<String>), MemoryError> {
        for namespace in &self.namespaces {
            let record = self.db_client.select_one(record_id, namespace).await?;
            if let Some(map) = record.and_then(|v| v.as_object().cloned()) {
                return Ok((Some(map), Some(namespace.clone())));
            }
        }
        Ok((None, None))
    }

    pub(crate) async fn record_fact_access(
        &self,
        fact_id: &str,
        boost: i64,
    ) -> Result<(), MemoryError> {
        let (record, namespace) = self.find_fact_record(fact_id).await?;
        let Some(namespace) = namespace else {
            return Ok(());
        };
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

        self.db_client
            .update(fact_id, Value::Object(record), &namespace)
            .await?;

        Ok(())
    }

    async fn find_episodes_via_entity(
        &self,
        entity_id: &str,
        namespace: &str,
    ) -> Result<Vec<crate::models::Episode>, MemoryError> {
        let sql = "SELECT * FROM episode WHERE episode_id IN (SELECT VALUE source_episode FROM fact WHERE fact_id IN (SELECT VALUE type::string(out) FROM edge WHERE in = <record> $entity_id AND relation = 'involved_in')) ORDER BY t_ref DESC LIMIT 10";
        let result = self
            .db_client
            .query(sql, Some(json!({"entity_id": entity_id})), namespace)
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
        namespace: &str,
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
                    json!({"namespace": namespace}),
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
                    "namespace": namespace,
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
            namespace,
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
                self, namespace, &entity_id, 3, budget,
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
                json!({"namespace": namespace}),
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
        namespace: &str,
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
                let episodes = self.find_episodes_via_entity(entity_id, namespace).await?;
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
