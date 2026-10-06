//! `assemble_context` must expose the claim relations of the facts it carries.
//!
//! The relations themselves are produced by the real ingest → extract → claim
//! projection → inline reconciliation pipeline, not inserted by hand: the
//! point under test is that the read path surfaces what the write path
//! actually decided. Two episodes share one explicit `source_lineage` with
//! distinct source ids, which is what ADR-0008's source gate requires before
//! it will call one claim a successor of the other.

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
        // `compact` defaults to true, which omits `reconciliation` from the
        // serialized form. The assertions read the in-memory field, but the
        // gate also has to be respected so a passing test does not depend on
        // a field the default response would not carry.
        compact: false,
    }
}

#[tokio::test]
async fn exposes_supersession_on_the_predecessor_item() {
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).expect("evidence is a valid stage");

    // Same lineage, distinct source ids: ADR-0008's source gate refuses
    // automatic supersession inside one source lineage, and refuses it across
    // lineages too. Both conditions are pinned here rather than assumed.
    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:sup-1",
        "fs:docs/deploy.md:aaaa",
        "fs:docs/deploy.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;

    common::ingest_lineage_episode(
        &service,
        &tm.db_client,
        "episode:sup-2",
        "fs:docs/deploy.md:bbbb",
        "fs:docs/deploy.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    let reconciled = common::wait_for_supersessions(&tm.db_client, 1).await;
    assert!(
        reconciled,
        "the pipeline must actually produce a supersession before the read path can be tested; got {}",
        common::supersession_count(&tm.db_client).await
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
        summary
            .counterpart_source_episode_id
            .as_deref()
            .is_some_and(|episode| !episode.is_empty()),
        "both facts are in the pack, so the counterpart episode must be known"
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
    let service = common::exposing_claims(tm.service).expect("evidence is a valid stage");

    // A single episode: one claim, nothing to relate it to.
    common::ingest_lineage_episode(
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
    // projects claims and persists relations but serves none of them to
    // `assemble_context`; `relations` persists without disclosing; `disabled`
    // extracts nothing. All three share `exposes_evidence() == false`, so they
    // take one branch today — but asserting a single stage would leave the
    // other two unguarded, and that predicate is the only thing standing
    // between a deployment and a relation row it was never cleared for.
    for stage in ["disabled", "shadow", "relations"] {
        let tm = common::TestMemory::new(false).await;
        let service = tm
            .service
            .with_claim_rollout_stage(stage)
            .unwrap_or_else(|err| panic!("{stage} is a valid claim rollout stage: {err}"));

        common::ingest_lineage_episode(
            &service,
            &tm.db_client,
            "episode:gate-1",
            "fs:docs/gated.md:aaaa",
            "fs:docs/gated.md",
            "ARR is legacy",
            Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
        )
        .await;
        common::ingest_lineage_episode(
            &service,
            &tm.db_client,
            "episode:gate-2",
            "fs:docs/gated.md:bbbb",
            "fs:docs/gated.md",
            "ARR is supersedes",
            Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
        )
        .await;

        if stage == "disabled" {
            assert_eq!(
                common::supersession_count(&tm.db_client).await,
                0,
                "disabled must not project claims at all"
            );
        } else {
            assert!(
                common::wait_for_supersessions(&tm.db_client, 1).await,
                "{stage} persists relations; only disclosure is withheld"
            );
        }

        let items =
            AssembleContextCapability::assemble_context_from_service(&service, request("ARR"))
                .await
                .expect("context should assemble");

        assert!(
            !items.is_empty(),
            "{stage}: the fixture must still be retrievable — facts are not claim-gated"
        );
        assert!(
            items.iter().all(|item| item.reconciliation.is_none()),
            "{stage} must not disclose relations through assemble_context, got {} disclosures",
            items.iter().filter(|i| i.reconciliation.is_some()).count()
        );
    }
}

/// The relation must change a response, not merely appear in one.
///
/// Attaching metadata that nothing reads would be the same defect in a
/// smaller box as the `reconciliation: None` this phase replaced. Two
/// independent services get the identical content and the identical query:
/// one is arranged so its claims reconcile, the other so they cannot (ADR-0008
/// refuses supersession across lineages), and only the reconciled one may lead
/// with the newer value.
///
/// Two *services*, not two assemblies of one — the context cache would return
/// the first result for a repeat query and the second assertion would compare
/// the list against itself. Each service allocates its own namespace and
/// database, so neither result can be served from the other's cache.
#[tokio::test]
async fn reconciliation_changes_which_fact_leads_the_pack() {
    // --- Arm A: one lineage for both episodes, so the claims reconcile. ---
    let tm = common::TestMemory::new(false).await;
    let service = common::exposing_claims(tm.service).expect("valid stage");
    let (leader, successor, supersessions) = assemble_pair(
        &service,
        &tm.db_client,
        "fs:docs/lead.md",
        "fs:docs/lead.md",
    )
    .await;
    assert!(
        supersessions > 0,
        "arm A must reconcile, or this proves nothing about influence"
    );

    // --- Arm B: a lineage per episode, so ADR-0008 refuses supersession. ---
    let tm_control = common::TestMemory::new(false).await;
    let control = common::exposing_claims(tm_control.service).expect("valid stage");
    let (control_leader, control_successor, control_supersessions) = assemble_pair(
        &control,
        &tm_control.db_client,
        "fs:docs/x.md",
        "fs:docs/y.md",
    )
    .await;
    assert_eq!(
        control_supersessions, 0,
        "the control must not reconcile; otherwise both arms exercise the same rule"
    );
    assert!(
        control_successor.is_none(),
        "the control must name no replacement, or the arms exercise the same rule"
    );
    assert_ne!(
        control_leader, "nonexistent",
        "the control pack must actually contain items"
    );

    // Narrow contract, stated narrowly: *when a supersession exists, the newer
    // value leads.* Nothing is asserted about the control's order — it may rank
    // the newer value first on its own merits, and promising otherwise would
    // claim something the ranker never committed to.
    let successor = successor.expect("arm A names its replacement");
    assert_eq!(
        leader, successor,
        "with a supersession in the pack, the successor must lead"
    );
}

/// Ingest the pair, wait for reconciliation, and assemble the pack once.
///
/// Returns `(leader_content, successor_content, supersession_count)`. The
/// successor is read from the pack's own relation rather than assumed, so the
/// assertion compares what the claim layer decided against what the reader
/// sees; `None` when nothing in the pack names a replacement. Exactly one
/// assembly: a repeat query returns the cached first result.
async fn assemble_pair(
    service: &memory_mcp::service::MemoryService,
    db_client: &memory_mcp::storage::SurrealDbClient,
    first_lineage: &str,
    second_lineage: &str,
) -> (String, Option<String>, usize) {
    common::ingest_lineage_episode(
        service,
        db_client,
        "episode:pair-1",
        "fs:docs/pair.md:aaaa",
        first_lineage,
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    common::ingest_lineage_episode(
        service,
        db_client,
        "episode:pair-2",
        "fs:docs/pair.md:bbbb",
        second_lineage,
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    common::wait_for_supersessions(db_client, 1).await;
    let supersessions = common::supersession_count(db_client).await;

    let items =
        AssembleContextCapability::assemble_context_from_service(service, request("ARR legacy"))
            .await
            .expect("context should assemble");
    assert!(
        items.len() >= 2,
        "both revisions must be in the pack, got {}",
        items.len()
    );

    let leader = items[0].content.clone();

    // Whichever item names a replacement identifies the fact the claim layer
    // calls the successor; the successor's own entry deliberately names none.
    let target_fact = items.iter().find_map(|item| {
        item.reconciliation.as_ref().and_then(|metadata| {
            metadata
                .relations
                .iter()
                .find_map(|relation| relation.superseded_by_fact_id.clone())
                .filter(|target| items.iter().any(|i| &i.fact_id == target))
        })
    });
    let successor = target_fact.map(|target| {
        items
            .iter()
            .find(|item| item.fact_id == target)
            .expect("named successor is in the pack")
            .content
            .clone()
    });

    (leader, successor, supersessions)
}
