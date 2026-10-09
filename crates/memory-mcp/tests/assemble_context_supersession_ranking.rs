//! Wiring proof for ADR-0074: `assemble_context` actually applies the
//! reordering policy, rather than only computing it.
//!
//! The three policy rules live as unit tests in `ranking.rs`, against
//! constructed items and no database. Repeating them here would prove the
//! wiring twice and the policy three times. This file asserts exactly one
//! thing those unit tests cannot: that `assemble_context` calls the policy
//! with the rows the claim pipeline persisted.
//!
//! Each query is assembled exactly once. The context cache returns the first
//! result for a repeat query, so a second assembly would compare the reordered
//! list against itself and pass whatever the policy did.

mod common;

use chrono::{TimeZone, Utc};
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;

fn request(query: &str) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.to_string(),
        as_of: None,
        budget: 8,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    }
}

#[tokio::test]
async fn content_query_does_not_rescue_a_successor_without_query_evidence() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).expect("evidence is a valid stage");

    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:rank-1",
        "fs:docs/rank.md:aaaa",
        "fs:docs/rank.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:rank-2",
        "fs:docs/rank.md:bbbb",
        "fs:docs/rank.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    assert!(
        common::wait_for_supersessions(&tm.db_client, 1).await,
        "the pipeline must produce a supersession before ordering can be tested"
    );

    // Under the retrieval admission contract, this content query supports the
    // predecessor with both terms but the successor with only ARR. A persisted
    // relation may explain the selected fact, but it cannot grant the
    // unsupported successor an admission bypass.
    let items =
        AssembleContextCapability::assemble_context_from_service(&service, request("ARR legacy"))
            .await
            .expect("context should assemble");

    assert_eq!(
        items.len(),
        1,
        "the relation must not append a successor that failed independent admission"
    );
    assert!(
        items[0].content.contains("legacy"),
        "the only admitted result is the directly supported predecessor"
    );
}

#[tokio::test]
async fn current_query_returns_only_the_persisted_successor() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).expect("evidence is a valid stage");

    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:current-1",
        "fs:docs/current.md:aaaa",
        "fs:docs/current.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:current-2",
        "fs:docs/current.md:bbbb",
        "fs:docs/current.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;
    assert!(
        common::wait_for_supersessions(&tm.db_client, 1).await,
        "current projection must be driven by a relation persisted by reconciliation"
    );

    let all_revisions =
        AssembleContextCapability::assemble_context_from_service(&service, request("ARR"))
            .await
            .expect("ordinary context should assemble");
    let successor_id = all_revisions
        .iter()
        .find_map(|item| {
            item.reconciliation.as_ref().and_then(|metadata| {
                metadata.relations.iter().find_map(|relation| {
                    matches!(
                        relation.outcome,
                        memory_mcp::models::claim::ClaimRelationOutcome::Supersession
                    )
                    .then(|| relation.superseded_by_fact_id.clone())
                    .flatten()
                })
            })
        })
        .expect("the persisted relation identifies the actual successor");

    let mut current_request = request("latest ARR");
    current_request.budget = 1;
    let current =
        AssembleContextCapability::assemble_context_from_service(&service, current_request)
            .await
            .expect("current context should assemble");

    assert_eq!(
        current
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        vec![successor_id.as_str()],
        "a current request must return the persisted successor, not its predecessor"
    );
}
