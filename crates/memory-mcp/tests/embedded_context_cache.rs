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
