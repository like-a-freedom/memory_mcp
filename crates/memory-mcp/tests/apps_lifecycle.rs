use chrono::{TimeZone, Utc};
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::storage::DbClient;

mod common;

#[tokio::test]
async fn lifecycle_view_and_archive_restore_flow_are_service_backed() {
    let (service, db_client) = common::make_service_with_client().await;
    let request_time = Utc.with_ymd_and_hms(2026, 1, 10, 9, 0, 0).unwrap();

    let episode_id = IngestCapability::ingest_from_service(
        &service,
        memory_mcp::models::IngestRequest {
            source_type: "meeting".to_string(),
            source_id: "lifecycle-archive-1".to_string(),
            content: "Lifecycle candidate episode".to_string(),
            t_ref: request_time,
            t_ingested: Some(request_time),
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("ingest episode");

    let view = memory_mcp::platform::lifecycle_runtime::handles_from(&service)
        .build_lifecycle_view()
        .await
        .expect("build lifecycle view");
    assert!(view.defaults.archival_age_days > 0);

    let archive = memory_mcp::platform::lifecycle_runtime::handles_from(&service)
        .archive_candidates(std::slice::from_ref(&episode_id), false)
        .await
        .expect("archive candidate");
    assert_eq!(archive.archived_count, 1);

    let archived = db_client
        .select_one(&episode_id, "org")
        .await
        .expect("load archived episode")
        .expect("archived episode exists");
    assert_eq!(archived["status"], "archived");

    let restore = memory_mcp::platform::lifecycle_runtime::handles_from(&service)
        .restore_archived(std::slice::from_ref(&episode_id))
        .await
        .expect("restore archived episode");
    assert_eq!(restore.restored_count, 1);

    let restored = db_client
        .select_one(&episode_id, "org")
        .await
        .expect("load restored episode")
        .expect("restored episode exists");
    assert_eq!(restored["status"], "active");
}
