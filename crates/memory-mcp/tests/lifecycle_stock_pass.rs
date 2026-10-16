//! Integration tests for the stock-level lifecycle pass.
//!
//! The pass reads the real stores and stamps the inventory gauges; the
//! assertion is on the levels returned, against a real embedded database.
//!
//! Note: These tests require `--test-threads=1` due to embedded SurrealDB lock.
//! Run with: cargo test lifecycle_stock_pass -- --test-threads=1

use chrono::{Duration, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::platform::lifecycle_runtime::stock_pass;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;

mod common;

#[tokio::test]
async fn a_stock_pass_on_an_empty_store_reports_zero_levels() {
    let service = common::make_service().await;

    let levels = stock_pass(&service)
        .await
        .expect("stock pass on an empty store");

    assert_eq!(levels.active_facts, 0);
    assert_eq!(levels.archival_candidates, 0);
    assert_eq!(levels.archival_candidate_ids, Vec::<String>::new());
    assert_eq!(levels.communities, 0);
}

#[tokio::test]
async fn a_stock_pass_counts_seeded_facts_and_archival_candidates() {
    let service = common::make_service().await;

    common::seed_fact_at(
        &service,
        "org",
        "Seed fact the stock pass must count",
        Utc::now(),
    )
    .await;

    let old_date = Utc::now() - Duration::days(200);
    IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
            source_type: "meeting".to_string(),
            source_id: "stock-pass-archival-candidate".to_string(),
            content: "Old episode the stock pass must count as an archival candidate".to_string(),
            t_ref: old_date,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest episode");

    let levels = stock_pass(&service)
        .await
        .expect("stock pass after seeding");

    assert!(
        levels.active_facts >= 1,
        "the seeded fact must count as active: {levels:?}"
    );
    assert_eq!(
        levels.archival_candidates, 1,
        "the old episode must count as an archival candidate: {levels:?}"
    );
    assert_eq!(
        levels.archival_candidate_ids.len(),
        1,
        "the candidate id must travel with the count: {levels:?}"
    );
}
