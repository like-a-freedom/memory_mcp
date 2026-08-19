//! View builders and episode fallback helpers.

use serde_json::json;
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};

use crate::logging::LogLevel;
use crate::models::{AccessPayload, AssembledContextItem, Episode};
use crate::service::error::MemoryError;
use crate::service::log_event;
use crate::service::value_helpers::json_string;

use super::filtering::{
    episode_record_allowed, fact_is_active_at, filter_facts_by_constraints, raw_object,
};
use super::types::RetrievalTier;

/// Parameters for building context items from episode fallback records.
pub(crate) struct EpisodeFallbackParams<'a, F> {
    pub(crate) episodes: Vec<Episode>,
    pub(crate) query_opt: Option<&'a str>,
    pub(crate) semantic_available: bool,
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
    F: for<'query> FnOnce(Option<&'query str>, DateTime<Utc>) -> String,
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

    episodes = dedupe_episode_fallbacks(episodes);

    let rationale_detail = (params.fallback_rationale_fn)(params.query_opt, params.cutoff);
    let query_terms = params
        .query_opt
        .map(crate::service::query::search_query_terms)
        .unwrap_or_default();

    episodes
        .into_iter()
        .take(params.budget.max(1) as usize)
        .map(|episode| {
            let lexical_score = fallback_lexical_score(&episode.content, &query_terms);
            let confidence = episode_fallback_confidence(&episode, &query_terms, params.cutoff);
            let grounding = episode_fallback_grounding(&episode.content, &query_terms);

            AssembledContextItem {
                fact_id: format!("episode_fallback:{}", episode.episode_id),
                content: episode.content.clone(),
                quote: episode.content.clone(),
                source_episode: episode.episode_id.clone(),
                confidence,
                relevance: grounding,
                grounding,
                semantic_available: Some(params.semantic_available),
                provenance: json!({
                    "source_episode": episode.episode_id,
                    "source_type": episode.source_type,
                    "source_id": episode.source_id,
                    "episode_fallback": true,
                }),
                rationale: format!(
                    "tier={} fts={:.2} access_count=0 confidence={:.2} grounding={:.2} semantic={} {}",
                    RetrievalTier::EpisodeFallback.as_str(),
                    lexical_score,
                    confidence,
                    grounding.unwrap_or(0.0),
                    if params.semantic_available {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    rationale_detail
                ),
                retrieval_tier: Some(RetrievalTier::EpisodeFallback.as_str().to_string()),
                reconciliation: None,
            }
        })
        .collect()
}

fn dedupe_episode_fallbacks(episodes: Vec<Episode>) -> Vec<Episode> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::with_capacity(episodes.len());

    for episode in episodes {
        let keys = episode_fallback_identity_keys(&episode);
        if keys.iter().any(|key| seen.contains(key)) {
            continue;
        }
        for key in keys {
            seen.insert(key);
        }
        deduped.push(episode);
    }

    deduped
}

fn episode_fallback_identity_keys(episode: &Episode) -> Vec<String> {
    let mut keys = Vec::new();

    let normalized_source_id = crate::service::normalize_text(&episode.source_id);
    if !normalized_source_id.is_empty() {
        keys.push(format!("source_id:{normalized_source_id}"));
    }

    let normalized_content = crate::service::normalize_text(&episode.content);
    if !normalized_content.is_empty() {
        keys.push(format!("content:{normalized_content}"));
    }

    if keys.is_empty() {
        keys.push(format!("episode:{}", episode.episode_id));
    }

    keys
}

fn fallback_lexical_score(content: &str, query_terms: &[String]) -> f64 {
    if query_terms.is_empty() {
        0.0
    } else {
        super::lexical::lexical_query_score_for_text(content, query_terms) as f64
    }
}

fn episode_fallback_grounding(content: &str, query_terms: &[String]) -> Option<f64> {
    if query_terms.is_empty() {
        None
    } else {
        Some(
            (fallback_lexical_score(content, query_terms) / query_terms.len() as f64)
                .clamp(0.0, 1.0),
        )
    }
}

fn episode_fallback_confidence(
    episode: &Episode,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> f64 {
    let lexical_score = fallback_lexical_score(&episode.content, query_terms);
    let lexical_coverage = if query_terms.is_empty() {
        0.0
    } else {
        (lexical_score / query_terms.len() as f64).clamp(0.0, 1.0)
    };

    let age_days = (cutoff - episode.t_ref).num_seconds().abs() as f64 / 86_400.0;
    let recency_factor = (1.0 / (1.0 + age_days / 180.0)).clamp(0.0, 1.0);

    let confidence = if query_terms.is_empty() {
        0.25 + (0.25 * recency_factor)
    } else {
        0.15 + (0.50 * lexical_coverage) + (0.20 * recency_factor)
    };

    confidence.clamp(0.15, 0.85)
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
    service: &crate::service::service_context::RetrievalContext,
    cutoff: DateTime<Utc>,
    budget: i32,
    access: &AccessPayload,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .context_store()
        .select_table("episode")
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
        if episode.t_ref > cutoff
            || episode.t_ingested > cutoff
            || !episode_record_allowed(&record, access)
        {
            continue;
        }

        let label = map
            .get("project")
            .and_then(json_string)
            .filter(|value| !value.trim().is_empty())
            .map(ToString::to_string)
            .or_else(|| episode.policy_tags.first().cloned())
            .unwrap_or_else(|| "uncategorized".to_string());

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
            ..AssembledContextItem {
                provenance: json!({
                    "facet": label,
                    "count": count,
                    "max_t_ingested": crate::service::normalize_dt(latest),
                }),
                rationale: "view_mode=facets grouped episodes by policy tags".to_string(),
                retrieval_tier: None,
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();

    service.logger.log(
        log_event(
            "assemble_context.facets_view",
            json!({}),
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
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) fact_types: &'a [String],
    pub(crate) access: &'a AccessPayload,
}

pub(crate) async fn build_wake_up_view(
    service: &crate::service::service_context::RetrievalContext,
    params: FactFilterParams<'_>,
    budget: i32,
    decayed_fn: impl Fn(&crate::models::Fact, DateTime<Utc>) -> f64,
    normalize_dt_fn: impl Fn(DateTime<Utc>) -> String,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let records = service
        .context_store()
        .select_table("fact")
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut facts = filter_facts_by_constraints(records, params.access, params.fact_types)
        .into_iter()
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
                ..AssembledContextItem {
                    provenance: fact.provenance.to_json_value(),
                    rationale: format!(
                        "view_mode=wake_up persona={} recent_t_ingested={}",
                        persona,
                        normalize_dt_fn(fact.t_ingested)
                    ),
                    retrieval_tier: None,
                    ..Default::default()
                }
            }
        })
        .collect::<Vec<_>>();

    service.logger.log(
        log_event(
            "assemble_context.wake_up_view",
            json!({"fact_type_count": params.fact_types.len()}),
            json!({"count": items.len(), "persona_count": persona_count}),
            Some(params.access),
            None,
            None,
        ),
        LogLevel::Debug,
    );

    Ok(items)
}

