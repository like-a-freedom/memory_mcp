//! Context assembly operations.

use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{AccessContext, AssembleContextRequest};
use super::cache::{CacheKey, SafeMutex};
use super::error::MemoryError;

/// Assemble context for a query.
pub async fn assemble_context(
    service: &crate::service::MemoryService,
    request: AssembleContextRequest,
) -> Result<Vec<Value>, MemoryError> {
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

    // Check cache
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

    // Query database
    let namespace = service.namespace_for_scope(&request.scope);
    let cutoff_iso = super::normalize_dt(cutoff);
    let cleaned_query = super::preprocess_search_query(&request.query);
    let query_opt = if cleaned_query.is_empty() { None } else { Some(cleaned_query.as_str()) };

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

    // Filter and process results
    let mut active = filter_facts_by_policy(fact_records, &access);
    sort_facts_by_recency(&mut active);

    let results: Vec<Value> = active
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|fact| {
            json!({
                "fact_id": fact.fact_id,
                "content": fact.content,
                "quote": fact.quote,
                "source_episode": fact.source_episode,
                "confidence": super::decayed_confidence(&fact, cutoff),
                "provenance": fact.provenance,
                "rationale": format!("matched scope={} and active at {}", request.scope, cutoff.date_naive()),
            })
        })
        .collect();

    // Cache and return
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
fn filter_facts_by_policy(
    records: Vec<Value>,
    access: &AccessContext,
) -> Vec<crate::models::Fact> {
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
                // Tag filtering
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
