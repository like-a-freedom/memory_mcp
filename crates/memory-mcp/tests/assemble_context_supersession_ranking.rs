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
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::storage::DbClient;

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

/// Persist an episode with an explicit `source_lineage`, then extract.
///
/// `ingest` derives lineage from `source_id`, and ADR-0008's source gate
/// refuses automatic supersession unless both sides share a lineage while
/// coming from different facts — so lineage is pinned here on purpose.
async fn ingest_lineage_episode(
    service: &memory_mcp::service::MemoryService,
    db_client: &memory_mcp::storage::SurrealDbClient,
    episode_id: &str,
    source_id: &str,
    lineage: &str,
    content: &str,
    t_ref: chrono::DateTime<Utc>,
) {
    let iso = t_ref.to_rfc3339();
    db_client
        .create(
            episode_id,
            serde_json::json!({
                "episode_id": episode_id,
                "source_type": "document",
                "source_id": source_id,
                "content": content,
                "t_ref": iso,
                "t_ingested": iso,
                "policy_tags": [],
                "source_lineage": lineage,
            }),
            "org",
            memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
        )
        .await
        .expect("create episode with lineage");

    ExtractCapability::extract_from_service(service, episode_id, None, None)
        .await
        .expect("extract episode with lineage");
}

async fn wait_for_supersession(db_client: &memory_mcp::storage::SurrealDbClient) -> bool {
    for _ in 0..200 {
        let found = db_client
            .query(
                "SELECT count() AS cnt FROM claim_relation WHERE outcome = 'supersession' AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)",
                None,
                "org",
            )
            .await
            .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
            .map(|rows| {
                rows.first()
                    .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                    .unwrap_or(0)
            })
            .unwrap_or(0);
        if found > 0 {
            return true;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

#[tokio::test]
async fn demotes_predecessor_below_successor_in_an_assembled_pack() {
    let tm = common::TestMemory::new(false).await;
    let service = tm
        .service
        .with_claim_rollout_stage("evidence")
        .expect("evidence is a valid claim rollout stage");

    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:rank-1",
        "fs:docs/rank.md:aaaa",
        "fs:docs/rank.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    ingest_lineage_episode(
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
        wait_for_supersession(&tm.db_client).await,
        "the pipeline must produce a supersession before ordering can be tested"
    );

    // Query terms favour the older item, so the un-reordered pack would lead
    // with it and the assertion below cannot pass on its own merits.
    let items =
        AssembleContextCapability::assemble_context_from_service(&service, request("ARR legacy"))
            .await
            .expect("context should assemble");

    let rank = |id: &str| {
        items
            .iter()
            .position(|item| item.fact_id == id)
            .unwrap_or_else(|| panic!("{id} must be in the pack"))
    };

    let superseded = items
        .iter()
        .find(|item| {
            item.reconciliation.as_ref().is_some_and(|metadata| {
                metadata.relations.iter().any(|relation| {
                    relation.superseded_by_fact_id.is_some()
                        && matches!(
                            relation.outcome,
                            memory_mcp::models::claim::ClaimRelationOutcome::Supersession
                        )
                })
            })
        })
        .expect("the pack carries the supersession relation");

    let successor = superseded
        .reconciliation
        .as_ref()
        .unwrap()
        .relations
        .iter()
        .find_map(|relation| relation.superseded_by_fact_id.clone())
        .expect("the losing item names its replacement");

    assert_eq!(
        items.len(),
        2,
        "both facts must be in the pack, or the budget decides this test instead of the policy"
    );
    assert!(
        rank(&successor) < rank(&superseded.fact_id),
        "successor must outrank predecessor, got successor at {} and predecessor at {}",
        rank(&successor),
        rank(&superseded.fact_id)
    );
}
