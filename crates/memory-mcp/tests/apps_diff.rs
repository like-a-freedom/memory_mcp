use chrono::{TimeZone, Utc};
use memory_mcp::service::DiffRequest;
use memory_mcp::service::capabilities::invalidate::InvalidateCapability;

mod common;

#[tokio::test]
async fn build_diff_reports_added_and_removed_facts_across_timepoints() {
    let service = common::make_service().await;
    let left_only_time = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap();
    let right_only_time = Utc.with_ymd_and_hms(2026, 3, 3, 9, 0, 0).unwrap();

    let left_fact = common::seed_fact_at(
        &service,
        "org",
        "Initial launch plan existed on Monday.",
        left_only_time,
    )
    .await;
    let right_fact = common::seed_fact_at(
        &service,
        "org",
        "Launch plan was updated on Wednesday.",
        right_only_time,
    )
    .await;

    InvalidateCapability::invalidate(
        &service.build_context(),
        memory_mcp::models::InvalidateRequest {
            fact_id: left_fact.clone(),
            reason: "superseded".to_string(),
            t_invalid: Utc.with_ymd_and_hms(2026, 3, 2, 12, 0, 0).unwrap(),
        },
        None,
    )
    .await
    .expect("invalidate left fact");

    let diff = service
        .build_diff(DiffRequest {
            target_type: "all".to_string(),
            target_id: None,
            as_of_left: Utc.with_ymd_and_hms(2026, 3, 1, 10, 0, 0).unwrap(),
            as_of_right: Utc.with_ymd_and_hms(2026, 3, 3, 10, 0, 0).unwrap(),
            time_axis: "valid".to_string(),
        })
        .await
        .expect("build diff");

    assert_eq!(diff.summary.added_count, 1);
    assert_eq!(diff.summary.removed_count, 1);
    assert_eq!(diff.summary.change_count, 2);
    assert!(
        diff.changes
            .iter()
            .any(|change| { change.fact_id == left_fact && change.change_type == "removed" })
    );
    assert!(
        diff.changes
            .iter()
            .any(|change| { change.fact_id == right_fact && change.change_type == "added" })
    );
}
