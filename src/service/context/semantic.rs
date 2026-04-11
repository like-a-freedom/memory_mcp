//! Semantic fact retrieval via embeddings.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::Fact;
use crate::service::error::MemoryError;

use super::embedding_from_value;
use super::filtering::{fact_is_active_at, fact_record_allowed};

pub(crate) struct CollectSemanticFactsRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) query: &'a str,
    pub(crate) access: &'a crate::models::AccessContext,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) excluded_fact_ids: &'a std::collections::HashSet<String>,
    pub(crate) budget: i32,
}

pub(crate) async fn collect_semantic_facts(
    service: &crate::service::MemoryService,
    request: CollectSemanticFactsRequest<'_>,
) -> Result<Vec<(Fact, String)>, MemoryError> {
    let query_embedding = match service.generate_embedding(request.query).await {
        Ok(Some(embedding)) => embedding,
        Ok(None) => return Ok(Vec::new()),
        Err(err) => {
            service.logger.log(
                std::collections::HashMap::from([
                    (
                        "op".to_string(),
                        serde_json::json!("embedding.query_skipped"),
                    ),
                    (
                        "provider".to_string(),
                        serde_json::json!(service.embedding_provider.provider_name()),
                    ),
                    ("error".to_string(), serde_json::json!(err.to_string())),
                ]),
                crate::logging::LogLevel::Warn,
            );
            return Ok(Vec::new());
        }
    };

    if query_embedding.is_empty() {
        return Ok(Vec::new());
    }

    let search_limit = request.budget.max(1) * 4;

    let fact_records = service
        .db_client
        .select_facts_ann(
            request.namespace,
            request.scope,
            &crate::service::normalize_dt(request.cutoff),
            &query_embedding,
            search_limit,
        )
        .await
        .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut ranked_facts = Vec::new();
    for record in fact_records {
        if !fact_record_allowed(&record, request.access, request.project, request.fact_types) {
            continue;
        }

        let Some(fact) = crate::service::episode::fact_from_record(&record) else {
            continue;
        };

        if fact.scope != request.scope
            || request.excluded_fact_ids.contains(&fact.fact_id)
            || !fact_is_active_at(&fact, request.cutoff)
        {
            continue;
        }

        let similarity = record
            .as_object()
            .and_then(|map: &serde_json::Map<String, Value>| map.get("sem_score"))
            .and_then(Value::as_f64)
            .unwrap_or_else(|| {
                let embedding = record
                    .as_object()
                    .and_then(|map: &serde_json::Map<String, Value>| map.get("embedding"))
                    .and_then(embedding_from_value);
                match embedding {
                    Some(ref emb) if emb.len() == query_embedding.len() => {
                        crate::service::embedding::cosine_similarity(&query_embedding, emb)
                    }
                    _ => 0.0,
                }
            });

        if similarity < service.embedding_similarity_threshold {
            continue;
        }

        ranked_facts.push((similarity, fact));
    }

    ranked_facts.sort_by(
        |(left_similarity, left_fact), (right_similarity, right_fact)| {
            right_similarity
                .total_cmp(left_similarity)
                .then_with(|| left_fact.fact_id.cmp(&right_fact.fact_id))
        },
    );

    Ok(ranked_facts
        .into_iter()
        .take(request.budget.max(1) as usize)
        .map(|(similarity, fact)| {
            (
                fact,
                format!(
                    "matched semantic similarity={similarity:.3} for query=\"{}\"",
                    request.query
                ),
            )
        })
        .collect())
}
