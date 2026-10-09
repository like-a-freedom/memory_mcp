//! Semantic fact retrieval via embeddings.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::filtering::{fact_is_active_at, fact_record_allowed};
use super::types::SemanticCandidate;
use crate::embedding::providers::embedding_from_value;
use crate::error::MemoryError;

fn validate_semantic_similarity(similarity: f64, fact_id: &str) -> Result<(), MemoryError> {
    if similarity.is_finite() && (-1.0..=1.0).contains(&similarity) {
        return Ok(());
    }

    Err(MemoryError::Storage(format!(
        "semantic score for fact {fact_id} is outside the cosine range [-1, 1]"
    )))
}

pub(crate) struct CollectSemanticFactsRequest<'a> {
    pub(crate) cutoff: DateTime<Utc>,
    pub(crate) query: &'a str,
    pub(crate) access: &'a crate::models::AccessPayload,
    pub(crate) fact_types: &'a [String],
    pub(crate) excluded_fact_ids: &'a std::collections::HashSet<String>,
    pub(crate) budget: i32,
}

pub(crate) async fn collect_semantic_facts(
    service: &crate::memory::retrieval::RetrievalContext,
    request: CollectSemanticFactsRequest<'_>,
) -> Result<Vec<SemanticCandidate>, MemoryError> {
    // The two stages a semantic retrieval spends its time in. Without them the
    // only number is the total, and a query that is slow could be waiting on
    // the embedding provider or on the vector index — two problems with
    // nothing in common.
    //
    // Scoped to the call, not to the function: bound at function scope the
    // guard would still be alive when the search below runs, so
    // `query_embedding` would measure the whole retrieval and double-count
    // `ann_search` — and a dashboard subtracting one from the other would read
    // zero, which is the discrimination these exist to provide.
    let query_embedding = {
        let _embedding_stage =
            crate::shared::observability::StageTimer::new("assemble_context", "query_embedding");
        match service
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
        }
    };

    if query_embedding.is_empty() {
        return Ok(Vec::new());
    }

    let search_limit = request.budget.max(1) * 4;

    let fact_records = {
        let _search_stage =
            crate::shared::observability::StageTimer::new("assemble_context", "ann_search");
        service
            .knowledge_store()
            .select_facts_ann(
                &crate::shared::temporal::normalize_dt(request.cutoff),
                &query_embedding,
                search_limit,
            )
            .await
    }
    .map_err(|err| MemoryError::Storage(format!("SurrealDB query error: {err}")))?;

    let mut ranked_facts = Vec::new();
    for record in fact_records {
        if !fact_record_allowed(&record, request.access, request.fact_types) {
            continue;
        }

        let Some(fact) = crate::knowledge::fact_parsing::fact_from_record(&record) else {
            continue;
        };

        if request.excluded_fact_ids.contains(&fact.fact_id)
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
                        crate::embedding::providers::cosine_similarity(&query_embedding, emb)
                    }
                    _ => 0.0,
                }
            });

        validate_semantic_similarity(similarity, &fact.fact_id)?;

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
        .map(|(similarity, fact)| SemanticCandidate {
            fact,
            rationale: format!(
                "matched semantic similarity={similarity:.3} for query=\"{}\"",
                request.query
            ),
            similarity,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::embedding::providers::EmbeddingProvider;
    use crate::embedding::providers::cosine_similarity;
    use crate::memory::retrieval::filtering;
    use crate::models::{Fact, Provenance};
    use crate::storage::{DbClient, SurrealDbClient};
    use async_trait::async_trait;
    use chrono::TimeZone;

    static NEXT_DB_ID: AtomicUsize = AtomicUsize::new(1);

    struct BoundaryProvider {
        embeddings: HashMap<String, Vec<f64>>,
    }

    #[async_trait]
    impl EmbeddingProvider for BoundaryProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "semantic-boundary-component"
        }

        fn dimension(&self) -> usize {
            1536
        }

        async fn embed(&self, input: &str) -> Result<Vec<f64>, MemoryError> {
            self.embeddings.get(input).cloned().ok_or_else(|| {
                MemoryError::Validation(format!(
                    "component provider has no vector for input of {} bytes",
                    input.len()
                ))
            })
        }
    }

    fn boundary_vector(similarity: f64) -> Vec<f64> {
        let mut vector = vec![0.0; 1536];
        vector[0] = similarity;
        vector[1] = (1.0 - similarity * similarity).sqrt();
        vector
    }

    async fn collect_threshold_boundary_candidates() -> (String, String, Vec<SemanticCandidate>) {
        let id = NEXT_DB_ID.fetch_add(1, Ordering::Relaxed);
        let db_name = format!("semantic_threshold_component_{id}");
        let db = Arc::new(
            SurrealDbClient::connect_in_memory(&db_name, "main", "warn")
                .await
                .expect("connect isolated semantic test database"),
        );
        db.apply_migrations("main")
            .await
            .expect("apply production migrations");

        let query = "quantum tarantula tariffs".to_string();
        let below_content = "A violet marmot crossed the glacier below the floor.";
        let at_floor_content = "A silver badger crossed the valley at the floor.";
        let mut query_vector = vec![0.0; 1536];
        query_vector[0] = 1.0;
        let embeddings = HashMap::from([
            (
                format!("note\n{below_content}\n{below_content}"),
                boundary_vector(0.699),
            ),
            (
                format!("note\n{at_floor_content}\n{at_floor_content}"),
                boundary_vector(0.7),
            ),
            (query.clone(), query_vector),
        ]);
        let service = crate::service::MemoryService::new_with_embedding_provider(
            db.clone() as Arc<dyn DbClient>,
            "main".to_string(),
            "warn".to_string(),
            50,
            100,
            Arc::new(BoundaryProvider { embeddings }),
            0.7,
            Arc::new(crate::service::AnnoEntityExtractor::new().expect("anno extractor")),
        )
        .expect("construct service with controlled component provider");
        let mut fact_ids = Vec::new();
        for content in [below_content, at_floor_content] {
            fact_ids.push(
                service
                    .add_fact(
                        "note",
                        content,
                        content,
                        "episode:semantic-component",
                        Utc::now(),
                        0.9,
                        Vec::new(),
                        Vec::new(),
                        Provenance::agent_observation("episode:semantic-component"),
                    )
                    .await
                    .expect("write component fact through production fact service"),
            );
        }

        let deps: crate::memory::retrieval::RetrievalContext = (&service).into();
        let cutoff = Utc::now();
        let access = crate::models::AccessPayload::default();
        let excluded_fact_ids = std::collections::HashSet::new();
        let candidates = collect_semantic_facts(
            &deps,
            CollectSemanticFactsRequest {
                cutoff,
                query: &query,
                access: &access,
                fact_types: &[],
                excluded_fact_ids: &excluded_fact_ids,
                budget: 8,
            },
        )
        .await
        .expect("run production ANN collection and threshold filtering");

        (fact_ids[0].clone(), fact_ids[1].clone(), candidates)
    }

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

    #[test]
    fn semantic_similarity_validation_rejects_non_finite_and_out_of_range_scores() {
        for similarity in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -1.01, 1.01] {
            assert!(validate_semantic_similarity(similarity, "fact:test").is_err());
        }
        assert!(validate_semantic_similarity(-1.0, "fact:test").is_ok());
        assert!(validate_semantic_similarity(1.0, "fact:test").is_ok());
    }

    #[tokio::test]
    async fn ann_threshold_rejects_0699_and_admits_exactly_0700() {
        let (below_floor_id, at_floor_id, candidates) =
            collect_threshold_boundary_candidates().await;
        let candidate_ids = candidates
            .iter()
            .map(|candidate| candidate.fact.fact_id.as_str())
            .collect::<Vec<_>>();

        assert!(!candidate_ids.contains(&below_floor_id.as_str()));
        assert!(candidate_ids.contains(&at_floor_id.as_str()));
        let admitted = candidates
            .iter()
            .find(|candidate| candidate.fact.fact_id == at_floor_id)
            .expect("exact-threshold candidate is present");
        assert!((admitted.similarity - 0.7).abs() <= 1e-9);
    }
}
