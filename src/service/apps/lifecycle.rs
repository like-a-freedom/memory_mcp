use std::collections::BTreeSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::config::LifecycleConfig;
use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::service::{MemoryService, fact_from_record, normalize_dt, parse_iso};

/// Returns JSON fallback per §2.5 for APP-04.
#[must_use]
pub fn lifecycle_fallback(dashboard: &LifecycleDashboard) -> serde_json::Value {
    serde_json::json!({
        "low_confidence": dashboard.low_confidence_facts.iter().map(|c| serde_json::json!({"id": c.fact_id, "reason": c.reason})).collect::<Vec<_>>(),
        "archival_candidates": dashboard.archival_candidates.iter().map(|c| serde_json::json!({"id": c.episode_id, "reason": c.reason})).collect::<Vec<_>>(),
        "archived_episodes": dashboard.archived_episodes.iter().map(|c| serde_json::json!({"id": c.episode_id, "reason": format!("archived {}", c.archived_at)})).collect::<Vec<_>>(),
        "stale_communities": dashboard.stale_communities.iter().map(|c| serde_json::json!({"id": c.community_id, "reason": c.reason})).collect::<Vec<_>>(),
    })
}

/// Service-side lifecycle console helper for APP-04.
pub struct LifecycleConsole;

/// Read model for the lifecycle dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleDashboard {
    pub low_confidence_facts: Vec<LifecycleCandidate>,
    pub archival_candidates: Vec<ArchivalCandidate>,
    pub archived_episodes: Vec<ArchivedEpisode>,
    pub stale_communities: Vec<StaleCommunity>,
}

/// Candidate fact that may be invalidated by decay policies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LifecycleCandidate {
    pub fact_id: String,
    pub confidence: f64,
    pub reason: String,
}

/// Episode candidate that may be archived.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivalCandidate {
    pub episode_id: String,
    pub reason: String,
    pub facts_count: i32,
}

/// Archived episode preview shown in the lifecycle dashboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedEpisode {
    pub episode_id: String,
    pub archived_at: DateTime<Utc>,
    pub facts_count: i32,
}

/// Community candidate that no longer matches the active graph structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaleCommunity {
    pub community_id: String,
    pub last_updated: DateTime<Utc>,
    pub reason: String,
}

#[derive(Debug, Clone, Default)]
struct DashboardFilters {
    min_confidence: Option<f64>,
    max_confidence: Option<f64>,
    inactive_days: Option<i64>,
    include_archived: bool,
}

impl DashboardFilters {
    fn from_json(filters: Option<&Value>) -> Self {
        let Some(filters) = filters.and_then(Value::as_object) else {
            return Self {
                include_archived: true,
                ..Self::default()
            };
        };

        Self {
            min_confidence: filters.get("min_confidence").and_then(Value::as_f64),
            max_confidence: filters.get("max_confidence").and_then(Value::as_f64),
            inactive_days: filters.get("inactive_days").and_then(Value::as_i64),
            include_archived: filters
                .get("include_archived")
                .and_then(Value::as_bool)
                .unwrap_or(true),
        }
    }
}

impl LifecycleConsole {
    pub(crate) async fn load_dashboard(
        service: &MemoryService,
        scope: &str,
        filters: Option<&Value>,
    ) -> Result<LifecycleDashboard, MemoryError> {
        let config = LifecycleConfig::from_env();
        let filters = DashboardFilters::from_json(filters);
        let namespace = service.resolve_namespace_for_scope(scope)?;
        let now = Utc::now();

        Ok(LifecycleDashboard {
            low_confidence_facts: load_low_confidence_facts(
                service,
                &namespace,
                now,
                config.decay_confidence_threshold,
                config.decay_half_life_days,
                &filters,
            )
            .await?,
            archival_candidates: load_archival_candidates(
                service,
                &namespace,
                now,
                filters
                    .inactive_days
                    .unwrap_or(i64::from(config.archival_age_days)) as u32,
            )
            .await?,
            archived_episodes: if filters.include_archived {
                load_archived_episodes(service, &namespace).await?
            } else {
                Vec::new()
            },
            stale_communities: load_stale_communities(service, scope, &namespace).await?,
        })
    }

