//! Query logging for context assembly.

use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{AssembleContextRequest, AssembledContextItem};
use crate::service::error::MemoryError;
use crate::service::log_event;

pub(crate) fn primary_retrieval_tier(results: &[AssembledContextItem]) -> Option<&str> {
    results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .find(|value| !value.is_empty())
}

pub(crate) fn summarize_retrieval_tiers(results: &[AssembledContextItem]) -> Value {
    let mut counts = serde_json::Map::new();

    for tier in results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let next = counts
            .get(tier)
            .and_then(Value::as_u64)
            .unwrap_or(0)
            .saturating_add(1);
        counts.insert(tier.to_string(), json!(next));
    }

    Value::Object(counts)
}

pub(crate) fn supplemental_experience_count(results: &[AssembledContextItem]) -> usize {
    results
        .iter()
        .filter(|item| item.rationale.starts_with("supplemental experience "))
        .count()
}

pub(crate) async fn record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
) -> Result<(), MemoryError> {
    let namespace = service.namespace_for_scope(&request.scope);
    let logged_at = crate::service::query::now();
    let project = request
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let retrieval_tier = results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .find(|value| !value.is_empty());

    let record_id = format!(
        "query_log:{}",
        crate::service::hash_prefix(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            crate::service::normalize_text(&request.scope),
            crate::service::normalize_text(&request.query),
            crate::service::normalize_text(project.unwrap_or_default()),
            crate::service::normalize_text(view_mode.unwrap_or_default()),
            crate::service::normalize_text(retrieval_tier.unwrap_or_default()),
            results.len(),
            if cache_hit { "1" } else { "0" },
            crate::service::normalize_dt(logged_at),
        ))
    );

    let mut payload = serde_json::Map::from_iter([
        ("query_log_id".to_string(), json!(record_id.clone())),
        (
            "logged_at".to_string(),
            json!(crate::service::normalize_dt(logged_at)),
        ),
        ("scope".to_string(), json!(request.scope.clone())),
        ("query".to_string(), json!(request.query.clone())),
        ("result_count".to_string(), json!(results.len() as i64)),
        ("latency_ms".to_string(), json!(latency_ms)),
        ("cache_hit".to_string(), json!(cache_hit)),
    ]);

    if let Some(project) = project {
        payload.insert("project".to_string(), json!(project));
    }
    if let Some(view_mode) = view_mode {
        payload.insert("view_mode".to_string(), json!(view_mode));
    }
    if let Some(retrieval_tier) = retrieval_tier {
        payload.insert("retrieval_tier".to_string(), json!(retrieval_tier));
    }

    service
        .db_client
        .create(&record_id, Value::Object(payload), &namespace)
        .await?;

    Ok(())
}

pub(crate) async fn maybe_record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    access: &crate::models::AccessContext,
) {
    if !service.is_query_logging_enabled() {
        service.logger.log(
            log_event(
                "assemble_context.query_log_skipped",
                json!({
                    "scope": request.scope,
                    "query": request.query,
                    "cache_hit": cache_hit,
                }),
                json!({"reason": "disabled"}),
                Some(access),
            ),
            LogLevel::Trace,
        );
        return;
    }

    match record_query_log(service, request, results, cache_hit, latency_ms).await {
        Ok(()) => {
            service.logger.log(
                log_event(
                    "assemble_context.query_log_recorded",
                    json!({
                        "scope": request.scope,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({
                        "result_count": results.len(),
                        "latency_ms": latency_ms,
                        "retrieval_tier": primary_retrieval_tier(results),
                    }),
                    Some(access),
                ),
                LogLevel::Debug,
            );

            match prune_expired_query_logs(service, &request.scope).await {
                Ok(pruned_count) if pruned_count > 0 => {
                    service.logger.log(
                        log_event(
                            "assemble_context.query_log_pruned",
                            json!({
                                "scope": request.scope,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"count": pruned_count}),
                            Some(access),
                        ),
                        LogLevel::Trace,
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    service.logger.log(
                        log_event(
                            "assemble_context.query_log_prune_error",
                            json!({
                                "scope": request.scope,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"error": err.to_string()}),
                            Some(access),
                        ),
                        LogLevel::Warn,
                    );
                }
            }
        }
        Err(err) => {
            service.logger.log(
                log_event(
                    "assemble_context.query_log_error",
                    json!({
                        "scope": request.scope,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({"error": err.to_string()}),
                    Some(access),
                ),
                LogLevel::Warn,
            );
        }
    }
}

async fn prune_expired_query_logs(
    service: &crate::service::MemoryService,
    scope: &str,
) -> Result<usize, MemoryError> {
    let namespace = service.namespace_for_scope(scope);
    let cutoff = crate::service::query::now()
        - chrono::Duration::days(i64::from(service.query_log_retention_days()));
    let deleted = service
        .db_client
        .query(
            "DELETE query_log WHERE logged_at IS NOT NONE AND type::datetime(logged_at) < type::datetime($cutoff) RETURN BEFORE",
            Some(json!({"cutoff": crate::service::normalize_dt(cutoff)})),
            &namespace,
        )
        .await?;

    Ok(deleted.as_array().map_or(0, std::vec::Vec::len))
}
