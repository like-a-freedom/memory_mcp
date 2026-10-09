mod common;

use chrono::{DateTime, Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest, Provenance};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability;

async fn service_with_recallable_fact()
-> Result<(MemoryService, DateTime<Utc>, String), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let now = Utc::now();
    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache",
            now - Duration::days(1),
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache"),
        )
        .await?;
    Ok((service, now, fact_id))
}

fn request() -> AssembleContextRequest {
    AssembleContextRequest {
        query: "ARR".to_string(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    }
}

fn historical_request(as_of: DateTime<Utc>) -> AssembleContextRequest {
    AssembleContextRequest {
        as_of: Some(as_of),
        ..request()
    }
}

async fn service_with_future_valid_fact(
    t_valid: DateTime<Utc>,
) -> Result<(MemoryService, String), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache-history",
            t_valid,
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache-history"),
        )
        .await?;
    Ok((service, fact_id))
}

#[tokio::test]
async fn embedded_context_cache_returns_same_results() -> Result<(), Box<dyn std::error::Error>> {
    let (service, _, _) = service_with_recallable_fact().await?;
    let first =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    let second =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;

    assert_eq!(first, second);
    Ok(())
}

#[tokio::test]
async fn invalidation_clears_cached_context_before_the_next_assembly()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, now, fact_id) = service_with_recallable_fact().await?;
    let before =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert_eq!(before.len(), 1);

    InvalidateCapability::invalidate_from_service(
        &service,
        InvalidateRequest {
            fact_id,
            reason: "no longer current".to_string(),
            t_invalid: now - Duration::seconds(1),
        },
        None,
    )
    .await?;

    let after =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert!(after.is_empty());
    Ok(())
}

#[tokio::test]
async fn ingestion_invalidates_a_warmed_empty_current_context()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let empty =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert!(empty.is_empty());

    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache-warmed-empty",
            Utc::now() - Duration::days(1),
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache-warmed-empty"),
        )
        .await?;

    let after_ingest =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert_eq!(after_ingest.len(), 1);
    assert_eq!(after_ingest[0].fact_id, fact_id);
    Ok(())
}

#[tokio::test]
async fn historical_cutoffs_in_one_bucket_do_not_reuse_an_empty_result()
-> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let next_bucket = now.timestamp().div_euclid(300) * 300 + 300;
    let before_valid_time = DateTime::from_timestamp(next_bucket + 30, 0)
        .ok_or("before cutoff is outside chrono range")?;
    let valid_time = DateTime::from_timestamp(next_bucket + 90, 0)
        .ok_or("valid time is outside chrono range")?;
    let after_valid_time = DateTime::from_timestamp(next_bucket + 150, 0)
        .ok_or("after cutoff is outside chrono range")?;
    let (service, fact_id) = service_with_future_valid_fact(valid_time).await?;

    let before = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(before_valid_time),
    )
    .await?;
    assert!(before.is_empty());

    let after = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(after_valid_time),
    )
    .await?;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].fact_id, fact_id);
    Ok(())
}

#[tokio::test]
async fn historical_cutoffs_in_one_bucket_do_not_reuse_a_later_result()
-> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let next_bucket = now.timestamp().div_euclid(300) * 300 + 300;
    let before_valid_time = DateTime::from_timestamp(next_bucket + 30, 0)
        .ok_or("before cutoff is outside chrono range")?;
    let valid_time = DateTime::from_timestamp(next_bucket + 90, 0)
        .ok_or("valid time is outside chrono range")?;
    let after_valid_time = DateTime::from_timestamp(next_bucket + 150, 0)
        .ok_or("after cutoff is outside chrono range")?;
    let (service, fact_id) = service_with_future_valid_fact(valid_time).await?;

    let after = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(after_valid_time),
    )
    .await?;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].fact_id, fact_id);

    let before = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(before_valid_time),
    )
    .await?;
    assert!(before.is_empty());
    Ok(())
}