    pub(crate) async fn archive_candidates(
        service: &MemoryService,
        scope: &str,
        candidate_ids: &[String],
        dry_run: bool,
    ) -> Result<Value, MemoryError> {
        let namespace = service.resolve_namespace_for_scope(scope)?;
        let now = normalize_dt(Utc::now());
        let mut impacted_active_facts = 0usize;
        let mut archived_episode_ids = Vec::new();

        for episode_id in candidate_ids {
            let Some(mut record) = service.db_client.select_one(episode_id, &namespace).await?
            else {
                continue;
            };

            let active_facts = service
                .db_client
                .select_active_facts_by_episode(&namespace, episode_id, &now, 10_000)
                .await?;
            impacted_active_facts += active_facts.len();
            archived_episode_ids.push(episode_id.clone());

            if dry_run {
                continue;
            }

            if let Some(map) = record.as_object_mut() {
                map.insert("status".to_string(), json!("archived"));
                map.insert("archived_at".to_string(), json!(now.clone()));
            }
            service
                .db_client
                .update(episode_id, record, &namespace)
                .await?;
        }

        Ok(json!({
            "ok": true,
            "dry_run": dry_run,
            "message": if dry_run {
                format!("Would archive {} episodes", archived_episode_ids.len())
            } else {
                format!("Archived {} episodes", archived_episode_ids.len())
            },
            "archived_episode_ids": archived_episode_ids,
            "impacted_active_facts": impacted_active_facts,
            "refresh_required": true,
        }))
    }

    pub(crate) async fn restore_archived(
        service: &MemoryService,
        scope: &str,
        episode_ids: &[String],
    ) -> Result<Value, MemoryError> {
        let namespace = service.resolve_namespace_for_scope(scope)?;
        let mut restored_episode_ids = Vec::new();

        for episode_id in episode_ids {
            let Some(mut record) = service.db_client.select_one(episode_id, &namespace).await?
            else {
                continue;
            };

            if let Some(map) = record.as_object_mut() {
                map.insert("status".to_string(), json!("active"));
                map.insert("archived_at".to_string(), Value::Null);
            }
            service
                .db_client
                .update(episode_id, record, &namespace)
                .await?;
            restored_episode_ids.push(episode_id.clone());
        }

        Ok(json!({
            "ok": true,
            "message": format!("Restored {} episodes", restored_episode_ids.len()),
            "episode_ids": restored_episode_ids,
            "refresh_required": true,
        }))
    }

    pub(crate) async fn recompute_decay(
        service: &MemoryService,
        scope: &str,
        target_ids: Option<&[String]>,
        dry_run: bool,
    ) -> Result<Value, MemoryError> {
        let config = LifecycleConfig::from_env();
        let namespace = service.resolve_namespace_for_scope(scope)?;
        let now = Utc::now();
        let active_facts = service
            .db_client
            .select_active_facts(&namespace, 10_000)
            .await?;
        let target_ids = target_ids.map(|ids| ids.iter().cloned().collect::<BTreeSet<_>>());
        let mut invalidated_fact_ids = Vec::new();

        for record in active_facts {
            let Some(fact) = fact_from_record(&record) else {
                continue;
            };
            if target_ids
                .as_ref()
                .is_some_and(|targets| !targets.contains(&fact.fact_id))
            {
                continue;
            }

            let decayed = compute_decayed_confidence(&fact, now, config.decay_half_life_days);
            if decayed >= config.decay_confidence_threshold
                || fact_is_hot(&fact, now, config.decay_half_life_days)
            {
                continue;
            }

            invalidated_fact_ids.push(fact.fact_id.clone());
            if dry_run {
                continue;
            }

            service
                .db_client
                .update(
                    &fact.fact_id,
                    json!({
                        "t_invalid": normalize_dt(now),
                        "t_invalid_ingested": normalize_dt(now),
                    }),
                    &namespace,
                )
                .await?;
        }

        crate::service::invalidate_cache_by_scope(&service.context_cache, scope).await;

        Ok(json!({
            "ok": true,
            "dry_run": dry_run,
            "message": if dry_run {
                format!("Would recompute decay for {} facts", invalidated_fact_ids.len())
            } else {
                format!("Recomputed decay for {} facts", invalidated_fact_ids.len())
            },
            "invalidated_fact_ids": invalidated_fact_ids,
            "refresh_required": true,
        }))
    }

    pub(crate) async fn rebuild_communities(
        service: &MemoryService,
        scope: &str,
        dry_run: bool,
    ) -> Result<Value, MemoryError> {
        if dry_run {
            let dashboard = Self::load_dashboard(service, scope, None).await?;
            return Ok(json!({
                "ok": true,
                "dry_run": true,
                "message": format!(
                    "Would rebuild communities and clean up {} stale communities",
                    dashboard.stale_communities.len()
                ),
                "stale_community_ids": dashboard
                    .stale_communities
                    .iter()
                    .map(|community| community.community_id.clone())
                    .collect::<Vec<_>>(),
                "refresh_required": false,
            }));
        }

        let community_count =
            crate::service::episode::rebuild_all_communities(service, scope).await?;
        let dashboard = Self::load_dashboard(service, scope, None).await?;

        Ok(json!({
            "ok": true,
            "message": format!("Rebuilt {} communities", community_count),
            "community_count": community_count,
            "stale_communities_remaining": dashboard.stale_communities.len(),
            "refresh_required": true,
        }))
    }
}