pub(crate) async fn build_map_view(
    service: &crate::service::service_context::RetrievalContext,
    cutoff: DateTime<Utc>,
    budget: i32,
    normalize_dt_fn: impl Fn(DateTime<Utc>) -> String,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let hub_entities = crate::service::apps::graph::find_hub_entities(
        service,
        cutoff,
        budget,
        crate::service::apps::graph::GraphTraversalBudget::FULL,
    )
    .await?;
    let communities =
        crate::service::apps::graph::list_communities(service, cutoff, budget).await?;

    service.logger.log(
        log_event(
            "assemble_context.map_view",
            json!({"budget": budget}),
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
            ..AssembledContextItem {
                provenance: json!({
                    "kind": "hub_entity",
                    "entity_id": hub.entity_id,
                    "canonical_name": hub.canonical_name,
                    "degree": hub.degree,
                }),
                rationale: "view_mode=map ranked hub entities by active graph degree".to_string(),
                retrieval_tier: None,
                ..Default::default()
            }
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
            ..AssembledContextItem {
                provenance: json!({
                    "kind": "community",
                    "community_id": community.community_id,
                    "member_entities": community.member_entities,
                    "member_count": member_count,
                    "updated_at": community.updated_at.map(&normalize_dt_fn),
                }),
                rationale: "view_mode=map listed active communities from the graph index"
                    .to_string(),
                retrieval_tier: None,
                ..Default::default()
            }
        });
    }

    items.truncate(budget.max(1) as usize);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn fallback_rationale(_query_opt: Option<&str>, _cutoff: DateTime<Utc>) -> String {
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
            semantic_available: false,
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

    #[test]
    fn build_episode_fallback_items_deduplicates_by_source_id_and_content() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
        let items = build_episode_fallback_items(EpisodeFallbackParams {
            episodes: vec![
                Episode {
                    episode_id: "episode:source-first".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "duplicate-source".to_string(),
                    content: "Release checklist summary with archive review notes.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 14, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
                Episode {
                    episode_id: "episode:source-second".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "duplicate-source".to_string(),
                    content: "Release checklist summary with revised archive review notes."
                        .to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 14, 11, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
                Episode {
                    episode_id: "episode:content-first".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "content-a".to_string(),
                    content: "Shared duplicate summary body for archive review.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 15, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
                Episode {
                    episode_id: "episode:content-second".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "content-b".to_string(),
                    content: "Shared duplicate summary body for archive review.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 15, 11, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
            ],
            query_opt: Some("release checklist archive review"),
            semantic_available: false,
            cutoff,
            window_start: None,
            window_end: None,
            timeline_mode: false,
            budget: 10,
            fallback_rationale_fn: fallback_rationale,
        });

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:source-first");
        assert_eq!(items[1].source_episode, "episode:content-first");
    }

    #[test]
    fn build_episode_fallback_items_assigns_bounded_confidence_from_query_overlap() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap();
        let items = build_episode_fallback_items(EpisodeFallbackParams {
            episodes: vec![
                Episode {
                    episode_id: "episode:strong".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "strong-match".to_string(),
                    content: "Release checklist and archive review planning notes.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 14, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
                Episode {
                    episode_id: "episode:weak".to_string(),
                    source_type: "meeting".to_string(),
                    source_id: "weak-match".to_string(),
                    content: "Planning notes with one checklist mention.".to_string(),
                    t_ref: Utc.with_ymd_and_hms(2025, 7, 10, 10, 0, 0).unwrap(),
                    t_ingested: cutoff,
                    scope: "org".to_string(),
                    visibility_scope: String::new(),
                    policy_tags: Vec::new(),
                },
            ],
            query_opt: Some("release checklist archive review"),
            semantic_available: false,
            cutoff,
            window_start: None,
            window_end: None,
            timeline_mode: false,
            budget: 10,
            fallback_rationale_fn: fallback_rationale,
        });

        assert_eq!(items.len(), 2);
        assert!(items[0].confidence > items[1].confidence);
        assert!(items.iter().all(|item| item.confidence > 0.0));
        assert!(items.iter().all(|item| item.confidence < 1.0));
        assert!(items[0].rationale.contains("tier=fallback"));
        assert!(
            items[0]
                .rationale
                .contains(&format!("confidence={:.2}", items[0].confidence)),
            "expected rationale to reflect fallback confidence, got {}",
            items[0].rationale
        );
    }
}
