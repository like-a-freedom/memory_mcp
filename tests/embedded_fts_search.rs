mod embedded_support;

use chrono::{Duration, Utc};
use memory_mcp::models::AssembleContextRequest;

/// Integration test: verifies that multi-word queries work through the full
/// SurrealDB stack (embedded) with the configured full-text analyzer.
#[tokio::test]
async fn embedded_multiword_fts_search() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc::now() - Duration::days(1);

    service
        .add_fact(
            "note",
            "Survey: Delta site includes enrollment workflow and gateway component on host alpha",
            "Delta Survey",
            "episode:fts_test_1",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:fts_test_1"}),
        )
        .await?;

    service
        .add_fact(
            "note",
            "Checklist entry: cert rotation scheduled, token refresh in progress, ports 5223 and 443 open",
            "cert checklist",
            "episode:fts_test_2",
            t,
            "org",
            0.85,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:fts_test_2"}),
        )
        .await?;

    let ctx = service
        .assemble_context(AssembleContextRequest {
            query: "Delta Enrollment".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx.is_empty(),
        "Multi-word FTS query 'Delta Enrollment' should find facts (got empty)"
    );
    let content = &ctx[0].content;
    assert!(
        content.contains("enrollment"),
        "Result content should contain 'enrollment', got: {content}"
    );

    let ctx2 = service
        .assemble_context(AssembleContextRequest {
            query: "mobile certs tokens ports episode:fts_test_2".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx2.is_empty(),
        "Query with episode ref should find facts after preprocessing (got empty)"
    );

    let ctx3 = service
        .assemble_context(AssembleContextRequest {
            query: "cert".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx3.is_empty(),
        "Single-word query 'cert' should still find facts (regression)"
    );

    Ok(())
}

#[tokio::test]
async fn embedded_fts_matches_separator_variants() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc::now() - Duration::days(1);

    service
        .add_fact(
            "note",
            "Deployment note: atlas_launch reached green status after final checklist.",
            "atlas_launch reached green status",
            "episode:fts_separator",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:fts_separator"}),
        )
        .await?;

    let ctx = service
        .assemble_context(AssembleContextRequest {
            query: "atlas launch".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            access: None,
        })
        .await?;

    assert!(
        !ctx.is_empty(),
        "punctuation-aware FTS should match atlas_launch for query 'atlas launch'"
    );
    assert!(ctx[0].content.contains("atlas_launch"));

    Ok(())
}

#[test]
fn schema_uses_datetime_for_fact_temporal_fields() {
    let schema = include_str!("../src/migrations/__Initial.surql");

    assert!(
        schema.contains("DEFINE FIELD t_valid ON fact TYPE datetime;"),
        "fact.t_valid should use datetime in schema"
    );
    assert!(
        schema.contains("DEFINE FIELD t_ingested ON fact TYPE datetime;"),
        "fact.t_ingested should use datetime in schema"
    );
    assert!(
        schema.contains("DEFINE FIELD t_invalid ON fact TYPE option<datetime>;"),
        "fact.t_invalid should use option<datetime> in schema"
    );
    assert!(
        schema.contains("DEFINE FIELD t_invalid_ingested ON fact TYPE option<datetime>;"),
        "fact.t_invalid_ingested should use option<datetime> in schema"
    );
}

#[test]
fn schema_removes_embedding_fields_and_hnsw_indexes() {
    let schema = include_str!("../src/migrations/__Initial.surql");

    assert!(
        !schema.contains("DEFINE FIELD embedding ON episode"),
        "episode.embedding should be removed from schema"
    );
    assert!(
        !schema.contains("DEFINE FIELD embedding ON entity"),
        "entity.embedding should be removed from schema"
    );
    assert!(
        !schema.contains("DEFINE FIELD embedding ON fact"),
        "fact.embedding should be removed from schema"
    );
    assert!(
        !schema.contains("episode_embedding_hnsw"),
        "episode HNSW index should be removed"
    );
    assert!(
        !schema.contains("entity_embedding_hnsw"),
        "entity HNSW index should be removed"
    );
    assert!(
        !schema.contains("fact_embedding_hnsw"),
        "fact HNSW index should be removed"
    );
}

#[test]
fn schema_uses_memory_fts_analyzer() {
    let schema = include_str!("../src/migrations/__Initial.surql");

    assert!(
        schema.contains("DEFINE ANALYZER memory_fts"),
        "schema should define the new memory_fts analyzer"
    );
    assert!(
        schema.contains("TOKENIZERS class"),
        "memory_fts should use class tokenization"
    );
    assert!(
        schema.contains("FILTERS lowercase, ascii, snowball(english);"),
        "memory_fts should normalize case, ascii, and English stemming"
    );
    assert!(
        schema.contains("FULLTEXT ANALYZER memory_fts"),
        "full-text indexes should use the memory_fts analyzer"
    );
}

#[test]
fn schema_uses_native_edge_endpoints() {
    let schema = include_str!("../src/migrations/__Initial.surql");

    assert!(
        schema.contains("DEFINE FIELD in ON edge"),
        "edge schema should define the native `in` endpoint"
    );
    assert!(
        schema.contains("DEFINE FIELD out ON edge"),
        "edge schema should define the native `out` endpoint"
    );
    assert!(
        schema.contains("DEFINE INDEX edge_in ON TABLE edge COLUMNS in;"),
        "edge schema should index the native `in` endpoint"
    );
    assert!(
        schema.contains("DEFINE INDEX edge_out ON TABLE edge COLUMNS out;"),
        "edge schema should index the native `out` endpoint"
    );
    assert!(
        !schema.contains("DEFINE FIELD from_id ON edge"),
        "legacy from_id field should be removed from edge schema"
    );
    assert!(
        !schema.contains("DEFINE FIELD to_id ON edge"),
        "legacy to_id field should be removed from edge schema"
    );
}
