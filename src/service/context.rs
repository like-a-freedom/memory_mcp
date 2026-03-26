//! Context assembly operations.

use serde_json::{Value, json};

use super::cache::{CacheKey, SafeMutex};
use super::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest, AssembledContextItem};

const RECIPROCAL_RANK_FUSION_K: f64 = 60.0;

/// Assemble context for a query.
pub async fn assemble_context(
    service: &crate::service::MemoryService,
    request: AssembleContextRequest,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let access = AccessContext::from_payload(request.access.clone());

    service.logger.log(
        super::log_event(
            "assemble_context.start",
            json!({"scope": request.scope, "query": request.query, "budget": request.budget}),
            json!({}),
            access.as_ref(),
        ),
        LogLevel::Info,
    );

    service.enforce_rate_limit(access.as_ref())?;

    if request.scope.trim().is_empty() {
        return Err(MemoryError::Validation("scope is required".into()));
    }

    let cutoff = request.as_of.unwrap_or_else(super::query::now);
    let access = access.unwrap_or_else(|| AccessContext {
        allowed_scopes: Some(vec![request.scope.clone()]),
        allowed_tags: None,
        caller_id: None,
        session_vars: None,
        transport: None,
        content_type: None,
        cross_scope_allow: None,
    });

    if !service.is_scope_allowed(&request.scope, &access) {
        return Ok(vec![]);
    }

    let cache_key = CacheKey::new(
        &request.query,
        &request.scope,
        cutoff,
        request.budget,
        access.allowed_tags.clone(),
    );

    let cached = {
        let mut cache = service.context_cache.safe_lock();
        cache.get(&cache_key).cloned()
    };

    if let Some(cached) = cached {
        service.logger.log(
            super::log_event(
                "assemble_context.cache_hit",
                json!({"scope": request.scope, "query": request.query}),
                json!({"count": cached.len()}),
                Some(&access),
            ),
            LogLevel::Info,
        );
        return Ok(cached);
    }

    let namespace = service.namespace_for_scope(&request.scope);
    let cutoff_iso = super::normalize_dt(cutoff);
    let cleaned_query = super::preprocess_search_query(&request.query);
    let query_opt = if cleaned_query.is_empty() {
        None
    } else {
        Some(cleaned_query.as_str())
    };

    let fact_records = select_fact_records_for_query(
        service,
        &namespace,
        &request.scope,
        &cutoff_iso,
        query_opt,
        request.budget,
    )
    .await?;

    let direct_facts = filter_facts_by_policy(fact_records, &access);
    let mut community_facts = Vec::new();
    if let Some(query) = query_opt {
        let direct_fact_ids: std::collections::HashSet<_> = direct_facts
            .iter()
            .map(|fact| fact.fact_id.clone())
            .collect();
        community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                namespace: &namespace,
                scope: &request.scope,
                cutoff_iso: &cutoff_iso,
                query,
                access: &access,
                direct_fact_ids: &direct_fact_ids,
                budget: request.budget,
            },
        )
        .await?;
    }

    let mut ranked_facts = build_ranked_context_facts(
        direct_facts,
        community_facts,
        query_opt,
        &request.scope,
        cutoff,
    );
    sort_ranked_context_facts(&mut ranked_facts);

    let results: Vec<AssembledContextItem> = ranked_facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|ranked| {
            let confidence = super::decayed_confidence(&ranked.fact, cutoff);
            AssembledContextItem {
                fact_id: ranked.fact.fact_id,
                content: ranked.fact.content,
                quote: ranked.fact.quote,
                source_episode: ranked.fact.source_episode,
                confidence,
                provenance: ranked.fact.provenance,
                rationale: ranked.rationale,
            }
        })
        .collect();

    {
        let mut cache = service.context_cache.safe_lock();
        cache.put(cache_key, results.clone());
    }

    service.logger.log(
        super::log_event(
            "assemble_context.cache_set",
            json!({"scope": request.scope}),
            json!({"count": results.len()}),
            Some(&access),
        ),
        LogLevel::Debug,
    );

    Ok(results)
}

/// Filter facts by policy tags.
fn filter_facts_by_policy(records: Vec<Value>, access: &AccessContext) -> Vec<crate::models::Fact> {
    let mut facts = Vec::new();

    for record in records {
        let items: Vec<&Value> = if let Some(arr) = record.get("Array").and_then(|v| v.as_array()) {
            arr.iter().collect()
        } else {
            vec![&record]
        };

        for item in items {
            let fact_item = if let Some(obj) = item.get("Object") {
                obj
            } else {
                item
            };

            if let Some(fact) = super::episode::fact_from_record(fact_item) {
                if !fact.policy_tags.is_empty()
                    && let Some(allowed_tags) = &access.allowed_tags
                {
                    let allowed: std::collections::HashSet<_> = allowed_tags.iter().collect();
                    if !fact.policy_tags.iter().any(|tag| allowed.contains(tag)) {
                        continue;
                    }
                }
                facts.push(fact);
            }
        }
    }

    facts
}

