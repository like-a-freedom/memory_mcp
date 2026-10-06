//! `assemble_context` must expose the claim relations of the facts it carries.
//!
//! The relations themselves are produced by the real ingest → extract → claim
//! projection → inline reconciliation pipeline, not inserted by hand: the
//! point under test is that the read path surfaces what the write path
//! actually decided. Two episodes share one explicit `source_lineage` with
//! distinct source ids, which is what ADR-0008's source gate requires before
//! it will call one claim a successor of the other.

mod common;

use chrono::{DateTime, TimeZone, Utc};
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};

/// The claim rollout stage this file runs at.
///
/// Relations are persisted at any stage that is not `disabled`, but the read
/// path only serves them at `evidence` — see `docs/evals/CLAIM_RECONCILIATION.md`
/// and `knowledge::api::SurrealRelationReader`. Without this the assertions
/// below fail for a reason unrelated to the projection being tested.
const CLAIM_STAGE: &str = "evidence";

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
        // `compact` defaults to true, which omits `reconciliation` from the
        // serialized form. The assertions read the in-memory field, but the
        // gate also has to be respected so a passing test does not depend on
        // a field the default response would not carry.
        compact: false,
    }
}

/// Persist an episode directly with an explicit `source_lineage`, then run the
/// public extract path. `ingest` derives lineage from `source_id`, and two
/// episodes with distinct source ids would therefore never satisfy the source
/// gate — lineage is the thing being pinned here, not the content.
async fn ingest_lineage_episode(
    service: &memory_mcp::service::MemoryService,
    db_client: &SurrealDbClient,
    episode_id: &str,
    source_id: &str,
    lineage: &str,
    content: &str,
    t_ref: DateTime<Utc>,
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

/// Poll until at least `want` active supersession relations exist.
///
/// Claim projection is fire-and-forget off `add_fact`, so the rows land after
/// `extract` returns. The bound is a real deadline, not a fixed sleep: it
/// yields between attempts and gives up after two hundred of them.
async fn wait_for_supersessions(db_client: &SurrealDbClient, want: usize) -> bool {
    for _ in 0..200 {
        let found = supersession_count(db_client).await;
        if found >= want {
            return true;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}

async fn supersession_count(db_client: &SurrealDbClient) -> usize {
    db_client
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
                .unwrap_or(0) as usize
        })
        .unwrap_or(0)
}

#[tokio::test]
async fn exposes_supersession_on_the_predecessor_item() {
    let tm = common::TestMemory::new(false).await;
    let service = tm
        .service
        .with_claim_rollout_stage(CLAIM_STAGE)
        .expect("evidence is a valid claim rollout stage");

    // Same lineage, distinct source ids: ADR-0008's source gate refuses
    // automatic supersession inside one source lineage, and refuses it across
    // lineages too. Both conditions are pinned here rather than assumed.
    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:sup-1",
        "fs:docs/deploy.md:aaaa",
        "fs:docs/deploy.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;

    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:sup-2",
        "fs:docs/deploy.md:bbbb",
        "fs:docs/deploy.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    let reconciled = wait_for_supersessions(&tm.db_client, 1).await;
    assert!(
        reconciled,
        "the pipeline must actually produce a supersession before the read path can be tested; got {}",
        supersession_count(&tm.db_client).await
    );

    // One assembly per query: the context cache returns the first result on a
    // second call, and every assertion here depends on seeing this pass.
    let items = AssembleContextCapability::assemble_context_from_service(&service, request("ARR"))
        .await
        .expect("context should assemble");

    assert!(
        !items.is_empty(),
        "the fixture must produce retrievable items"
    );

    let with_relations: Vec<_> = items
        .iter()
        .filter(|item| item.reconciliation.is_some())
        .collect();
    assert!(
        !with_relations.is_empty(),
        "at least one assembled item must carry its relations; all {} items had None",
        items.len()
    );

    // The claim layer's own output, not a recomputation of it.
    let summary = with_relations
        .iter()
        .find_map(|item| {
            item.reconciliation.as_ref().and_then(|metadata| {
                metadata.relations.iter().find(|relation| {
                    matches!(
                        relation.outcome,
                        memory_mcp::models::claim::ClaimRelationOutcome::Supersession
                    )
                })
            })
        })
        .expect("a Supersession summary is exposed");

    assert!(
        !summary.relation_id.is_empty(),
        "the summary must name the persisted relation"
    );
    assert!(
        !summary.reason_code.is_empty(),
        "the summary must carry the evaluator's reason code"
    );
    assert!(
        !summary.counterpart_source_episode_id.is_empty(),
        "the summary must say which episode the replacement came from"
    );

    // The predecessor names its successor; the successor names nothing, so no
    // reader can demote an item below itself.
    let predecessor = with_relations.iter().find(|item| {
        item.reconciliation.as_ref().is_some_and(|metadata| {
            metadata
                .relations
                .iter()
                .any(|relation| relation.superseded_by_fact_id.is_some())
        })
    });
    let predecessor = predecessor.expect("the losing item names its replacement");
    let target = predecessor
        .reconciliation
        .as_ref()
        .unwrap()
        .relations
        .iter()
        .find_map(|relation| relation.superseded_by_fact_id.clone())
        .unwrap();
    assert!(
        items.iter().any(|item| item.fact_id == target),
        "the named successor must itself be in the pack, otherwise the pointer is dangling"
    );
    assert_ne!(
        predecessor.fact_id, target,
        "an item must never name itself as its own replacement"
    );

    assert!(
        !predecessor
            .reconciliation
            .as_ref()
            .unwrap()
            .claim_ids
            .is_empty(),
        "claim_ids must be populated from the persisted claims, not left empty"
    );
}

#[tokio::test]
async fn leaves_reconciliation_none_when_no_relation_exists() {
    let tm = common::TestMemory::new(false).await;
    let service = tm
        .service
        .with_claim_rollout_stage(CLAIM_STAGE)
        .expect("evidence is a valid claim rollout stage");

    // A single episode: one claim, nothing to relate it to.
    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:lonely",
        "fs:docs/only.md:aaaa",
        "fs:docs/only.md",
        "ARR is standalone",
        Utc.with_ymd_and_hms(2026, 6, 5, 10, 0, 0).unwrap(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(&service, request("ARR"))
        .await
        .expect("context should assemble");

    assert!(!items.is_empty(), "the fixture must be retrievable");
    assert!(
        items.iter().all(|item| item.reconciliation.is_none()),
        "a fact in no relation must keep reconciliation at None rather than Some(empty)"
    );
}

#[tokio::test]
async fn withholds_relations_below_the_evidence_stage() {
    // The rollout contract in `docs/evals/CLAIM_RECONCILIATION.md`: `shadow`
    // projects claims and persists relations, but the read path serves none of
    // them. Without this test the gate is invisible — nothing else in the tree
    // observes the difference between `shadow` and `evidence`, so a regression
    // that exposes relations to every deployment would stay green.
    let tm = common::TestMemory::new(false).await;
    let service = tm
        .service
        .with_claim_rollout_stage("shadow")
        .expect("shadow is a valid claim rollout stage");

    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:gate-1",
        "fs:docs/gated.md:aaaa",
        "fs:docs/gated.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:gate-2",
        "fs:docs/gated.md:bbbb",
        "fs:docs/gated.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    assert!(
        wait_for_supersessions(&tm.db_client, 1).await,
        "relations are persisted at shadow too; only disclosure is withheld"
    );

    let items = AssembleContextCapability::assemble_context_from_service(&service, request("ARR"))
        .await
        .expect("context should assemble");

    assert!(!items.is_empty(), "the fixture must be retrievable");
    assert!(
        items.iter().all(|item| item.reconciliation.is_none()),
        "shadow must not disclose relations through assemble_context, got {} disclosures",
        items.iter().filter(|i| i.reconciliation.is_some()).count()
    );
}
