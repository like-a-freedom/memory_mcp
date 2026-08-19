mod embedded_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, TimeZone, Utc};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;
use memory_mcp::service::{MemoryError, MemoryService, normalize_dt};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

static NEXT_EMBEDDED_DB: AtomicUsize = AtomicUsize::new(1);

async fn setup_embedded_service_with_client()
-> Result<(MemoryService, Arc<SurrealDbClient>), Box<dyn std::error::Error>> {
    let db_name = format!(
        "ingest_select_{}",
        NEXT_EMBEDDED_DB.fetch_add(1, Ordering::Relaxed)
    );
    let db_client = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &["org".to_string()], "warn")
            .await?,
    );
    db_client.apply_migrations("org").await?;

    let service = MemoryService::new(
        db_client.clone(),
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )?;
    Ok((service, db_client))
}

async fn seed_legacy_episode(
    db_client: &Arc<SurrealDbClient>,
    episode_id: &str,
    source_type: &str,
    source_id: &str,
    content: &str,
    t_ref: DateTime<Utc>,
) -> Result<(), Box<dyn std::error::Error>> {
    let normalized_t_ref = normalize_dt(t_ref);
    db_client
        .create(
            episode_id,
            json!({
                "episode_id": episode_id,
                "source_type": source_type,
                "source_id": source_id,
                "content": content,
                "t_ref": normalized_t_ref,
                "t_ingested": normalized_t_ref,
                "policy_tags": [],
            }),
            "org",
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn ingest_then_extract_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let svc = embedded_support::setup_embedded_service().await?;

    let req = IngestRequest {
        source_type: "meeting".to_string(),
        source_id: "test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Budget $100k".to_string(),
        t_ref: Utc::now(),
        t_ingested: None,
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest(&svc.build_context(), req.clone(), None).await?;
    let episode_id_2 = IngestCapability::ingest(&svc.build_context(), req, None).await?;
    assert_eq!(episode_id, episode_id_2);

    let payload = ExtractCapability::extract(&svc.build_context(), &episode_id, None, None).await?;
    assert_eq!(payload.episode_id, episode_id);
    assert!(!payload.entities.is_empty());
    assert!(!payload.facts.is_empty());

    let count = svc.episode_count().await?;
    assert!(count >= 1, "expected at least one episode in DB");

    Ok(())
}

#[tokio::test]
async fn ingest_reuses_one_legacy_episode_by_source_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = setup_embedded_service_with_client().await?;
    let t_ref = Utc.with_ymd_and_hms(2026, 8, 19, 10, 0, 0).unwrap();
    seed_legacy_episode(
        &db_client,
        "episode:legacy-reused",
        "inline",
        "legacy-source",
        "legacy content",
        t_ref,
    )
    .await?;

    let episode_id = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "inline".into(),
            source_id: "legacy-source".into(),
            content: "legacy content".into(),
            t_ref,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await?;

    assert_eq!(episode_id, "episode:legacy-reused");
    assert_eq!(service.episode_count().await?, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_rejects_ambiguous_legacy_episode_identity_without_writing()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = setup_embedded_service_with_client().await?;
    let t_ref = Utc.with_ymd_and_hms(2026, 8, 19, 11, 0, 0).unwrap();
    for (episode_id, content) in [
        ("episode:legacy-a", "legacy content A"),
        ("episode:legacy-b", "legacy content B"),
    ] {
        seed_legacy_episode(
            &db_client,
            episode_id,
            "inline",
            "ambiguous-source",
            content,
            t_ref,
        )
        .await?;
    }

    let result = IngestCapability::ingest(
        &service.build_context(),
        IngestRequest {
            source_type: "inline".into(),
            source_id: "ambiguous-source".into(),
            content: "ambiguous content".into(),
            t_ref,
            t_ingested: None,
            policy_tags: vec![],
        },
        None,
    )
    .await;

    assert!(matches!(
        result,
        Err(MemoryError::Conflict(message))
            if message.contains("ambiguous legacy episode identity")
    ));
    assert_eq!(service.episode_count().await?, 2);
    Ok(())
}
