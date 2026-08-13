//! Query logging for context assembly.

use serde_json::{Value, json};

use crate::logging::LogLevel;
use crate::models::{AssembleContextRequest, AssembledContextItem};
use crate::service::error::MemoryError;
use crate::service::log_event;

pub(crate) struct QueryLogDiagnostics<'a> {
    pub(crate) resolved_view_mode: Option<&'a str>,
    pub(crate) query_flags: &'a [String],
}

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
    service: &crate::service::service_context::ServiceContext,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    diagnostics: &QueryLogDiagnostics<'_>,
) -> Result<(), MemoryError> {
    let logged_at = crate::service::query::now();
    let view_mode = request
        .view_mode
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let resolved_view_mode = diagnostics
        .resolved_view_mode
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let query_flags = diagnostics
        .query_flags
        .iter()
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let retrieval_tier = primary_retrieval_tier(results);
    let retrieval_tiers = summarize_retrieval_tiers(results);

    let record_id = format!(
        "query_log:{}",
        crate::service::hash_prefix(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}",
            crate::service::normalize_text(&request.query),
            crate::service::normalize_text(view_mode.unwrap_or_default()),
            crate::service::normalize_text(resolved_view_mode.unwrap_or_default()),
            crate::service::normalize_text(&query_flags.join(",")),
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
        ("query".to_string(), json!(request.query.clone())),
        ("result_count".to_string(), json!(results.len() as i64)),
        ("latency_ms".to_string(), json!(latency_ms)),
        ("cache_hit".to_string(), json!(cache_hit)),
    ]);

    if let Some(view_mode) = view_mode {
        payload.insert("view_mode".to_string(), json!(view_mode));
    }
    if let Some(resolved_view_mode) = resolved_view_mode {
        payload.insert("resolved_view_mode".to_string(), json!(resolved_view_mode));
    }
    payload.insert("query_flags".to_string(), json!(query_flags));
    if let Some(retrieval_tier) = retrieval_tier {
        payload.insert("retrieval_tier".to_string(), json!(retrieval_tier));
    }
    if retrieval_tiers
        .as_object()
        .is_some_and(|value| !value.is_empty())
    {
        payload.insert("retrieval_tiers".to_string(), retrieval_tiers);
    }

    service
        .context_access_log()
        .create(&record_id, Value::Object(payload))
        .await?;

    Ok(())
}

pub(crate) async fn maybe_record_query_log(
    service: &crate::service::service_context::ServiceContext,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    access: &crate::models::AccessPayload,
    diagnostics: &QueryLogDiagnostics<'_>,
) {
    if !service.is_query_logging_enabled() {
        service.logger.log(
            log_event(
                "assemble_context.query_log_skipped",
                json!({
                    "namespace": service.active_namespace,
                    "query": request.query,
                    "cache_hit": cache_hit,
                }),
                json!({"reason": "disabled"}),
                Some(access),
                None,
                None,
            ),
            LogLevel::Trace,
        );
        return;
    }

    match record_query_log(
        service,
        request,
        results,
        cache_hit,
        latency_ms,
        diagnostics,
    )
    .await
    {
        Ok(()) => {
            service.logger.log(
                log_event(
                    "assemble_context.query_log_recorded",
                    json!({
                        "namespace": service.active_namespace,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({
                        "result_count": results.len(),
                        "latency_ms": latency_ms,
                        "retrieval_tier": primary_retrieval_tier(results),
                        "resolved_view_mode": diagnostics.resolved_view_mode,
                        "query_flags": diagnostics.query_flags,
                    }),
                    Some(access),
                    None,
                    None,
                ),
                LogLevel::Debug,
            );

            match prune_expired_query_logs(service).await {
                Ok(pruned_count) if pruned_count > 0 => {
                    service.logger.log(
                        log_event(
                            "assemble_context.query_log_pruned",
                            json!({
                                "namespace": service.active_namespace,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"count": pruned_count}),
                            Some(access),
                            None,
                            None,
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
                                "namespace": service.active_namespace,
                                "retention_days": service.query_log_retention_days(),
                            }),
                            json!({"error": err.to_string()}),
                            Some(access),
                            None,
                            None,
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
                        "namespace": service.active_namespace,
                        "query": request.query,
                        "cache_hit": cache_hit,
                    }),
                    json!({"error": err.to_string()}),
                    Some(access),
                    None,
                    None,
                ),
                LogLevel::Warn,
            );
        }
    }
}

