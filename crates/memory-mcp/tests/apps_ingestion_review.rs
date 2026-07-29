use memory_mcp::service::{
    CommitIngestionReviewRequest, PrepareIngestionReviewRequest, fact_from_record,
};
use memory_mcp::storage::DbClient;

mod common;

#[tokio::test]
async fn prepare_ingestion_review_uses_episode_backed_drafts() {
    let (service, _db_client) = common::make_service_with_client().await;
    let episode_id = common::ingest_episode(
        &service,
        "draft-episode",
        "Alice promised a launch on Friday.",
    )
    .await;

    let bundle = service
        .prepare_ingestion_review(PrepareIngestionReviewRequest {
            scope: "personal".to_string(),
            source_text: None,
            draft_episode_id: Some(episode_id.clone()),
        })
        .await
        .expect("prepare ingestion review");

    assert_eq!(
        bundle.source.draft_episode_id.as_deref(),
        Some(episode_id.as_str())
    );
    assert_eq!(bundle.items.len(), 1);
    assert_eq!(bundle.items[0].source_episode, episode_id);
    assert_eq!(bundle.items[0].status, "pending");
    assert_eq!(bundle.summary.pending, 1);
    assert_eq!(bundle.summary.committable, 0);
}

#[tokio::test]
async fn commit_ingestion_review_persists_approved_items_as_facts() {
    let (service, db_client) = common::make_service_with_client().await;
    let bundle = service
        .prepare_ingestion_review(PrepareIngestionReviewRequest {
            scope: "org".to_string(),
            source_text: Some("Beta launch is scheduled for Friday.".to_string()),
            draft_episode_id: None,
        })
        .await
        .expect("prepare ingestion review");

    let mut approved = bundle.items.clone();
    approved[0].status = "approved".to_string();

    let outcome = service
        .commit_ingestion_review(CommitIngestionReviewRequest {
            scope: "org".to_string(),
            items: approved,
        })
        .await
        .expect("commit ingestion review");

    assert_eq!(outcome.committed_count, 1);
    let stored = db_client
        .select_one(&outcome.fact_ids[0], "org")
        .await
        .expect("load committed fact")
        .expect("stored fact exists");
    let fact = fact_from_record(&stored).expect("fact parses");
    assert_eq!(fact.content, "Beta launch is scheduled for Friday.");
    assert!(fact.source_episode.starts_with("episode:"));
}
