//! Semantic fact retrieval via embeddings.

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::Fact;
use crate::service::embedding::embedding_from_value;
use crate::service::error::MemoryError;

use super::filtering::{fact_is_active_at, fact_record_allowed};

pub(crate) struct CollectSemanticFactsRequest<'a> {
    pub(crate) namespace: &'a str,
    pub(crate) scope: &'a str,
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) query: &'a str,
    pub(crate) access: &'a crate::models::AccessPayload,
    pub(crate) project: Option<&'a str>,
    pub(crate) fact_types: &'a [String],
    pub(crate) excluded_fact_ids: &'a std::collections::HashSet<String>,
    pub(crate) budget: i32,
}

pub(crate) async fn collect_semantic_facts(
    service: &crate::service::service_context::ServiceContext,
    request: CollectSemanticFactsRequest<'_>,
) -> Result<Vec<(Fact, String)>, MemoryError> {
    let query_embedding = match service
        .embedding_service
        .generate_query_embedding_with_background(request.query)
        .await
    {
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
                        serde_json::json!(
                            service
                                .embedding_service
                                .embedding_provider()
                                .provider_name()
                        ),
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
        .context_store()
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

        if similarity < service.embedding_service.embedding_similarity_threshold() {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::Fact;
    use crate::service::context::filtering;
    use crate::service::embedding::cosine_similarity;
    use chrono::TimeZone;

    fn make_fact(t_valid: chrono::DateTime<Utc>, t_invalid: Option<chrono::DateTime<Utc>>) -> Fact {
        Fact {
            fact_id: "fact:test".to_string(),
            t_valid,
            t_ingested: t_valid,
            t_invalid,
            t_invalid_ingested: None,
            scope: "org".to_string(),
            content: "test".to_string(),
            quote: String::new(),
            fact_type: "explicit".to_string(),
            source_episode: "episode:1".to_string(),
            confidence: 0.8,
            access_count: 0,
            last_accessed: None,
            policy_tags: Vec::new(),
            index_keys: Vec::new(),
            entity_links: Vec::new(),
            provenance: crate::models::Provenance::manual(),
            ft_score: 0.0,
        }
    }

    #[test]
    fn fact_is_active_at_true_when_not_invalidated() {
        let utc = Utc;
        let t_valid = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let cutoff = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let fact = make_fact(t_valid, None);
        assert!(filtering::fact_is_active_at(&fact, cutoff));
    }

    #[test]
    fn fact_is_active_at_false_when_invalidated_before_cutoff() {
        let utc = Utc;
        let t_valid = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let t_invalid = utc.with_ymd_and_hms(2024, 3, 1, 0, 0, 0).unwrap();
        let cutoff = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let mut fact = make_fact(t_valid, Some(t_invalid));
        fact.t_invalid_ingested = Some(t_invalid);
        assert!(!filtering::fact_is_active_at(&fact, cutoff));
    }

    #[test]
    fn fact_is_active_at_true_when_invalidated_after_cutoff() {
        let utc = Utc;
        let t_valid = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let t_invalid = utc.with_ymd_and_hms(2024, 9, 1, 0, 0, 0).unwrap();
        let cutoff = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let fact = make_fact(t_valid, Some(t_invalid));
        assert!(filtering::fact_is_active_at(&fact, cutoff));
    }

    #[test]
    fn fact_is_active_at_false_when_t_valid_after_cutoff() {
        let utc = Utc;
        let t_valid = utc.with_ymd_and_hms(2024, 9, 1, 0, 0, 0).unwrap();
        let cutoff = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let fact = make_fact(t_valid, None);
        assert!(!filtering::fact_is_active_at(&fact, cutoff));
    }

    #[test]
    fn fact_is_active_at_false_when_t_ingested_after_cutoff() {
        let utc = Utc;
        let t_valid = utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
        let cutoff = utc.with_ymd_and_hms(2024, 6, 1, 0, 0, 0).unwrap();
        let mut fact = make_fact(t_valid, None);
        fact.t_ingested = utc.with_ymd_and_hms(2024, 9, 1, 0, 0, 0).unwrap();
        assert!(!filtering::fact_is_active_at(&fact, cutoff));
    }

    #[test]
    fn cosine_similarity_identical_vectors_returns_one() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim > 0.99, "expected ~1.0, got {sim}");
    }

    #[test]
    fn cosine_similarity_orthogonal_vectors_returns_zero() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!(sim.abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_opposite_vectors_returns_minus_one() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![-1.0, 0.0, 0.0];
        let sim = cosine_similarity(&a, &b);
        assert!((sim - (-1.0)).abs() < 1e-10);
    }

    #[test]
    fn cosine_similarity_empty_vectors_returns_zero() {
        let a: Vec<f64> = vec![];
        let b: Vec<f64> = vec![];
        let sim = cosine_similarity(&a, &b);
        assert_eq!(sim, 0.0);
    }
}