fn compute_decayed_confidence(fact: &Fact, now: DateTime<Utc>, half_life_days: f64) -> f64 {
    let days_since_valid = (now - fact.t_valid).num_days() as f64;
    let decay_rate = (2.0_f64).ln() / half_life_days.max(1.0);
    fact.confidence * (-decay_rate * days_since_valid).exp()
}

fn fact_is_hot(fact: &Fact, now: DateTime<Utc>, half_life_days: f64) -> bool {
    fact.access_count > 0
        && fact
            .last_accessed
            .is_some_and(|last_accessed| (now - last_accessed).num_days() as f64 <= half_life_days)
}

async fn load_low_confidence_facts(
    service: &MemoryService,
    namespace: &str,
    now: DateTime<Utc>,
    threshold: f64,
    half_life_days: f64,
    filters: &DashboardFilters,
) -> Result<Vec<LifecycleCandidate>, MemoryError> {
    let mut candidates = service
        .db_client
        .select_active_facts(namespace, 500)
        .await?
        .into_iter()
        .filter_map(|record| fact_from_record(&record))
        .filter_map(|fact| {
            let decayed = compute_decayed_confidence(&fact, now, half_life_days);
            if decayed >= threshold || fact_is_hot(&fact, now, half_life_days) {
                return None;
            }
            if filters
                .min_confidence
                .is_some_and(|min_confidence| decayed < min_confidence)
            {
                return None;
            }
            if filters
                .max_confidence
                .is_some_and(|max_confidence| decayed > max_confidence)
            {
                return None;
            }

            Some(LifecycleCandidate {
                fact_id: fact.fact_id,
                confidence: decayed,
                reason: format!(
                    "decayed confidence {:.3} below threshold {:.3}",
                    decayed, threshold
                ),
            })
        })
        .collect::<Vec<_>>();

    candidates.sort_by(|left, right| {
        left.confidence
            .partial_cmp(&right.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| left.fact_id.cmp(&right.fact_id))
    });
    Ok(candidates)
}

async fn load_archival_candidates(
    service: &MemoryService,
    namespace: &str,
    now: DateTime<Utc>,
    archival_age_days: u32,
) -> Result<Vec<ArchivalCandidate>, MemoryError> {
    let cutoff = normalize_dt(now - chrono::Duration::days(i64::from(archival_age_days)));
    let mut candidates = Vec::new();

    for record in service
        .db_client
        .select_episodes_for_archival(namespace, &cutoff, 200)
        .await?
    {
        let Some(episode_id) = record.get("episode_id").and_then(record_string) else {
            continue;
        };

        let active_facts = service
            .db_client
            .select_active_facts_by_episode(namespace, &episode_id, &normalize_dt(now), 10_000)
            .await?;
        let has_recent_heat =
            episode_has_recent_fact_access(service, namespace, &episode_id, archival_age_days)
                .await?;
        if !active_facts.is_empty() || has_recent_heat {
            continue;
        }

        candidates.push(ArchivalCandidate {
            episode_id: episode_id.clone(),
            reason: format!(
                "older than {} days with no active facts and no recent access",
                archival_age_days
            ),
            facts_count: count_facts_for_episode(service, namespace, &episode_id).await?,
        });
    }

    candidates.sort_by(|left, right| left.episode_id.cmp(&right.episode_id));
    Ok(candidates)
}

async fn load_archived_episodes(
    service: &MemoryService,
    namespace: &str,
) -> Result<Vec<ArchivedEpisode>, MemoryError> {
    let mut rows = service
        .db_client
        .query(
            "SELECT * FROM episode WHERE status = 'archived' ORDER BY archived_at DESC LIMIT 100",
            None,
            namespace,
        )
        .await?
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut archived_episodes = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        let Some(episode_id) = row.get("episode_id").and_then(record_string) else {
            continue;
        };
        let Some(archived_at) = row
            .get("archived_at")
            .and_then(record_string)
            .as_deref()
            .and_then(parse_iso)
        else {
            continue;
        };

        archived_episodes.push(ArchivedEpisode {
            episode_id: episode_id.clone(),
            archived_at,
            facts_count: count_facts_for_episode(service, namespace, &episode_id).await?,
        });
    }

    Ok(archived_episodes)
}

