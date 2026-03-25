//! Context assembly operations.

use serde_json::{Value, json};

use super::cache::{CacheKey, SafeMutex};
use super::error::MemoryError;
use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest, AssembledContextItem};

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

    let fact_records = service
        .db_client
        .select_facts_filtered(
            &namespace,
            &request.scope,
            &cutoff_iso,
            query_opt,
            request.budget,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut active = filter_facts_by_policy(fact_records, &access);
    sort_facts_by_recency(&mut active);

    let mut rationale_by_fact_id = std::collections::HashMap::new();
    if let Some(query) = query_opt {
        let direct_fact_ids: std::collections::HashSet<_> =
            active.iter().map(|fact| fact.fact_id.clone()).collect();
        let community_facts = collect_community_facts(
            service,
            &namespace,
            &request.scope,
            &cutoff_iso,
            query,
            &access,
            &direct_fact_ids,
            request.budget,
        )
        .await?;

        for (fact, rationale) in community_facts {
            rationale_by_fact_id.insert(fact.fact_id.clone(), rationale);
            active.push(fact);
        }

        sort_facts_by_recency(&mut active);
        let mut deduped = Vec::with_capacity(active.len());
        let mut seen = std::collections::HashSet::new();
        for fact in active {
            if seen.insert(fact.fact_id.clone()) {
                deduped.push(fact);
            }
        }
        active = deduped;
    }

    let results: Vec<AssembledContextItem> = active
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|fact| {
            let confidence = super::decayed_confidence(&fact, cutoff);
            let rationale = rationale_by_fact_id
                .remove(&fact.fact_id)
                .unwrap_or_else(|| {
                    format!(
                        "matched scope={} and active at {}",
                        request.scope,
                        cutoff.date_naive()
                    )
                });
            AssembledContextItem {
                fact_id: fact.fact_id,
                content: fact.content,
                quote: fact.quote,
                source_episode: fact.source_episode,
                confidence,
                provenance: fact.provenance,
                rationale,
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

async fn collect_community_facts(
    service: &crate::service::MemoryService,
    namespace: &str,
    scope: &str,
    cutoff_iso: &str,
    query: &str,
    access: &AccessContext,
    direct_fact_ids: &std::collections::HashSet<String>,
    budget: i32,
) -> Result<Vec<(crate::models::Fact, String)>, MemoryError> {
    let matched_communities = find_matching_communities(service, namespace, query).await?;
    if matched_communities.is_empty() {
        return Ok(Vec::new());
    }

    let member_ids: std::collections::HashSet<_> = matched_communities
        .iter()
        .flat_map(|community| community.member_entities.iter().cloned())
        .collect();

    let fallback_records = service
        .db_client
        .select_facts_filtered(namespace, scope, cutoff_iso, None, budget.max(1) * 10)
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut facts = filter_facts_by_policy(fallback_records, access)
        .into_iter()
        .filter(|fact| !direct_fact_ids.contains(&fact.fact_id))
        .filter(|fact| {
            fact.entity_links
                .iter()
                .any(|entity_id| member_ids.contains(entity_id))
        })
        .collect::<Vec<_>>();
    sort_facts_by_recency(&mut facts);

    let community_summary = matched_communities
        .iter()
        .map(|community| community.summary.as_str())
        .collect::<Vec<_>>()
        .join(" | ");

    Ok(facts
        .into_iter()
        .take(budget.max(1) as usize)
        .map(|fact| {
            (
                fact,
                format!(
                    "matched community summary for query=\"{}\" via {}",
                    query, community_summary
                ),
            )
        })
        .collect())
}

#[derive(Debug)]
struct StoredCommunitySummary {
    summary: String,
    member_entities: Vec<String>,
}

async fn find_matching_communities(
    service: &crate::service::MemoryService,
    namespace: &str,
    query: &str,
) -> Result<Vec<StoredCommunitySummary>, MemoryError> {
    let normalized_query = query.to_lowercase();
    let communities = service
        .db_client
        .select_table("community", namespace)
        .await?;

    Ok(communities
        .iter()
        .filter_map(stored_community_summary_from_value)
        .filter(|community| community.summary.to_lowercase().contains(&normalized_query))
        .collect())
}

fn stored_community_summary_from_value(value: &Value) -> Option<StoredCommunitySummary> {
    let map = value.as_object()?;
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
    use chrono::Utc;
    use serde_json::json;

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
            embedding: None,
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
}
