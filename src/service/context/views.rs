//! View builders and episode fallback helpers.

use serde_json::json;
use std::collections::HashMap;

use chrono::{DateTime, Utc};

use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembledContextItem, Episode};
use crate::service::error::MemoryError;
use crate::service::log_event;
use crate::service::value_helpers::json_string;

use super::filtering::{
    episode_record_allowed, fact_is_active_at, filter_facts_by_constraints, raw_object,
};
use super::ranking::RetrievalTier;

/// Parameters for building context items from episode fallback records.
pub(crate) struct EpisodeFallbackParams<'a, F> {
    pub(crate) episodes: Vec<Episode>,
    pub(crate) query_opt: Option<&'a str>,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) window_start: Option<DateTime<Utc>>,
    pub(crate) window_end: Option<DateTime<Utc>>,
    pub(crate) timeline_mode: bool,
    pub(crate) budget: i32,
    pub(crate) fallback_rationale_fn: F,
}

pub(crate) fn build_episode_fallback_items<F>(
    params: EpisodeFallbackParams<'_, F>,
) -> Vec<AssembledContextItem>
where
    F: FnOnce(Option<&str>, &str, DateTime<Utc>) -> String,
{
    let mut episodes = params.episodes;
    apply_episode_time_window(&mut episodes, params.window_start, params.window_end);

    if params.timeline_mode {
        episodes.sort_by(|left, right| {
            left.t_ref
                .cmp(&right.t_ref)
                .then_with(|| left.episode_id.cmp(&right.episode_id))
        });
    } else if params.query_opt.is_none() {
        episodes.sort_by(|left, right| {
            right
                .t_ref
                .cmp(&left.t_ref)
                .then_with(|| left.episode_id.cmp(&right.episode_id))
        });
    }

    let rationale = (params.fallback_rationale_fn)(params.query_opt, params.scope, params.cutoff);

    episodes
        .into_iter()
        .take(params.budget.max(1) as usize)
        .map(|episode| AssembledContextItem {
            fact_id: format!("episode_fallback:{}", episode.episode_id),
            content: episode.content.clone(),
            quote: episode.content.clone(),
            source_episode: episode.episode_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "source_episode": episode.episode_id,
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "episode_fallback": true,
            }),
            rationale: rationale.clone(),
            retrieval_tier: Some(RetrievalTier::EpisodeFallback.as_str().to_string()),
        })
        .collect()
}

fn apply_episode_time_window(
    episodes: &mut Vec<Episode>,
    window_start: Option<DateTime<Utc>>,
    window_end: Option<DateTime<Utc>>,
) {
    if window_start.is_none() && window_end.is_none() {
        return;
    }

    episodes.retain(|episode| {
        let after_start = window_start.is_none_or(|start| episode.t_ref >= start);
        let before_end = window_end.is_none_or(|end| episode.t_ref <= end);
        after_start && before_end
    });
}

pub(crate) async fn build_facets_view(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff: DateTime<Utc>,
    project: Option<&str>,
    budget: i32,
    access: &AccessContext,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .db_client
        .select_table("episode", namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut buckets = HashMap::<String, (usize, DateTime<Utc>)>::new();

    for record in records {
        let Some(map) = raw_object(&record) else {
            continue;
        };
        let Some(episode) = crate::service::episode::episode_from_record(map) else {
            continue;
        };
        if episode.scope != scope
            || episode.t_ref > cutoff
            || episode.t_ingested > cutoff
            || !episode_record_allowed(&record, access, project)
        {
            continue;
        }

        let label = map
            .get("project")
            .and_then(json_string)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| episode.policy_tags.first().cloned())
            .unwrap_or_else(|| scope.to_string());

        buckets
            .entry(label)
            .and_modify(|(count, latest)| {
                *count += 1;
                *latest = (*latest).max(episode.t_ingested);
            })
            .or_insert((1, episode.t_ingested));
    }

    let mut buckets = buckets.into_iter().collect::<Vec<_>>();
    buckets.sort_by(
        |(left_label, (_, left_latest)), (right_label, (_, right_latest))| {
            right_latest
                .cmp(left_latest)
                .then_with(|| left_label.cmp(right_label))
        },
    );

    let items = buckets
        .into_iter()
        .take(budget.max(1) as usize)
        .map(|(label, (count, latest))| AssembledContextItem {
            fact_id: format!("facet:{label}"),
            content: label.clone(),
            quote: format!("{count} episodes"),
            source_episode: format!("facet:{label}"),
            confidence: 1.0,
            provenance: json!({
                "facet": label,
                "count": count,
                "max_t_ingested": crate::service::normalize_dt(latest),
            }),
            rationale: "view_mode=facets grouped episodes by project/policy/scope".to_string(),
            retrieval_tier: None,
        })
        .collect::<Vec<_>>();

    service.logger.log(
        log_event(
            "assemble_context.facets_view",
            json!({"scope": scope, "project": project}),
            json!({"count": items.len()}),
            Some(access),
            None,
            None,
        ),
        LogLevel::Debug,
    );

    Ok(items)
}

pub(crate) struct FactFilterParams<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) access: &'a AccessContext,
}