/// Sort facts by recency.
fn sort_facts_by_recency(facts: &mut [crate::models::Fact]) {
    facts.sort_by(|a, b| {
        b.t_valid
            .cmp(&a.t_valid)
            .then_with(|| b.fact_id.cmp(&a.fact_id))
    });
}

#[derive(Debug)]
struct RankedContextFact {
    fact: crate::models::Fact,
    rationale: String,
    fusion_score: f64,
    source_priority: u8,
}

fn build_ranked_context_facts(
    direct_facts: Vec<crate::models::Fact>,
    community_facts: Vec<(crate::models::Fact, String)>,
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<RankedContextFact> {
    let mut ranked_by_fact_id = std::collections::HashMap::<String, RankedContextFact>::new();

    for (rank, fact) in direct_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += reciprocal_rank(rank);
                candidate.source_priority = 0;
            })
            .or_insert_with(|| RankedContextFact {
                rationale: default_direct_rationale(query_opt, scope, cutoff),
                fact,
                fusion_score: reciprocal_rank(rank),
                source_priority: 0,
            });
    }

    for (rank, (fact, rationale)) in community_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        if let Some(candidate) = ranked_by_fact_id.get_mut(&fact_id) {
            candidate.fusion_score += reciprocal_rank(rank);
            continue;
        }

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                fact,
                rationale,
                fusion_score: reciprocal_rank(rank),
                source_priority: 1,
            },
        );
    }

    ranked_by_fact_id.into_values().collect()
}

fn reciprocal_rank(rank: usize) -> f64 {
    1.0 / (RECIPROCAL_RANK_FUSION_K + rank as f64 + 1.0)
}

fn default_direct_rationale(
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> String {
    query_opt.map_or_else(
        || {
            format!(
                "matched scope={} and active at {}",
                scope,
                cutoff.date_naive()
            )
        },
        |query| {
            format!(
                "matched lexical query=\"{}\" in scope={} and active at {}",
                query,
                scope,
                cutoff.date_naive()
            )
        },
    )
}

fn sort_ranked_context_facts(facts: &mut [RankedContextFact]) {
    facts.sort_by(|a, b| {
        b.fusion_score
            .total_cmp(&a.fusion_score)
            .then_with(|| a.source_priority.cmp(&b.source_priority))
            .then_with(|| b.fact.t_valid.cmp(&a.fact.t_valid))
            .then_with(|| a.fact.fact_id.cmp(&b.fact.fact_id))
    });
}

async fn select_fact_records_for_query(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query_opt: Option<&str>,
    limit: i32,
) -> Result<Vec<Value>, MemoryError> {
    let initial = service
        .db_client
        .select_facts_filtered(namespace, scope, cutoff_iso, query_opt, limit)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    if !initial.is_empty() {
        return Ok(initial);
    }

    let Some(query) = query_opt else {
        return Ok(initial);
    };

    let fallback_terms = query
        .split_whitespace()
        .filter(|term| !term.trim().is_empty())
        .collect::<Vec<_>>();
    if fallback_terms.len() < 2 {
        return Ok(initial);
    }

    let mut fallback_records = Vec::new();
    for term in fallback_terms {
        let term_records = service
            .db_client
            .select_facts_filtered(namespace, scope, cutoff_iso, Some(term), limit)
            .await
            .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;
        fallback_records.extend(term_records);
    }

    Ok(fallback_records)
}

struct CollectCommunityFactsRequest<'a> {
    namespace: &'a str,
    scope: &'a str,
    cutoff_iso: &'a str,
    query: &'a str,
    access: &'a AccessContext,
    direct_fact_ids: &'a std::collections::HashSet<String>,
    budget: i32,
}