async fn load_stale_communities(
    service: &MemoryService,
    scope: &str,
    namespace: &str,
) -> Result<Vec<StaleCommunity>, MemoryError> {
    let mut stale_communities = Vec::new();

    for record in service
        .db_client
        .select_table("community", namespace)
        .await?
    {
        let Some(community_id) = record.get("community_id").and_then(record_string) else {
            continue;
        };
        let member_entities = record
            .get("member_entities")
            .and_then(record_string_array)
            .unwrap_or_default();
        let last_updated = record
            .get("updated_at")
            .and_then(record_string)
            .as_deref()
            .and_then(parse_iso)
            .unwrap_or_else(Utc::now);

        let reason = if member_entities.len() < 2 {
            Some("community has fewer than 2 members".to_string())
        } else {
            let expected = crate::service::episode::connected_entity_component(
                service,
                &member_entities,
                scope,
            )
            .await?
            .into_iter()
            .collect::<BTreeSet<_>>();
            let actual = member_entities.into_iter().collect::<BTreeSet<_>>();

            (expected != actual).then_some(
                "community membership drifted from the active graph component".to_string(),
            )
        };

        if let Some(reason) = reason {
            stale_communities.push(StaleCommunity {
                community_id,
                last_updated,
                reason,
            });
        }
    }

    stale_communities.sort_by(|left, right| left.community_id.cmp(&right.community_id));
    Ok(stale_communities)
}

async fn count_facts_for_episode(
    service: &MemoryService,
    namespace: &str,
    episode_id: &str,
) -> Result<i32, MemoryError> {
    let result = service
        .db_client
        .query(
            "SELECT count() AS count FROM fact WHERE source_episode = $episode_id GROUP ALL",
            Some(json!({"episode_id": episode_id})),
            namespace,
        )
        .await?;

    Ok(result
        .as_array()
        .and_then(|rows| rows.first())
        .and_then(|row| row.get("count"))
        .and_then(record_i64)
        .unwrap_or(0) as i32)
}

async fn episode_has_recent_fact_access(
    service: &MemoryService,
    namespace: &str,
    episode_id: &str,
    age_days: u32,
) -> Result<bool, MemoryError> {
    let hot_cutoff = normalize_dt(Utc::now() - chrono::Duration::days(i64::from(age_days)));
    let result = service
        .db_client
        .query(
            "SELECT fact_id FROM fact WHERE source_episode = $episode_id AND last_accessed IS NOT NONE AND last_accessed >= type::datetime($hot_cutoff) LIMIT 1",
            Some(json!({"episode_id": episode_id, "hot_cutoff": hot_cutoff})),
            namespace,
        )
        .await?;

    Ok(result.as_array().is_some_and(|rows| !rows.is_empty()))
}

fn record_string(value: &Value) -> Option<String> {
    if let Some(value) = value.as_str() {
        return Some(value.to_string());
    }

    let object = value.as_object()?;
    object
        .get("String")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            object
                .get("Strand")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            object
                .get("Strand")
                .and_then(|inner| inner.get("String"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            object
                .get("Datetime")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            object
                .get("Datetime")
                .and_then(|inner| inner.get("String"))
                .and_then(Value::as_str)
                .map(ToString::to_string)
        })
        .or_else(|| {
            object.get("RecordId").and_then(|record_id| {
                let record_id = record_id.as_object()?;
                let table = record_id.get("table")?.as_str()?;
                let key = record_id.get("key")?.as_str()?;
                Some(format!("{table}:{key}"))
            })
        })
}

fn record_string_array(value: &Value) -> Option<Vec<String>> {
    let array = if let Some(array) = value.as_array() {
        Some(array)
    } else {
        value
            .as_object()
            .and_then(|object| object.get("Array")?.as_array())
    }?;

    Some(array.iter().filter_map(record_string).collect())
}

