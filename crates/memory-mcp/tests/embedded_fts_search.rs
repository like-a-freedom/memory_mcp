mod embedded_support;

use chrono::{Duration, TimeZone, Utc};
use memory_mcp::models::{AssembleContextRequest, Provenance};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::resolve::ResolveCapability;

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
            Provenance::agent_observation("episode:fts_test_1"),
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
            Provenance::agent_observation("episode:fts_test_2"),
        )
        .await?;

    let ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "Delta Enrollment".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
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

    let ctx2 = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "mobile certs tokens ports episode:fts_test_2".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        !ctx2.is_empty(),
        "Query with episode ref should find facts after preprocessing (got empty)"
    );

    let ctx3 = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "cert".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
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
            Provenance::agent_observation("episode:fts_separator"),
        )
        .await?;

    let ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "atlas launch".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        !ctx.is_empty(),
        "punctuation-aware FTS should match atlas_launch for query 'atlas launch'"
    );
    assert!(ctx[0].content.contains("atlas_launch"));

    Ok(())
}

#[tokio::test]
async fn embedded_fts_matches_fact_index_keys() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc.with_ymd_and_hms(2026, 3, 15, 9, 0, 0).unwrap();
    let alice_id = service.resolve_entity("person", "Alice Smith").await?;

    service
        .add_fact(
            "note",
            "Quarterly launch review finalized.",
            "launch review finalized",
            "episode:fts_index_keys",
            t,
            "org",
            0.9,
            vec![alice_id],
            vec![],
            Provenance::agent_observation("episode:fts_index_keys"),
        )
        .await?;

    let person_ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "alice smith".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        !person_ctx.is_empty(),
        "query should match canonical entity name through fact.index_keys"
    );

    let time_ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "march 2026".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        !time_ctx.is_empty(),
        "query should match temporal marker through fact.index_keys"
    );

    Ok(())
}

#[tokio::test]
async fn embedded_fts_matches_source_id_reference_keys() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc.with_ymd_and_hms(2026, 3, 16, 9, 0, 0).unwrap();

    let fact_id = service
        .add_fact(
            "note",
            "Launch exception approved after architecture review.",
            "Launch exception approved",
            "episode:fts_reference_keys",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::extraction("episode:fts_reference_keys", "", "work-item-9794206", ""),
        )
        .await?;

    let ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "9794206".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        ctx.iter().any(|item| item.fact_id == fact_id),
        "query should match source_id-derived reference keys through fact.index_keys"
    );

    Ok(())
}

#[test]
fn schema_uses_datetime_for_fact_temporal_fields() {
    let schema = include_str!("../migrations/__Initial.surql");

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
fn schema_defines_fact_embedding_field_and_hnsw_index_only() {
    let schema = include_str!("../migrations/__Initial.surql");

    assert!(
        !schema.contains("DEFINE FIELD embedding ON episode"),
        "episode.embedding should stay absent from schema"
    );
    assert!(
        !schema.contains("DEFINE FIELD embedding ON entity"),
        "entity.embedding should stay absent from schema"
    );
    assert!(
        schema.contains("DEFINE FIELD embedding ON fact TYPE option<array<float>>;"),
        "fact.embedding should be defined for semantic retrieval"
    );
    assert!(
        !schema.contains("episode_embedding_hnsw"),
        "episode HNSW index should stay absent"
    );
    assert!(
        !schema.contains("entity_embedding_hnsw"),
        "entity HNSW index should stay absent"
    );
    assert!(
        schema.contains("fact_embedding_hnsw"),
        "fact HNSW index should be present"
    );
}

#[test]
fn schema_uses_memory_fts_analyzer() {
    let schema = include_str!("../migrations/__Initial.surql");

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
    let schema = include_str!("../migrations/__Initial.surql");

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

#[test]
fn schema_keeps_edge_origin_out_of_initial_migration() {
    let schema = include_str!("../migrations/__Initial.surql");

    assert!(
        !schema.contains("DEFINE FIELD origin ON edge TYPE string"),
        "edge origin must be introduced only via a new follow-up migration, not by editing __Initial.surql"
    );
}

#[test]
fn edge_origin_is_introduced_by_followup_migration() {
    let migration = include_str!("../migrations/017_edge_origin.surql");

    assert!(
        migration
            .contains("DEFINE FIELD OVERWRITE origin ON edge TYPE string DEFAULT 'extracted';"),
        "migration 017 should introduce the edge origin field"
    );
}

// ---------------------------------------------------------------------------
// Regression tests for the 2026-06-29 plan-review fix-up.
//
// These guard two gaps found while reviewing the implementation against the
// plan (see docs/superpowers/plans/2026-06-29-plan-review-critical-analysis.md):
//
//   1. `EntityService::find_entity_id_by_alias` used the FTS operator `@1@`
//      against a non-FULLTEXT index on `entity.aliases`, so the fuzzy resolver
//      silently never matched. The probe confirmed a real alias stored on disk
//      was returned as `[]`. Fixed to use `CONTAINS`.
//
//   2. Migration 025 defined a `memory_fts_ru` analyzer but no index referenced
//      it, so Russian-language stemming never ran. Migration 026 fixes this by
//      folding `snowball(russian)` into the shared `memory_fts` analyzer.
// ---------------------------------------------------------------------------

/// Regression for the alias-lookup bug: resolving a *different* canonical name
/// that is recorded as an alias on an existing entity must return the existing
/// entity id, not create a new one.
#[tokio::test]
async fn embedded_resolve_finds_entity_by_alias() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    // Create "Alice Smith" and attach the alias "Alicia".
    let alice_id = ResolveCapability::resolve(
        &service.build_context(),
        memory_mcp::models::EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Alice Smith".to_string(),
            aliases: vec!["Alicia".to_string()],
        },
        None,
    )
    .await?;

    // Resolving the bare canonical "Alicia" (which only matches via alias)
    // must return the same entity id, NOT create a new one.
    let alicia_id = ResolveCapability::resolve(
        &service.build_context(),
        memory_mcp::models::EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Alicia".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    assert_eq!(
        alice_id, alicia_id,
        "resolving a name that exists only as an alias should return the existing entity, \
         not create a new one (find_entity_id_by_alias regression)"
    );
    Ok(())
}

/// Regression for the Cyrillic FTS gap: a Russian-language fact must be
/// retrievable by a Russian query term that shares a stem with the stored
/// content. Before migration 026, the FTS analyzer only ran the English
/// snowball stemmer, so Russian queries matched on raw substring only.
#[tokio::test]
async fn embedded_fts_finds_russian_content() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let t = Utc::now() - Duration::days(1);

    service
        .add_fact(
            "note",
            // Use an inflected object form so a stemmer has work to do;
            // without Russian stemming the query below would not match.
            "Иван Петров работает в Газпроме, курирует архитектуру",
            "Иван работает в Газпроме",
            "episode:fts_cyrillic_ru",
            t,
            "org",
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:fts_cyrillic_ru"),
        )
        .await?;

    let ctx = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            // Query the nominative form; the stored fact has the prepositional
            // case "Газпроме". A Russian stemmer collapses both to the same stem.
            query: "Газпром".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 10,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;

    assert!(
        !ctx.is_empty(),
        "Russian query 'Газпром' should match fact containing 'Газпроме' via snowball(russian) \
         (migration 026 regression)"
    );
    Ok(())
}
