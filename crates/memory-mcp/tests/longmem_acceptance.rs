use chrono::{Duration, TimeZone, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest};
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability;

mod common;

#[tokio::test]
async fn assemble_context_when_fact_is_needed_across_sessions_then_returns_evidence() {
    let service = common::make_service().await;

    common::ingest_episode(
        &service,
        "sess-1",
        "Alice will send the Atlas deck by Friday.",
    )
    .await;
    common::ingest_episode(&service, "sess-2", "We discussed unrelated travel plans.").await;
    common::ingest_episode(
        &service,
        "sess-3",
        "Reminder: Atlas launch is still on track.",
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "alice atlas deck".into(),
            as_of: None,
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("context should assemble");

    assert!(
        items
            .iter()
            .any(|item| item.content.contains("send the Atlas deck")),
        "expected promise evidence in returned context pack"
    );
}

#[tokio::test]
async fn assemble_context_when_question_is_unanswerable_then_returns_empty() {
    let service = common::make_service().await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "what is Bob's passport number".into(),
            as_of: None,
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("context should assemble");

    assert!(items.is_empty());
}

#[tokio::test]
async fn assemble_context_when_fact_is_invalid_after_cutoff_then_old_view_keeps_it() {
    let service = common::make_service().await;
    let now = Utc::now();
    let t_valid = now - Duration::days(1);
    let fact_id =
        common::seed_fact_at(&service, "personal", "Atlas launch was scheduled", t_valid).await;
    let invalid_at = now + Duration::days(2);

    InvalidateCapability::invalidate_from_service(
        &service,
        InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "launch rescheduled".into(),
            t_invalid: invalid_at,
        },
        None,
    )
    .await
    .expect("invalidate should succeed");

    let before_items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "atlas launch".into(),
            as_of: Some(now + Duration::days(1)),
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("historical context should assemble");
    let after_items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "atlas launch".into(),
            as_of: Some(now + Duration::days(3)),
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("future context should assemble");

    assert!(before_items.iter().any(|item| item.fact_id == fact_id));
    assert!(!after_items.iter().any(|item| item.fact_id == fact_id));
}

/// A genuine correction, distinguished from retraction by observable outcome.
///
/// The test this replaces was named for supersession but its body called
/// `invalidate` — ADR-0009's deliberate opposite, which performs *retraction*.
/// It passed, and passed for the wrong reason: it proved retraction works
/// under a name that promised supersession. The three assertions here are
/// chosen so the old body could not satisfy them:
///
/// 1. the stale value does not lead, so ordering changed;
/// 2. the stale value is still present — retraction removes it, correction
///    preserves it, and this is the assertion that separates the two;
/// 3. the stale item carries the relation that demoted it, so the reader can
///    see *why* rather than guessing.
#[tokio::test]
async fn corrected_fact_supersedes_the_stale_value_in_the_latest_view() {
    let (service, db_client) = common::make_service_with_client().await;
    let service = common::exposing_claims(service).expect("evidence is a valid stage");

    // Two revisions of one document: same lineage, distinct source ids. The
    // newer one carries the transition qualifier, so ADR-0008's source gate
    // admits a supersession rather than a contradiction.
    common::ingest_lineage_episode(
        &service,
        &db_client,
        "episode:budget-1",
        "fs:docs/budget.md:aaaa",
        "fs:docs/budget.md",
        "ARR is legacy",
        Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
    )
    .await;
    common::ingest_lineage_episode(
        &service,
        &db_client,
        "episode:budget-2",
        "fs:docs/budget.md:bbbb",
        "fs:docs/budget.md",
        "ARR is supersedes",
        Utc.with_ymd_and_hms(2026, 6, 2, 10, 0, 0).unwrap(),
    )
    .await;

    assert!(
        common::wait_for_supersessions(&db_client, 1).await,
        "the pipeline must reconcile before correction can be observed"
    );

    // Exactly one assembly: a repeat query is served from the context cache
    // and would return this same list regardless of what changed underneath.
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "ARR legacy".into(),
            as_of: None,
            budget: 10,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("context should assemble");

    assert_eq!(items.len(), 2, "both revisions must be in the pack");

    // 1. The stale value does not lead.
    assert_ne!(
        items[0].content, "ARR is legacy",
        "the superseded value must not be served first"
    );

    // 2. Correction, not retraction: the stale record survives.
    assert!(
        items.iter().any(|item| item.content == "ARR is legacy"),
        "the superseded value remains retrievable for provenance — correction never deletes"
    );

    // 3. The stale item explains itself.
    let stale = items
        .iter()
        .find(|item| item.content == "ARR is legacy")
        .expect("stale item present");
    let summary = stale
        .reconciliation
        .as_ref()
        .expect("the superseded item carries its relation")
        .relations
        .iter()
        .find(|relation| {
            matches!(
                relation.outcome,
                memory_mcp::models::claim::ClaimRelationOutcome::Supersession
            )
        })
        .expect("and the relation is a Supersession");
    assert!(
        summary.superseded_by_fact_id.is_some(),
        "the stale item names the value that replaced it"
    );
}

/// Control for the test above: retraction still works, and looks different.
///
/// `invalidate` closes a fact's validity window, so the fact disappears from
/// the latest view while its record survives. This is the behaviour the
/// replaced test was actually asserting — kept, under a name that says so,
/// because dropping it would lose coverage of `InvalidateCapability`.
#[tokio::test]
async fn invalidate_retracts_a_fact_from_the_latest_view() {
    let service = common::make_service().await;
    let old_time = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
    let old_fact_id =
        common::seed_fact_at(&service, "personal", "Atlas budget is $1M", old_time).await;

    InvalidateCapability::invalidate_from_service(
        &service,
        InvalidateRequest {
            fact_id: old_fact_id.clone(),
            reason: "budget updated".into(),
            t_invalid: Utc.with_ymd_and_hms(2026, 2, 1, 0, 0, 0).unwrap(),
        },
        None,
    )
    .await
    .expect("invalidate should succeed");

    let new_time = old_time + Duration::days(35);
    let new_fact_id =
        common::seed_fact_at(&service, "personal", "Atlas budget is $2M", new_time).await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "atlas budget".into(),
            as_of: None,
            budget: 10,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("context should assemble");

    assert!(items.iter().any(|item| item.fact_id == new_fact_id));
    assert!(!items.iter().any(|item| item.fact_id == old_fact_id));
}

#[tokio::test]
async fn assemble_context_when_direct_fact_lookup_then_returns_exact_evidence() {
    let service = common::make_service().await;
    let fact_id = common::seed_fact_at(
        &service,
        "personal",
        "Atlas deployment window is Thursday 10:00 UTC",
        Utc.with_ymd_and_hms(2026, 3, 3, 10, 0, 0).unwrap(),
    )
    .await;

    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "deployment window thursday".into(),
            as_of: None,
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await
    .expect("context should assemble");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
}