fn record_i64(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_f64().map(|number| number as i64))
        .or_else(|| {
            value.as_object().and_then(|object| {
                object
                    .get("Number")
                    .and_then(Value::as_i64)
                    .or_else(|| object.get("int").and_then(Value::as_i64))
                    .or_else(|| {
                        object
                            .get("float")
                            .and_then(Value::as_f64)
                            .map(|number| number as i64)
                    })
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_fallback_empty_dashboard() {
        let dashboard = LifecycleDashboard {
            low_confidence_facts: vec![],
            archival_candidates: vec![],
            archived_episodes: vec![],
            stale_communities: vec![],
        };
        let val = lifecycle_fallback(&dashboard);
        assert!(val["low_confidence"].as_array().unwrap().is_empty());
        assert!(val["archival_candidates"].as_array().unwrap().is_empty());
        assert!(val["archived_episodes"].as_array().unwrap().is_empty());
        assert!(val["stale_communities"].as_array().unwrap().is_empty());
    }

    #[test]
    fn lifecycle_fallback_with_data() {
        let dashboard = LifecycleDashboard {
            low_confidence_facts: vec![LifecycleCandidate {
                fact_id: "fact:1".to_string(),
                confidence: 0.1,
                reason: "low_confidence".to_string(),
            }],
            archival_candidates: vec![ArchivalCandidate {
                episode_id: "episode:1".to_string(),
                reason: "old_and_empty".to_string(),
                facts_count: 0,
            }],
            archived_episodes: vec![ArchivedEpisode {
                episode_id: "episode:2".to_string(),
                archived_at: Utc::now(),
                facts_count: 5,
            }],
            stale_communities: vec![StaleCommunity {
                community_id: "comm:1".to_string(),
                last_updated: Utc::now(),
                reason: "stale".to_string(),
            }],
        };
        let val = lifecycle_fallback(&dashboard);
        assert_eq!(val["low_confidence"].as_array().unwrap().len(), 1);
        assert_eq!(val["archival_candidates"].as_array().unwrap().len(), 1);
        assert_eq!(val["archived_episodes"].as_array().unwrap().len(), 1);
        assert_eq!(val["stale_communities"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn lifecycle_fallback_includes_reasons() {
        let dashboard = LifecycleDashboard {
            low_confidence_facts: vec![LifecycleCandidate {
                fact_id: "fact:1".to_string(),
                confidence: 0.2,
                reason: "confidence_below_threshold".to_string(),
            }],
            archival_candidates: vec![],
            archived_episodes: vec![],
            stale_communities: vec![],
        };
        let val = lifecycle_fallback(&dashboard);
        assert_eq!(
            val["low_confidence"][0]["reason"],
            "confidence_below_threshold"
        );
    }

    #[test]
    fn lifecycle_dashboard_serializes() {
        let dashboard = LifecycleDashboard {
            low_confidence_facts: vec![],
            archival_candidates: vec![],
            archived_episodes: vec![],
            stale_communities: vec![],
        };
        let val = serde_json::to_value(&dashboard).unwrap();
        assert!(val.get("low_confidence_facts").is_some());
    }

    #[test]
    fn lifecycle_candidate_serializes() {
        let candidate = LifecycleCandidate {
            fact_id: "fact:1".to_string(),
            confidence: 0.3,
            reason: "test".to_string(),
        };
        let val = serde_json::to_value(&candidate).unwrap();
        assert_eq!(val["fact_id"], "fact:1");
        assert_eq!(val["confidence"], 0.3);
    }

    #[test]
    fn archival_candidate_serializes() {
        let candidate = ArchivalCandidate {
            episode_id: "episode:1".to_string(),
            reason: "old".to_string(),
            facts_count: 0,
        };
        let val = serde_json::to_value(&candidate).unwrap();
        assert_eq!(val["episode_id"], "episode:1");
    }

    #[test]
    fn archived_episode_serializes() {
        let now = Utc::now();
        let episode = ArchivedEpisode {
            episode_id: "episode:1".to_string(),
            archived_at: now,
            facts_count: 3,
        };
        let val = serde_json::to_value(&episode).unwrap();
        assert_eq!(val["episode_id"], "episode:1");
        assert_eq!(val["facts_count"], 3);
    }

    #[test]
    fn stale_community_serializes() {
        let community = StaleCommunity {
            community_id: "comm:1".to_string(),
            last_updated: Utc::now(),
            reason: "too_old".to_string(),
        };
        let val = serde_json::to_value(&community).unwrap();
        assert_eq!(val["community_id"], "comm:1");
    }

    #[test]
    fn lifecycle_fallback_formats_archived_date() {
        let dt = Utc::now();
        let dashboard = LifecycleDashboard {
            low_confidence_facts: vec![],
            archival_candidates: vec![],
            archived_episodes: vec![ArchivedEpisode {
                episode_id: "episode:1".to_string(),
                archived_at: dt,
                facts_count: 0,
            }],
            stale_communities: vec![],
        };
        let val = lifecycle_fallback(&dashboard);
        let archived_str = val["archived_episodes"][0]["reason"].as_str().unwrap();
        assert!(archived_str.starts_with("archived "));
    }
}
