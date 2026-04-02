mod eval_support;

use chrono::{DateTime, Utc};
use eval_support::dataset::parse_retrieval_cases;
use eval_support::metrics::RetrievalSuiteSummary;
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::{AssembleContextRequest, IngestRequest};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "eval: manual retrieval quality run"]
async fn run_retrieval_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/retrieval_cases.json").unwrap();
    let cases = parse_retrieval_cases(&raw).unwrap();
    let mut summary = RetrievalSuiteSummary::default();

    for case in cases {
        summary.total_cases += 1;
        // Use GLiNER + LocalCandle embeddings for accurate eval
        let service = eval_support::common::make_service_with_gliner_and_embeddings().await;

        for episode in &case.episodes {
            let episode_id = service
                .ingest(
                    IngestRequest {
                        source_type: episode.source_type.clone(),
                        source_id: episode.source_id.clone(),
                        content: episode.content.clone(),
                        t_ref: episode.t_ref.parse::<DateTime<Utc>>().unwrap(),
                        scope: episode.scope.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
            service.extract(&episode_id, None).await.unwrap();
        }

        let items = service
            .assemble_context(AssembleContextRequest {
                query: case.query.query.clone(),
                scope: case.query.scope.clone(),
                as_of: case
                    .query
                    .as_of
                    .as_ref()
                    .map(|ts| ts.parse::<DateTime<Utc>>().unwrap()),
                budget: case.query.budget,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap();

        let contents: Vec<String> = items.iter().map(|item| item.content.clone()).collect();
        let case_ok = eval_support::metrics::record_retrieval_case(
            &mut summary,
            &case.expected.must_contain,
            &case.expected.must_not_contain,
            case.expected.expect_empty,
            &contents,
            &case.expected.tier,
        );
        assert!(case_ok, "retrieval eval case failed: {}", case.id);
    }

    print_retrieval_summary(&summary);

    let total = summary.total_cases as f64;
    let recall_at_5 = summary.recall_at_k_sum / total;
    let empty_when_irrelevant = summary.empty_when_irrelevant_hits as f64 / total;

    assert!(recall_at_5 >= 0.80, "recall_at_5 dropped below 0.80");
    assert!(
        empty_when_irrelevant >= 0.50,
        "empty_when_irrelevant dropped below 0.50"
    );
}

/// Verifies that semantic (ANN) retrieval fires when local embeddings are enabled.
/// Queries with a synonym ("revenue growth") that FTS would partially match,
/// but semantic retrieval boosts the ARR fact due to embedding similarity.
///
/// This is the default MCP scenario: GLiNER for NER + LocalCandle for embeddings.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_retrieval_fires_when_local_provider_enabled() {
    let service = crate::eval_support::common::make_service_with_local_embeddings().await;

    // Use metric-style content so that extract() creates a fact with embedding
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "note".to_string(),
                source_id: "semantic_test_1".to_string(),
                content: "ARR increased by 15 percent this quarter.".to_string(),
                t_ref: chrono::Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

    service.extract(&episode_id, None).await.expect("extract");

    // Query uses a synonym ("revenue growth") that FTS may partially match,
    // but semantic retrieval should boost the ARR fact due to embedding similarity.
    let results = service
        .assemble_context(AssembleContextRequest {
            query: "revenue growth".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    assert!(
        results
            .iter()
            .any(|item| item.content.contains("ARR") || item.content.contains("increased")),
        "semantic retrieval must surface fact about ARR increase for query 'revenue growth'"
    );
}

/// Verifies that file-based SurrealDB (RocksDB) does not degrade after many sequential ingests.
/// Uses file-based storage instead of in-memory to avoid SurrealDB in-memory instability.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_based_db_handles_25_sequential_ingests_without_degradation() {
    let (service, _temp_dir) = crate::eval_support::common::make_file_service().await;

    for i in 0..25 {
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "note".to_string(),
                    source_id: format!("stability-{i}"),
                    // Use promise-style content so extract creates facts
                    content: format!("I will complete project gamma task number {i} by Friday."),
                    t_ref: chrono::Utc::now(),
                    scope: "org".to_string(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap_or_else(|_| panic!("ingest {i} failed"));
        // Extract is required to create facts from episodes
        service
            .extract(&episode_id, None)
            .await
            .unwrap_or_else(|_| panic!("extract {i} failed"));
    }

    let results = service
        .assemble_context(AssembleContextRequest {
            query: "project gamma".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 100,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble after 25 ingests");

    assert!(
        !results.is_empty(),
        "retrieval must return results after 25 sequential ingests — file-based DB degraded"
    );
}

/// Verifies that the LocalCandle embedding provider produces vectors of the
/// expected dimension and that semantically similar texts yield higher cosine
/// similarity than unrelated texts. This is a regression guard against
/// configuration/model drift.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_candle_provider_dimension_and_similarity_regression() {
    use memory_mcp::service::{EmbeddingProvider, LocalCandleEmbeddingProvider};
    use std::path::Path;

    let embedding_model_dir = Path::new("tests/fixtures/multilingual-e5-small");
    if !embedding_model_dir.join("tokenizer.json").exists() {
        eprintln!(
            "Skipping: embedding model not found at {:?}",
            embedding_model_dir
        );
        return;
    }

    let dimension = 384usize;
    let max_tokens = 512usize;
    let provider = LocalCandleEmbeddingProvider::new(
        "intfloat/multilingual-e5-small",
        dimension,
        max_tokens,
        embedding_model_dir,
    )
    .expect("failed to create LocalCandle provider");

    // Verify configured dimension matches actual output
    assert_eq!(
        provider.dimension(),
        dimension,
        "provider dimension should match configuration"
    );

    // Verify semantic similarity ordering
    let emb_query = provider.embed("revenue growth").await.expect("embed query");
    let emb_similar = provider
        .embed("ARR increased this quarter")
        .await
        .expect("embed similar");
    let emb_unrelated = provider
        .embed("the weather is sunny today")
        .await
        .expect("embed unrelated");

    assert_eq!(emb_query.len(), dimension, "query embedding dimension");
    assert_eq!(emb_similar.len(), dimension, "similar embedding dimension");
    assert_eq!(
        emb_unrelated.len(),
        dimension,
        "unrelated embedding dimension"
    );

    // Cosine similarity: similar should be higher than unrelated
    let sim_with_similar: f64 = emb_query
        .iter()
        .zip(emb_similar.iter())
        .map(|(a, b)| a * b)
        .sum();
    let sim_with_unrelated: f64 = emb_query
        .iter()
        .zip(emb_unrelated.iter())
        .map(|(a, b)| a * b)
        .sum();

    assert!(
        sim_with_similar > sim_with_unrelated,
        "similar text should have higher cosine similarity than unrelated text: \
         similar={sim_with_similar:.4}, unrelated={sim_with_unrelated:.4}"
    );
}

/// Verifies that retrieval results are diversified across topics.
/// When multiple facts share the same source_episode or near-duplicate content,
/// the pipeline should cap them and surface results from different episodes.
///
/// This is a regression test for the "single topic dominance" problem where a generic
/// query returns 5+ results from the same topic cluster.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrieval_diversifies_across_topic_clusters() {
    let service = crate::eval_support::common::make_service().await;

    // Seed episodes on the SAME topic (cloud migration) — realistic content
    // that matches generic queries like "important decisions updates"
    let dominant_topic_episodes = [
        (
            "cloud_migration_1",
            "CRITICAL: Cloud platform migration escalation. 2 enterprise clients may cancel contracts. Revenue at risk. Regional team needs functional testing confirmation. Options: overtime or delay competing integration to next year roadmap.",
        ),
        (
            "cloud_migration_2",
            "Email Thread: Cloud Migration Escalation MAJOR. Business Impact revenue at risk from regional deals. Customer ultimatum cancel contracts. R&D Options delay competing integration. Timeline platform 3.3 end Q2 2026.",
        ),
        (
            "cloud_migration_3",
            "Cloud platform inclusion in 2026 roadmap. CRITICAL clients may cancel contracts. Revenue at risk. Regional ministries on legacy platform. Options functional testing only or overtime.",
        ),
        (
            "cloud_migration_facts",
            "FACT 1 Business Risk Cloud Migration revenue at risk regional deals. FACT 2 Customer Ultimatum clients cancel contracts. FACT 3 Region Stance prioritizes time over precision. FACT 4 R&D Option 1 delay competing integration.",
        ),
        (
            "cloud_migration_email",
            "Cloud migration escalation update from regional lead. Clients purchased licenses and may cancel without platform support. Engineering lead needs one week to work through alternative option. Third-party supplier available per node.",
        ),
    ];

    for (source_id, content) in &dominant_topic_episodes {
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "email".to_string(),
                    source_id: source_id.to_string(),
                    content: content.to_string(),
                    t_ref: chrono::Utc::now(),
                    scope: "org".to_string(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .expect("ingest");
        service.extract(&episode_id, None).await.expect("extract");
    }

    // Seed episodes on DIFFERENT topics — also match generic queries
    let other_topics = [
        (
            "license_policy_update",
            "Platform licensing policy update: premium tier users get extended API access. Legacy standalone versions will accept this license. Bug tracker item don't allow premium-only license in standalone mode.",
        ),
        (
            "sizing_calculator_update",
            "Platform sizing calculator updated with new capacity mapping for tier B. Small profile 1000 users to 500 capacity units. Medium 10000 users to 5000 capacity units. Large 20000 users to 10000 capacity units.",
        ),
        (
            "auth_rollout",
            "Mandatory multi-factor authentication rollout affects platform authentication flow. Requirement item enforce MFA across all workspaces. Impact on workspace authentication.",
        ),
        (
            "regional_sku_variants",
            "Regional platform variants with localized licensing. Core tier 1000 1-year license entry-level. Standard tier 3000 mid-market. License terms 1-year 2-year options available.",
        ),
        (
            "cross_dc_ha",
            "Cross-DC high availability support required for platform 8.x. Requirement item submitted to R&D for estimation. Automatic failover between sites on failure.",
        ),
    ];

    for (source_id, content) in &other_topics {
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "note".to_string(),
                    source_id: source_id.to_string(),
                    content: content.to_string(),
                    t_ref: chrono::Utc::now(),
                    scope: "org".to_string(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .expect("ingest");
        service.extract(&episode_id, None).await.expect("extract");
    }

    // Generic query that matches ALL topics
    let results = service
        .assemble_context(AssembleContextRequest {
            query: "important decisions updates March 2026".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    // Count how many results are from the dominant topic
    let dominant_count = results
        .iter()
        .filter(|r| {
            let content_lower = r.content.to_lowercase();
            content_lower.contains("cloud migration") || content_lower.contains("cloud platform")
        })
        .count();

    // Count unique source episodes in top results
    let unique_episodes: std::collections::HashSet<_> = results
        .iter()
        .map(|r| {
            r.provenance
                .get("source_episode")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string()
        })
        .collect();

    // Without diversification: all 5 dominant episodes dominate, dominant_count >= 5
    // With diversification: dominant topic should not dominate more than half the results
    assert!(
        dominant_count <= results.len() / 2 + 1,
        "retrieval should diversify across topics: got {} dominant-topic results out of {} total, \
         expected at most {}. Unique episodes: {}. Results: {:?}",
        dominant_count,
        results.len(),
        results.len() / 2 + 1,
        unique_episodes.len(),
        results.iter().map(|r| &r.rationale).collect::<Vec<_>>()
    );

    // Also verify we see results from non-dominant topics
    let non_dominant_count = results.len() - dominant_count;

    // Debug: print what we got
    eprintln!("=== DIVERSIFICATION TEST DEBUG ===");
    eprintln!("Total results: {}", results.len());
    eprintln!("Dominant topic results: {}", dominant_count);
    eprintln!("Non-dominant results: {}", non_dominant_count);
    eprintln!("Unique episodes: {}", unique_episodes.len());
    for (i, r) in results.iter().enumerate() {
        let is_dominant = r.content.to_lowercase().contains("cloud migration")
            || r.content.to_lowercase().contains("cloud platform");
        let source_ep = r
            .provenance
            .get("source_episode")
            .and_then(|v| v.as_str())
            .unwrap_or("?");
        eprintln!(
            "  [{}] dominant={} episode={} rationale={}",
            i,
            is_dominant,
            source_ep,
            &r.rationale[..r.rationale.len().min(80)]
        );
    }
    eprintln!("=== END DEBUG ===");

    assert!(
        non_dominant_count >= 2,
        "retrieval should include results from multiple topics: got only {} non-dominant results out of {}",
        non_dominant_count,
        results.len()
    );
}

/// Verifies that near-duplicate content from the same episode is collapsed.
/// When a single episode produces multiple extracted facts, they should not
/// all appear in the top results for a generic query.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retrieval_collapses_near_duplicates_from_same_episode() {
    let service = crate::eval_support::common::make_service_with_local_embeddings().await;

    // One episode that extracts into multiple facts (metric, decision, task)
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "email".to_string(),
                source_id: "multi_fact_episode".to_string(),
                content: "Decision: Cloud platform 3.3 support in Q2 2026. \
                          Metric: $2M revenue at risk. \
                          Task: Support cloud migration across all product lines."
                    .to_string(),
                t_ref: chrono::Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");
    service.extract(&episode_id, None).await.expect("extract");

    // Another unrelated episode
    let episode_id2 = service
        .ingest(
            IngestRequest {
                source_type: "note".to_string(),
                source_id: "other_episode".to_string(),
                content:
                    "Platform licensing policy update: premium tier users get extended API access"
                        .to_string(),
                t_ref: chrono::Utc::now(),
                scope: "org".to_string(),
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");
    service.extract(&episode_id2, None).await.expect("extract");

    let results = service
        .assemble_context(AssembleContextRequest {
            query: "important updates decisions".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble");

    // Count facts from the multi-fact episode in top results
    let multi_fact_count = results
        .iter()
        .filter(|r| {
            r.provenance
                .get("source_episode")
                .and_then(|v| v.as_str())
                .is_some_and(|ep| ep.contains(&episode_id))
        })
        .count();

    // Without diversification: all 3 facts from the same episode appear
    // With diversification: at most 1-2 facts from the same episode
    assert!(
        multi_fact_count <= 2,
        "near-duplicate facts from same episode should be collapsed: \
         got {} facts from episode {} in top {} results",
        multi_fact_count,
        episode_id,
        results.len()
    );
}
