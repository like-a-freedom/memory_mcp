//! Community fact retrieval and matching.

use std::collections::{BTreeSet, HashMap, HashSet};

use serde_json::Value;

use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::service::value_helpers::{json_f64, json_string};
use crate::storage::GraphDirection;

use super::filtering::{compare_facts_by_recency, filter_facts_by_constraints};

pub(crate) struct CollectCommunityFactsRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff_iso: &'a str,
    pub(crate) query: &'a str,
    pub(crate) access: &'a crate::models::AccessContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) direct_fact_ids: &'a HashSet<String>,
    pub(crate) budget: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct CommunityMatch {
    pub(crate) rank: usize,
    pub(crate) community_id: String,
    pub(crate) summary: String,
}

#[derive(Debug)]
pub(crate) struct StoredCommunitySummary {
    pub(crate) community_id: String,
    pub(crate) summary: String,
    pub(crate) member_entities: Vec<String>,
    pub(crate) ft_score: f64,
}

fn unwrap_context_array(value: &Value) -> Option<&Vec<Value>> {
    if let Some(array) = value.as_array() {
        Some(array)
    } else if let Some(object) = value.as_object() {
        object.get("Array").and_then(Value::as_array)
    } else {
        None
    }
}

pub(crate) async fn collect_community_facts(
    service: &crate::service::MemoryService,
    request: CollectCommunityFactsRequest<'_>,
) -> Result<Vec<(Fact, String, f64)>, MemoryError> {
    let matched_communities =
        find_matching_communities(service, request.namespace, request.query).await?;
    if matched_communities.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids = matched_communities
        .iter()
        .flat_map(|community| community.member_entities.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let fallback_records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &member_ids,
            request.budget.max(1),
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let community_summary_by_member = matched_communities
        .iter()
        .enumerate()
        .flat_map(|(rank, community)| {
            community
                .member_entities
                .iter()
                .cloned()
                .map(move |entity_id| {
                    (
                        entity_id,
                        CommunityMatch {
                            rank,
                            community_id: community.community_id.clone(),
                            summary: community.summary.clone(),
                        },
                    )
                })
        })
        .collect::<HashMap<_, _>>();

    let mut facts = filter_facts_by_constraints(
        fallback_records,
        request.access,
        request.project,
        request.fact_types,
    )
    .into_iter()
    .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
    .filter(|fact| {
        fact.entity_links
            .iter()
            .any(|entity_id| member_ids.iter().any(|member_id| member_id == entity_id))
    })
    .collect::<Vec<_>>();
    facts.sort_by(|left, right| {
        let left_rank = best_community_match(left, &community_summary_by_member)
            .map(|m| m.rank)
            .unwrap_or(usize::MAX);
        let right_rank = best_community_match(right, &community_summary_by_member)
            .map(|m| m.rank)
            .unwrap_or(usize::MAX);

        left_rank
            .cmp(&right_rank)
            .then_with(|| compare_facts_by_recency(left, right))
    });

    let mut entity_origin_factor_cache = HashMap::<String, f64>::new();

    let mut ranked_facts = Vec::new();
    for fact in facts.into_iter().take(request.budget.max(1) as usize) {
        let rationale = best_community_match(&fact, &community_summary_by_member).map_or_else(
            || format!("matched community summary for query=\"{}\"", request.query),
            |matched| {
                format!(
                    "matched community summary for query=\"{}\" via {}: {}",
                    request.query, matched.community_id, matched.summary
                )
            },
        );
        let origin_factor = community_origin_factor_for_fact(
            service,
            request.namespace,
            request.cutoff_iso,
            &fact,
            &community_summary_by_member,
            &mut entity_origin_factor_cache,
        )
        .await?;
        ranked_facts.push((fact, rationale, origin_factor));
    }

    Ok(ranked_facts)
}

fn best_community_match<'a>(
    fact: &Fact,
    matches_by_entity: &'a HashMap<String, CommunityMatch>,
) -> Option<&'a CommunityMatch> {
    fact.entity_links
        .iter()
        .filter_map(|entity_id| matches_by_entity.get(entity_id))
        .min_by(|left, right| left.rank.cmp(&right.rank))
}

async fn community_origin_factor_for_fact(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff_iso: &str,
    fact: &Fact,
    matches_by_entity: &HashMap<String, CommunityMatch>,
    entity_origin_factor_cache: &mut HashMap<String, f64>,
) -> Result<f64, MemoryError> {
    let mut best_factor: Option<f64> = None;

    for entity_id in fact
        .entity_links
        .iter()
        .filter(|e| matches_by_entity.contains_key(*e))
    {
        let factor = entity_origin_factor(
            service,
            namespace,
            cutoff_iso,
            entity_id,
            entity_origin_factor_cache,
        )
        .await?;
        best_factor = Some(best_factor.map_or(factor, |c| c.max(factor)));
    }

    Ok(best_factor.unwrap_or(1.0))
}

async fn entity_origin_factor(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff_iso: &str,
    entity_id: &str,
    cache: &mut HashMap<String, f64>,
) -> Result<f64, MemoryError> {
    if let Some(&cached) = cache.get(entity_id) {
        return Ok(cached);
    }

    let mut best_factor: Option<f64> = None;
    for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
        for edge in service
            .db_client
            .select_edge_neighbors(namespace, entity_id, cutoff_iso, direction)
            .await?
        {
            let factor = edge_origin_factor(&edge);
            best_factor = Some(best_factor.map_or(factor, |c| c.max(factor)));
        }
    }

    let factor = best_factor.unwrap_or(1.0);
    cache.insert(entity_id.to_string(), factor);
    Ok(factor)
}

fn edge_origin_factor(edge: &Value) -> f64 {
    let Some(map) = edge.as_object() else {
        return 1.0;
    };

    let confidence = map
        .get("confidence")
        .and_then(json_f64)
        .unwrap_or(0.0)
        .clamp(0.0, 1.0);

    match map.get("origin").and_then(json_string) {
        Some("extracted") => 1.0,
        Some("inferred") => confidence,
        Some("ambiguous") => 0.5,
        _ => 1.0,
    }
}

pub(crate) async fn find_matching_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let communities = service
        .db_client
        .select_communities_matching_summary(namespace, query)
        .await?;

    let mut matched = communities
        .iter()
        .filter_map(stored_community_summary_from_value)
        .collect::<Vec<_>>();
    matched.sort_by(|left, right| {
        right
            .ft_score
            .total_cmp(&left.ft_score)
            .then_with(|| left.community_id.cmp(&right.community_id))
    });

    Ok(matched)
}

pub(crate) fn stored_community_summary_from_value(value: &Value) -> Option<StoredCommunitySummary> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(json_string)
        .or_else(|| map.get("id").and_then(json_string))?
        .to_string();
    let summary = map
        .get("summary")
        .and_then(json_string)
        .unwrap_or_default()
        .to_string();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_context_array)
        .map(|values| {
            values
                .iter()
                .filter_map(json_string)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let ft_score = map.get("ft_score").and_then(json_f64).unwrap_or(0.0);

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(StoredCommunitySummary {
        community_id,
        summary,
        member_entities,
        ft_score,
    })
}