async fn prune_expired_query_logs(
    service: &crate::service::service_context::ServiceContext,
) -> Result<usize, MemoryError> {
    let cutoff = crate::service::query::now()
        - chrono::Duration::days(i64::from(service.query_log_retention_days()));
    let deleted = service
        .context_access_log()
        .query(
            "DELETE query_log WHERE logged_at IS NOT NONE AND type::datetime(logged_at) < type::datetime($cutoff) RETURN BEFORE",
            Some(json!({"cutoff": crate::service::normalize_dt(cutoff)})),
        )
        .await?;

    Ok(deleted.as_array().map_or(0, std::vec::Vec::len))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(retrieval_tier: Option<&str>, rationale: &str) -> AssembledContextItem {
        AssembledContextItem {
            fact_id: "f:1".into(),
            content: "test content".into(),
            quote: String::new(),
            source_episode: "ep:1".into(),
            confidence: 0.9,
            relevance: None,
            grounding: None,
            semantic_available: None,
            provenance: serde_json::json!({}),
            rationale: rationale.into(),
            retrieval_tier: retrieval_tier.map(str::to_string),
            reconciliation: None,
        }
    }

    // -- primary_retrieval_tier --------------------------------------------

    #[test]
    fn primary_tier_returns_first_non_empty_tier() {
        let items = vec![
            make_item(None, ""),
            make_item(Some(""), ""),
            make_item(Some("  "), ""),
            make_item(Some("direct"), ""),
            make_item(Some("graph"), ""),
        ];
        assert_eq!(primary_retrieval_tier(&items), Some("direct"));
    }

    #[test]
    fn primary_tier_none_for_all_empty() {
        let items = vec![make_item(None, ""), make_item(Some(""), "")];
        assert_eq!(primary_retrieval_tier(&items), None);
    }

    #[test]
    fn primary_tier_none_for_empty_slice() {
        assert_eq!(primary_retrieval_tier(&[]), None);
    }

    #[test]
    fn primary_tier_trims_whitespace() {
        let items = vec![make_item(Some("  direct  "), "")];
        assert_eq!(primary_retrieval_tier(&items), Some("direct"));
    }

    // -- summarize_retrieval_tiers -----------------------------------------

    #[test]
    fn summarize_tiers_counts_correctly() {
        let items = vec![
            make_item(Some("direct"), ""),
            make_item(Some("direct"), ""),
            make_item(Some("graph"), ""),
        ];
        let summary = summarize_retrieval_tiers(&items);
        let map = summary.as_object().expect("should be object");
        assert_eq!(map["direct"], 2);
        assert_eq!(map["graph"], 1);
    }

    #[test]
    fn summarize_tiers_empty_for_no_results() {
        let summary = summarize_retrieval_tiers(&[]);
        assert!(summary.as_object().is_none_or(|m| m.is_empty()));
    }

    #[test]
    fn summarize_tiers_skips_empty_and_none_tiers() {
        let items = vec![
            make_item(None, ""),
            make_item(Some(""), ""),
            make_item(Some("direct"), ""),
        ];
        let summary = summarize_retrieval_tiers(&items);
        let map = summary.as_object().expect("should be object");
        assert_eq!(map.len(), 1);
        assert_eq!(map["direct"], 1);
    }

    #[test]
    fn summarize_tiers_trims_whitespace_in_tier_name() {
        let items = vec![
            make_item(Some("  direct  "), ""),
            make_item(Some("direct"), ""),
        ];
        let summary = summarize_retrieval_tiers(&items);
        assert_eq!(summary["direct"], 2);
    }

    // -- supplemental_experience_count -------------------------------------

    #[test]
    fn experience_count_finds_supplemental_rationales() {
        let items = vec![
            make_item(
                Some("direct"),
                "supplemental experience recent_t_ingested=2026-01-01",
            ),
            make_item(
                Some("graph"),
                "supplemental experience recent_t_ingested=2026-01-02",
            ),
            make_item(Some("graph"), "regular rationale"),
        ];
        assert_eq!(supplemental_experience_count(&items), 2);
    }

    #[test]
    fn experience_count_zero_when_none() {
        let items = vec![make_item(Some("direct"), "regular rationale")];
        assert_eq!(supplemental_experience_count(&items), 0);
    }

    #[test]
    fn experience_count_zero_for_empty() {
        assert_eq!(supplemental_experience_count(&[]), 0);
    }

    #[test]
    fn experience_count_requires_exact_prefix() {
        let items = vec![
            make_item(Some("direct"), "Supplemental experience ..."),
            make_item(Some("direct"), "supplemental fact recent..."),
        ];
        assert_eq!(supplemental_experience_count(&items), 0);
    }
}