async fn collect_community_facts(
    service: &crate::service::MemoryService,
    request: CollectCommunityFactsRequest<'_>,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    let matched_communities =
        find_matching_communities(service, request.namespace, request.query).await?;
    if matched_communities.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids = matched_communities
        .iter()
        .flat_map(|community| community.member_entities.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
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
        .flat_map(|community| {
            community
                .member_entities
                .iter()
                .cloned()
                .map(move |entity_id| {
                    (
                        entity_id,
                        (community.community_id.clone(), community.summary.clone()),
                    )
                })
        })
        .collect::<std::collections::HashMap<_, _>>();

    let mut facts = filter_facts_by_policy(fallback_records, request.access)
        .into_iter()
        .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
        .filter(|fact| {
            fact.entity_links
                .iter()
                .any(|entity_id| member_ids.iter().any(|member_id| member_id == entity_id))
        })
        .collect::<Vec<_>>();
    sort_facts_by_recency(&mut facts);

    Ok(facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|fact| {
            let rationale = fact
                .entity_links
                .iter()
                .find_map(|entity_id| community_summary_by_member.get(entity_id).cloned())
                .map_or_else(
                    || format!("matched community summary for query=\"{}\"", request.query),
                    |(community_id, summary)| {
                        format!(
                            "matched community summary for query=\"{}\" via {}: {}",
                            request.query, community_id, summary
                        )
                    },
                );
            (fact, rationale)
        })
        .collect())
}

#[derive(Debug)]
struct StoredCommunitySummary {
    community_id: String,
    summary: String,
    member_entities: Vec<String>,
}

async fn find_matching_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let communities = service
        .db_client
        .select_communities_matching_summary(namespace, query)
        .await?;

    Ok(communities
        .iter()
        .filter_map(stored_community_summary_from_value)
        .collect())
}