pub(crate) async fn build_wake_up_view(
    service: &crate::service::MemoryService,
    params: FactFilterParams<'_>,
    budget: i32,
    decayed_fn: impl Fn(&crate::models::Fact, DateTime<Utc>) -> f64,
    normalize_dt_fn: impl Fn(DateTime<Utc>) -> String,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .db_client
        .select_table("fact", params.namespace)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut facts =
        filter_facts_by_constraints(records, params.access, params.project, params.fact_types)
            .into_iter()
            .filter(|fact| fact.scope == params.scope)
            .filter(|fact| fact_is_active_at(fact, params.cutoff))
            .collect::<Vec<_>>();

    facts.sort_by(|left, right| {
        let left_persona = left.policy_tags.iter().any(|tag| tag == "persona");
        let right_persona = right.policy_tags.iter().any(|tag| tag == "persona");
        right_persona
            .cmp(&left_persona)
            .then_with(|| right.t_ingested.cmp(&left.t_ingested))
            .then_with(|| right.t_valid.cmp(&left.t_valid))
            .then_with(|| left.fact_id.cmp(&right.fact_id))
    });

    let persona_count = facts
        .iter()
        .filter(|fact| fact.policy_tags.iter().any(|tag| tag == "persona"))
        .count();

    let items = facts
        .into_iter()
        .take(budget.max(1) as usize)
        .map(|fact| {
            let persona = fact.policy_tags.iter().any(|tag| tag == "persona");
            let confidence = if persona {
                fact.confidence.max(decayed_fn(&fact, params.cutoff))
            } else {
                decayed_fn(&fact, params.cutoff)
            };
            AssembledContextItem {
                fact_id: fact.fact_id,
                content: fact.content,
                quote: fact.quote,
                source_episode: fact.source_episode,
                confidence,
                provenance: fact.provenance,
                rationale: format!(
                    "view_mode=wake_up persona={} recent_t_ingested={}",
                    persona,
                    normalize_dt_fn(fact.t_ingested)
                ),
                retrieval_tier: None,
            }
        })
        .collect::<Vec<_>>();

    service.logger.log(
        log_event(
            "assemble_context.wake_up_view",
            json!({"scope": params.scope, "project": params.project, "fact_type_count": params.fact_types.len()}),
            json!({"count": items.len(), "persona_count": persona_count}),
            Some(params.access), None, None,
        ),
        LogLevel::Debug,
    );

    Ok(items)
}

pub(crate) async fn build_map_view(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff: DateTime<Utc>,
    budget: i32,
    normalize_dt_fn: impl Fn(DateTime<Utc>) -> String,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let hub_entities =
        crate::service::apps::graph::find_hub_entities(service, namespace, cutoff, budget).await?;
    let communities =
        crate::service::apps::graph::list_communities(service, namespace, cutoff, budget).await?;

    service.logger.log(
        log_event(
            "assemble_context.map_view",
            json!({"namespace": namespace, "budget": budget}),
            json!({"hub_entities": hub_entities.len(), "communities": communities.len()}),
            None,
            None,
            None,
        ),
        LogLevel::Debug,
    );

    let mut items = Vec::with_capacity(hub_entities.len() + communities.len());

    for hub in hub_entities {
        items.push(AssembledContextItem {
            fact_id: format!("map:hub:{}", hub.entity_id),
            content: hub.canonical_name.clone(),
            quote: format!("{} connections", hub.degree),
            source_episode: hub.entity_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "kind": "hub_entity",
                "entity_id": hub.entity_id,
                "canonical_name": hub.canonical_name,
                "degree": hub.degree,
            }),
            rationale: "view_mode=map ranked hub entities by active graph degree".to_string(),
            retrieval_tier: None,
        });
    }

    for community in communities {
        let member_count = community.member_entities.len();
        items.push(AssembledContextItem {
            fact_id: format!("map:community:{}", community.community_id),
            content: community.summary.clone(),
            quote: format!("{member_count} members"),
            source_episode: community.community_id.clone(),
            confidence: 1.0,
            provenance: json!({
                "kind": "community",
                "community_id": community.community_id,
                "member_entities": community.member_entities,
                "member_count": member_count,
                "updated_at": community.updated_at.map(&normalize_dt_fn),
            }),
            rationale: "view_mode=map listed active communities from the graph index".to_string(),
            retrieval_tier: None,
        });
    }

    items.truncate(budget.max(1) as usize);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fallback_rationale(
        _query_opt: Option<&str>,
        _scope: &str,
        _cutoff: DateTime<Utc>,
    ) -> String {
        "fallback".to_string()
    }

    #[test]
    fn build_episode_fallback_items_preserves_input_order_for_query_mode() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
        let items = build_episode_fallback_items(EpisodeFallbackParams {
            episodes: vec![
                Episode {
                    episode_id: "episode:exact".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "fallback-exact".to_string(),
                    content: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 14, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
                Episode {
                    episode_id: "episode:generic".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "fallback-generic".to_string(),
                    content: "Platform notes July 2025 with rollout reminders.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 15, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
            ],
            query_opt: Some("platform planning notes july 2025"),
            scope: "org",
            cutoff,
            window_start: None,
            window_end: None,
            timeline_mode: false,
            budget: 5,
            fallback_rationale_fn: fallback_rationale,
        });

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:exact");
        assert_eq!(items[1].source_episode, "episode:generic");
    }
}
