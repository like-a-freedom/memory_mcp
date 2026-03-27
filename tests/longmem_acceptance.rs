use chrono::{Duration, TimeZone, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest};

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

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "alice atlas deck".into(),
            scope: "personal".into(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
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

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "what is Bob's passport number".into(),
            scope: "personal".into(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
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

    service
        .invalidate(
            InvalidateRequest {
                fact_id: fact_id.clone(),
                reason: "launch rescheduled".into(),
                t_invalid: invalid_at,
            },
            None,
        )
        .await
        .expect("invalidate should succeed");

    let before_items = service
        .assemble_context(AssembleContextRequest {
            query: "atlas launch".into(),
            scope: "personal".into(),
            as_of: Some(now + Duration::days(1)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("historical context should assemble");
    let after_items = service
        .assemble_context(AssembleContextRequest {
            query: "atlas launch".into(),
            scope: "personal".into(),
            as_of: Some(now + Duration::days(3)),
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("future context should assemble");

    assert!(before_items.iter().any(|item| item.fact_id == fact_id));
    assert!(!after_items.iter().any(|item| item.fact_id == fact_id));
}

#[tokio::test]
async fn assemble_context_when_newer_fact_supersedes_older_one_then_latest_view_prefers_active_fact()
 {
    let service = common::make_service().await;
    let old_time = Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap();
    let old_fact_id =
        common::seed_fact_at(&service, "personal", "Atlas budget is $1M", old_time).await;

    service
        .invalidate(
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

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "atlas budget".into(),
            scope: "personal".into(),
            as_of: None,
            budget: 10,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
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

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "deployment window thursday".into(),
            scope: "personal".into(),
            as_of: None,
            budget: 5,
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("context should assemble");

    assert!(items.iter().any(|item| item.fact_id == fact_id));
}