fn stored_community_summary_from_value(value: &Value) -> Option<StoredCommunitySummary> {
    let map = value.as_object()?;
    let community_id = map
        .get("community_id")
        .and_then(unwrap_context_string)
        .or_else(|| map.get("id").and_then(unwrap_context_string))?
        .to_string();
    let summary = map
        .get("summary")
        .and_then(unwrap_context_string)
        .unwrap_or_default()
        .to_string();
    let member_entities = map
        .get("member_entities")
        .and_then(unwrap_context_array)
        .map(|values| {
            values
                .iter()
                .filter_map(unwrap_context_string)
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    if summary.is_empty() || member_entities.is_empty() {
        return None;
    }

    Some(StoredCommunitySummary {
        community_id,
        summary,
        member_entities,
    })
}

fn unwrap_context_string(value: &Value) -> Option<&str> {
    if let Some(value) = value.as_str() {
        Some(value)
    } else if let Some(object) = value.as_object() {
        object
            .get("String")
            .and_then(Value::as_str)
            .or_else(|| object.get("Strand").and_then(Value::as_str))
            .or_else(|| {
                object
                    .get("Strand")
                    .and_then(|inner| inner.get("String"))
                    .and_then(Value::as_str)
            })
    } else {
        None
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{DbClient, GraphDirection};
    use chrono::Utc;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn create_test_fact(fact_id: &str, t_valid: chrono::DateTime<Utc>) -> crate::models::Fact {
        crate::models::Fact {
            fact_id: fact_id.to_string(),
            fact_type: "note".to_string(),
            content: "Test content".to_string(),
            quote: "Test quote".to_string(),
            source_episode: "episode:123".to_string(),
            t_valid,
            t_ingested: t_valid,
            t_invalid: None,
            t_invalid_ingested: None,
            confidence: 1.0,
            entity_links: vec![],
            scope: "org".to_string(),
            policy_tags: vec![],
            provenance: json!({}),
        }
    }

    #[test]
    fn sort_facts_by_recency_orders_by_date_desc() {
        let t1 = Utc::now();
        let t2 = t1 - chrono::Duration::hours(1);
        let t3 = t1 - chrono::Duration::hours(2);

        let mut facts = vec![
            create_test_fact("fact:3", t3),
            create_test_fact("fact:1", t1),
            create_test_fact("fact:2", t2),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:1");
        assert_eq!(facts[1].fact_id, "fact:2");
        assert_eq!(facts[2].fact_id, "fact:3");
    }

    #[test]
    fn sort_facts_by_recency_breaks_ties_with_id() {
        let t = Utc::now();

        let mut facts = vec![
            create_test_fact("fact:b", t),
            create_test_fact("fact:a", t),
            create_test_fact("fact:c", t),
        ];

        sort_facts_by_recency(&mut facts);

        assert_eq!(facts[0].fact_id, "fact:c"); // 'c' > 'b' > 'a'
        assert_eq!(facts[1].fact_id, "fact:b");
        assert_eq!(facts[2].fact_id, "fact:a");
    }

    #[test]
    fn sort_facts_by_recency_handles_empty() {
        let mut facts: Vec<crate::models::Fact> = vec![];
        sort_facts_by_recency(&mut facts);
        assert!(facts.is_empty());
    }

    #[test]
    fn sort_facts_by_recency_handles_single() {
        let mut facts = vec![create_test_fact("fact:1", Utc::now())];
        sort_facts_by_recency(&mut facts);
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_returns_empty_for_empty_input() {
        let access = AccessContext::default();
        let result = filter_facts_by_policy(vec![], &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_skips_invalid_records() {
        let access = AccessContext::default();
        let records = vec![json!({"invalid": "data"})];
        let result = filter_facts_by_policy(records, &access);
        assert!(result.is_empty());
    }

    #[test]
    fn filter_facts_by_policy_filters_by_allowed_tags() {
        let mut fact1 = create_test_fact("fact:1", Utc::now());
        fact1.policy_tags = vec!["allowed".to_string(), "other".to_string()];

        let mut fact2 = create_test_fact("fact:2", Utc::now());
        fact2.policy_tags = vec!["blocked".to_string()];

        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: Some(vec!["allowed".to_string()]),
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["allowed", "other"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["blocked"]
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_allows_all_when_no_tags_specified() {
        let access = AccessContext {
            allowed_scopes: None,
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };

        let records = vec![
            json!({
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org",
                "policy_tags": ["tag1"]
            }),
            json!({
                "fact_id": "fact:2",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }),
        ];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn filter_facts_by_policy_handles_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Object": {
                "fact_id": "fact:1",
                "fact_type": "note",
                "content": "Test",
                "quote": "Quote",
                "source_episode": "episode:1",
                "t_valid": "2024-01-15T10:30:00Z",
                "scope": "org"
            }
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].fact_id, "fact:1");
    }

    #[test]
    fn filter_facts_by_policy_handles_array_wrapped_objects() {
        let access = AccessContext::default();

        let records = vec![json!({
            "Array": [
                {
                    "Object": {
                        "fact_id": "fact:1",
                        "fact_type": "note",
                        "content": "Test",
                        "quote": "Quote",
                        "source_episode": "episode:1",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                },
                {
                    "Object": {
                        "fact_id": "fact:2",
                        "fact_type": "note",
                        "content": "Test2",
                        "quote": "Quote2",
                        "source_episode": "episode:2",
                        "t_valid": "2024-01-15T10:30:00Z",
                        "scope": "org"
                    }
                }
            ]
        })];

        let result = filter_facts_by_policy(records, &access);
        assert_eq!(result.len(), 2);
    }

    #[tokio::test]
    async fn assemble_context_uses_db_side_community_lookup_for_summary_matches() {
        struct CommunityLookupDbClient {
            community_lookup_calls: AtomicUsize,
            entity_link_fact_calls: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl DbClient for CommunityLookupDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                panic!("assemble_context should not scan community with select_table")
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                if query_contains.is_some() {
                    Ok(vec![])
                } else {
                    panic!(
                        "community fact expansion should not use unfiltered select_facts_filtered fallback"
                    )
                }
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                self.entity_link_fact_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(entity_links, &["entity:alice".to_string()]);

                Ok(vec![
                    json!({
                        "fact_id": "fact:community",
                        "fact_type": "note",
                        "content": "Alice works on project Atlas",
                        "quote": "Alice works on project Atlas",
                        "source_episode": "episode:1",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:alice"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:1"}
                    }),
                    json!({
                        "fact_id": "fact:other",
                        "fact_type": "note",
                        "content": "Mallory works elsewhere",
                        "quote": "Mallory works elsewhere",
                        "source_episode": "episode:2",
                        "t_valid": "2026-01-15T10:30:00Z",
                        "t_ingested": "2026-01-15T10:30:00Z",
                        "scope": "org",
                        "entity_links": ["entity:mallory"],
                        "policy_tags": [],
                        "provenance": {"source_episode": "episode:2"}
                    }),
                ])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                self.community_lookup_calls.fetch_add(1, Ordering::SeqCst);
                assert_eq!(query, "alice atlas");

                Ok(vec![json!({
                    "community_id": "community:atlas",
                    "summary": "Alice and the Atlas project team",
                    "member_entities": ["entity:alice"]
                })])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db_client = Arc::new(CommunityLookupDbClient {
            community_lookup_calls: AtomicUsize::new(0),
            entity_link_fact_calls: AtomicUsize::new(0),
        });
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice atlas".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(db_client.community_lookup_calls.load(Ordering::SeqCst), 1);
        assert_eq!(db_client.entity_link_fact_calls.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].fact_id, "fact:community");
        assert!(results[0].rationale.contains("community:atlas"));
    }

    #[tokio::test]
    async fn assemble_context_without_lexical_or_graph_matches_returns_empty() {
        struct EmptyDbClient;

        #[async_trait::async_trait]
        impl DbClient for EmptyDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                _entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                _query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let db_client = Arc::new(EmptyDbClient);
        let service = crate::service::MemoryService::new(
            db_client.clone(),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "alice platform".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert!(
            results.is_empty(),
            "without lexical or graph matches, assemble_context should return no results"
        );
    }

    #[tokio::test]
    async fn assemble_context_prefers_direct_lexical_matches_over_newer_community_expansion() {
        struct FusionDbClient;

        #[async_trait::async_trait]
        impl DbClient for FusionDbClient {
            async fn select_one(
                &self,
                _record_id: &str,
                _namespace: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_table(
                &self,
                _table: &str,
                _namespace: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_facts_filtered(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                query_contains: Option<&str>,
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(query_contains, Some("atlas launch"));
                Ok(vec![json!({
                    "fact_id": "fact:direct",
                    "fact_type": "note",
                    "content": "Atlas launch checklist is blocked on DNS cutover.",
                    "quote": "Atlas launch checklist is blocked on DNS cutover.",
                    "source_episode": "episode:direct",
                    "t_valid": "2026-01-10T10:30:00Z",
                    "t_ingested": "2026-01-10T10:30:00Z",
                    "scope": "org",
                    "entity_links": ["entity:atlas"],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:direct"},
                    "ft_score": 100.0
                })])
            }

            async fn select_facts_by_entity_links(
                &self,
                _namespace: &str,
                _scope: &str,
                _cutoff: &str,
                entity_links: &[String],
                _limit: i32,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(entity_links, &["entity:atlas".to_string()]);
                Ok(vec![json!({
                    "fact_id": "fact:community",
                    "fact_type": "note",
                    "content": "Atlas team sync moved to Friday.",
                    "quote": "Atlas team sync moved to Friday.",
                    "source_episode": "episode:community",
                    "t_valid": "2026-01-15T10:30:00Z",
                    "t_ingested": "2026-01-15T10:30:00Z",
                    "scope": "org",
                    "entity_links": ["entity:atlas"],
                    "policy_tags": [],
                    "provenance": {"source_episode": "episode:community"}
                })])
            }

            async fn select_edges_filtered(
                &self,
                _namespace: &str,
                _cutoff: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_edge_neighbors(
                &self,
                _namespace: &str,
                _node_id: &str,
                _cutoff: &str,
                _direction: GraphDirection,
            ) -> Result<Vec<Value>, MemoryError> {
                Ok(vec![])
            }

            async fn select_entity_lookup(
                &self,
                _namespace: &str,
                _normalized_name: &str,
            ) -> Result<Option<Value>, MemoryError> {
                Ok(None)
            }

            async fn select_communities_matching_summary(
                &self,
                _namespace: &str,
                query: &str,
            ) -> Result<Vec<Value>, MemoryError> {
                assert_eq!(query, "atlas launch");
                Ok(vec![json!({
                    "community_id": "community:atlas",
                    "summary": "Atlas launch workstream",
                    "member_entities": ["entity:atlas"]
                })])
            }

            async fn relate_edge(
                &self,
                _namespace: &str,
                _edge_id: &str,
                _from_id: &str,
                _to_id: &str,
                _content: Value,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn create(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn update(
                &self,
                _record_id: &str,
                _content: Value,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn query(
                &self,
                _sql: &str,
                _vars: Option<Value>,
                _namespace: &str,
            ) -> Result<Value, MemoryError> {
                Ok(Value::Null)
            }

            async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
                Ok(())
            }
        }

        let service = crate::service::MemoryService::new(
            Arc::new(FusionDbClient),
            vec!["org".to_string()],
            "warn".to_string(),
            50,
            100,
        )
        .expect("service");

        let results = assemble_context(
            &service,
            crate::models::AssembleContextRequest {
                query: "atlas launch".to_string(),
                scope: "org".to_string(),
                as_of: Some(Utc::now()),
                budget: 5,
                access: None,
            },
        )
        .await
        .expect("assemble context");

        assert_eq!(results.len(), 2);
        assert_eq!(results[0].fact_id, "fact:direct");
        assert!(
            results[0].rationale.contains("lexical"),
            "direct lexical result should explain itself as a lexical match, got: {}",
            results[0].rationale
        );
        assert_eq!(results[1].fact_id, "fact:community");
        assert!(results[1].rationale.contains("community:atlas"));
    }
}
