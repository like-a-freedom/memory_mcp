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
    pub(crate) access: &'a crate::models::AccessPayload,
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
    service: &crate::service::service_context::ServiceContext,
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
        .context_store()
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
    service: &crate::service::service_context::ServiceContext,
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
    service: &crate::service::service_context::ServiceContext,
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
            .context_store()
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
    service: &crate::service::service_context::ServiceContext,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let communities = service
        .context_store()
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

#[cfg(test)]
mod tests {
    use super::*;

    // -- stored_community_summary_from_value --------------------------------

    #[test]
    fn parses_full_value_with_plain_array() {
        let value = serde_json::json!({
            "community_id": "comm:42",
            "summary": "Tech enthusiasts group",
            "member_entities": ["e:1", "e:2"],
            "ft_score": 3.5,
        });
        let result = stored_community_summary_from_value(&value).expect("should parse");
        assert_eq!(result.community_id, "comm:42");
        assert_eq!(result.summary, "Tech enthusiasts group");
        assert_eq!(result.member_entities, vec!["e:1", "e:2"]);
        assert!((result.ft_score - 3.5).abs() < f64::EPSILON);
    }

    #[test]
    fn parses_with_id_fallback() {
        let value = serde_json::json!({
            "id": "comm:99",
            "summary": "Gaming community",
            "member_entities": ["e:3"],
        });
        let result = stored_community_summary_from_value(&value).expect("should use id fallback");
        assert_eq!(result.community_id, "comm:99");
    }

    #[test]
    fn parses_surrealdb_wrapped_array() {
        let value = serde_json::json!({
            "community_id": "comm:7",
            "summary": "test",
            "member_entities": {"Array": ["e:1", "e:2"]},
        });
        let result = stored_community_summary_from_value(&value).expect("should unwrap Array");
        assert_eq!(result.member_entities, vec!["e:1", "e:2"]);
    }

    #[test]
    fn returns_none_for_empty_summary() {
        let value = serde_json::json!({
            "community_id": "comm:1",
            "summary": "",
            "member_entities": ["e:1"],
        });
        assert!(stored_community_summary_from_value(&value).is_none());
    }

    #[test]
    fn returns_none_for_empty_member_entities() {
        let value = serde_json::json!({
            "community_id": "comm:1",
            "summary": "test",
            "member_entities": [],
        });
        assert!(stored_community_summary_from_value(&value).is_none());
    }

    #[test]
    fn returns_none_for_missing_community_id() {
        let value = serde_json::json!({
            "summary": "test",
            "member_entities": ["e:1"],
        });
        assert!(stored_community_summary_from_value(&value).is_none());
    }

    #[test]
    fn defaults_ft_score_to_zero() {
        let value = serde_json::json!({
            "community_id": "comm:1",
            "summary": "test",
            "member_entities": ["e:1"],
        });
        let result =
            stored_community_summary_from_value(&value).expect("should parse without ft_score");
        assert!((result.ft_score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn returns_none_for_non_object() {
        assert!(stored_community_summary_from_value(&serde_json::json!("string")).is_none());
        assert!(stored_community_summary_from_value(&serde_json::json!(42)).is_none());
        assert!(stored_community_summary_from_value(&serde_json::json!(null)).is_none());
    }

    #[test]
    fn handles_missing_member_entities_field() {
        let value = serde_json::json!({
            "community_id": "comm:1",
            "summary": "test",
        });
        assert!(stored_community_summary_from_value(&value).is_none());
    }

    // -- unwrap_context_array ----------------------------------------------

    #[test]
    fn unwrap_passes_through_plain_array() {
        let value = serde_json::json!(["a", "b"]);
        assert_eq!(unwrap_context_array(&value).map(|v| v.len()), Some(2));
    }

    #[test]
    fn unwrap_extracts_surrealdb_wrapped_array() {
        let value = serde_json::json!({"Array": ["x", "y"]});
        assert_eq!(unwrap_context_array(&value).map(|v| v.len()), Some(2));
    }

    #[test]
    fn unwrap_returns_none_for_other() {
        assert!(unwrap_context_array(&serde_json::json!("string")).is_none());
        assert!(unwrap_context_array(&serde_json::json!(42)).is_none());
    }

    // -- edge_origin_factor ------------------------------------------------

    #[test]
    fn edge_origin_non_object_returns_one() {
        assert!((edge_origin_factor(&serde_json::json!("string")) - 1.0).abs() < f64::EPSILON);
        assert!((edge_origin_factor(&serde_json::json!(42)) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_origin_extracted_returns_one() {
        let edge = serde_json::json!({
            "origin": "extracted",
            "confidence": 0.3,
        });
        assert!((edge_origin_factor(&edge) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_origin_inferred_uses_confidence() {
        let edge = serde_json::json!({
            "origin": "inferred",
            "confidence": 0.7,
        });
        assert!((edge_origin_factor(&edge) - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_origin_ambiguous_returns_half() {
        let edge = serde_json::json!({
            "origin": "ambiguous",
        });
        assert!((edge_origin_factor(&edge) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_origin_missing_returns_one() {
        let edge = serde_json::json!({});
        assert!((edge_origin_factor(&edge) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn edge_origin_confidence_clamped() {
        let edge = serde_json::json!({
            "origin": "inferred",
            "confidence": 1.5,
        });
        assert!((edge_origin_factor(&edge) - 1.0).abs() < f64::EPSILON);

        let edge = serde_json::json!({
            "origin": "inferred",
            "confidence": -0.5,
        });
        assert!((edge_origin_factor(&edge) - 0.0).abs() < f64::EPSILON);
    }

    // -- best_community_match ----------------------------------------------

    #[test]
    fn best_match_selects_lowest_rank() {
        let fact = Fact {
            entity_links: vec!["e:1".into(), "e:2".into()],
            ..make_test_fact()
        };
        let mut matches = HashMap::new();
        matches.insert(
            "e:1".into(),
            CommunityMatch {
                rank: 5,
                community_id: "c:5".into(),
                summary: "bad".into(),
            },
        );
        matches.insert(
            "e:2".into(),
            CommunityMatch {
                rank: 1,
                community_id: "c:1".into(),
                summary: "good".into(),
            },
        );
        let result = best_community_match(&fact, &matches).expect("should find match");
        assert_eq!(result.community_id, "c:1");
        assert_eq!(result.rank, 1);
    }

    #[test]
    fn best_match_none_when_no_entity_matches() {
        let fact = Fact {
            entity_links: vec!["e:99".into()],
            ..make_test_fact()
        };
        let matches = HashMap::new();
        assert!(best_community_match(&fact, &matches).is_none());
    }

    fn make_test_fact() -> Fact {
        Fact {
            fact_id: "f:1".into(),
            fact_type: "note".into(),
            content: "test".into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            t_valid: chrono::Utc::now(),
            t_ingested: chrono::Utc::now(),
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 0.9,
            index_keys: vec![],
            access_count: 0,
            last_accessed: None,
            entity_links: vec![],
            scope: "org".into(),
            policy_tags: vec![],
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    #[test]
    fn stored_community_summary_from_value_handles_wrapped_ft_score_number() {
        let summary = stored_community_summary_from_value(&serde_json::json!({
            "community_id": "community:atlas",
            "summary": "Atlas workstream",
            "member_entities": ["entity:atlas"],
            "ft_score": {"Number": 42.5}
        }))
        .expect("community summary");

        assert_eq!(summary.ft_score, 42.5);
    }
}

// ---------------------------------------------------------------------------
