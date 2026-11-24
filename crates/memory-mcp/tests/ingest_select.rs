mod common;

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use memory_mcp::knowledge::api::owned_episode_scan;
use memory_mcp::models::IngestRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::service::{MemoryError, normalize_dt};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

/// How many episodes the service can see.
///
/// `MemoryService::episode_count` used to provide this. No production
/// caller reached it, and the owner-named read is the seam this test
/// actually wants: the same rows a caller would enumerate.
async fn episode_scan(
    service: &memory_mcp::MemoryService,
) -> Result<Vec<serde_json::Value>, MemoryError> {
    let port = memory_mcp::knowledge::infra::KnowledgeReadAdapter::new(
        service.db_client_for_port(),
        service.namespace_for_port(),
    );
    owned_episode_scan(&port).await
}

async fn episode_count(service: &memory_mcp::MemoryService) -> Result<i32, MemoryError> {
    Ok(episode_scan(service).await?.len() as i32)
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
            memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
        )
        .await?;
    Ok(())
}

#[tokio::test]
async fn ingest_then_extract_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let svc = common::make_service().await;

    let req = IngestRequest {
        source_context: None,
        source_type: "meeting".to_string(),
        source_id: "test-1".to_string(),
        content: "Meeting with Alice Inc and Bob Corp. Budget $100k".to_string(),
        t_ref: Utc.with_ymd_and_hms(2026, 10, 3, 9, 30, 0).unwrap(),
        t_ingested: Some(Utc.with_ymd_and_hms(2026, 10, 3, 9, 31, 0).unwrap()),
        policy_tags: vec![],
    };

    let episode_id = IngestCapability::ingest_from_service(&svc, req.clone(), None).await?;
    let episode_id_2 = IngestCapability::ingest_from_service(&svc, req, None).await?;
    assert_eq!(episode_id, episode_id_2);

    let payload = ExtractCapability::extract_from_service(&svc, &episode_id, None, None).await?;
    assert_eq!(payload.episode_id, episode_id);
    assert!(!payload.entities.is_empty());
    assert!(!payload.facts.is_empty());

    let count = episode_count(&svc).await?;
    assert!(count >= 1, "expected at least one episode in DB");

    Ok(())
}

/// Ingestion preserves source content/valid time and owns transaction time.
#[tokio::test]
async fn ingest_persists_source_fields_with_server_transaction_time()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let t_ref = Utc.with_ymd_and_hms(2026, 10, 3, 10, 0, 0).unwrap();
    let t_ingested = Utc.with_ymd_and_hms(2026, 10, 3, 10, 1, 0).unwrap();
    let request = || IngestRequest {
        source_context: None,
        source_type: "inline".to_owned(),
        source_id: "source-a01".to_owned(),
        content: "exact original episode content".to_owned(),
        t_ref,
        t_ingested: Some(t_ingested),
        policy_tags: vec!["reviewed".to_owned()],
    };

    let before = Utc::now();
    let episode_id = IngestCapability::ingest_from_service(&service, request(), None).await?;
    let after = Utc::now();
    let before_retry = episode_scan(&service).await?;
    assert_eq!(before_retry.len(), 1, "one episode was persisted");
    assert_eq!(before_retry[0]["episode_id"], episode_id);
    assert_eq!(before_retry[0]["source_type"], "inline");
    assert_eq!(before_retry[0]["source_id"], "source-a01");
    assert_eq!(before_retry[0]["content"], "exact original episode content");
    assert_eq!(before_retry[0]["t_ref"], "2026-10-03T10:00:00Z");
    let recorded = chrono::DateTime::parse_from_rfc3339(
        before_retry[0]["t_ingested"]
            .as_str()
            .expect("stored transaction timestamp"),
    )?
    .with_timezone(&Utc);
    assert!(
        recorded >= before && recorded <= after,
        "transaction time must come from the server, not the supplied timestamp"
    );
    assert_eq!(before_retry[0]["policy_tags"], json!(["reviewed"]));
    Ok(())
}

/// Agent-provided content is data, even when it names an existing local file.
#[tokio::test]
async fn ingest_treats_existing_file_path_as_literal_content()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let source = tempfile::tempdir()?;
    let file_path = source.path().join("private-notes.txt");
    std::fs::write(&file_path, "unrequested private file contents")?;
    let literal_path = file_path.to_string_lossy().into_owned();

    let response = memory_mcp::tools::ingest(
        &service,
        memory_mcp::tools::params::IngestParams {
            source_type: "agent_note".into(),
            source_id: "caller-selected-source".into(),
            content: literal_path.clone(),
            t_ref: Utc
                .with_ymd_and_hms(2026, 11, 22, 12, 0, 0)
                .unwrap()
                .to_rfc3339(),
            t_ingested: None,
            policy_tags: vec![],
            source_group: None,
            source_position: None,
        },
    )
    .await?;
    let episodes = episode_scan(&service).await?;
    let episode = episodes
        .iter()
        .find(|episode| episode["episode_id"] == response.result)
        .expect("ingested episode is visible through the owner read");

    assert_eq!(episode["content"], literal_path);
    assert_eq!(episode["source_id"], "caller-selected-source");
    Ok(())
}

/// Agent-provided URL text is data and must not trigger an HTTP import.
#[tokio::test]
async fn ingest_treats_loopback_url_as_literal_content() -> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let literal_url = "http://127.0.0.1:9/agent-reference";

    let response = memory_mcp::tools::ingest(
        &service,
        memory_mcp::tools::params::IngestParams {
            source_type: "agent_note".into(),
            source_id: "caller-selected-url-source".into(),
            content: literal_url.into(),
            t_ref: Utc
                .with_ymd_and_hms(2026, 11, 22, 12, 1, 0)
                .unwrap()
                .to_rfc3339(),
            t_ingested: None,
            policy_tags: vec![],
            source_group: None,
            source_position: None,
        },
    )
    .await?;
    let episodes = episode_scan(&service).await?;
    let episode = episodes
        .iter()
        .find(|episode| episode["episode_id"] == response.result)
        .expect("ingested episode is visible through the owner read");

    assert_eq!(episode["content"], literal_url);
    assert_eq!(episode["source_id"], "caller-selected-url-source");
    Ok(())
}

/// High-confidence credentials are refused before an episode is committed,
/// and the validation error does not echo the supplied value.
#[tokio::test]
async fn ingest_rejects_credential_material_without_persisting_or_echoing_it()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let secret = "sensitive-canary-credential-value";
    let error = memory_mcp::tools::ingest(
        &service,
        memory_mcp::tools::params::IngestParams {
            source_type: "note".into(),
            source_id: "credential-test".into(),
            content: format!("API_KEY={secret}"),
            t_ref: "2026-11-22T12:02:00Z".into(),
            t_ingested: None,
            policy_tags: vec![],
            source_group: None,
            source_position: None,
        },
    )
    .await
    .expect_err("credential-bearing episode must be rejected");

    assert!(
        !error.to_string().contains(secret),
        "validation errors must not reflect credential material"
    );
    assert_eq!(episode_count(&service).await?, 0);
    Ok(())
}

/// Ordinary discussion of credential concepts remains ingestible.
#[tokio::test]
async fn ingest_accepts_non_secret_security_discussion() -> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let response = memory_mcp::tools::ingest(
        &service,
        memory_mcp::tools::params::IngestParams {
            source_type: "note".into(),
            source_id: "security-discussion".into(),
            content: "OAuth access tokens expire after one hour; rotate API keys regularly.".into(),
            t_ref: "2026-11-22T12:03:00Z".into(),
            t_ingested: None,
            policy_tags: vec![],
            source_group: None,
            source_position: None,
        },
    )
    .await?;

    assert_eq!(episode_count(&service).await?, 1);
    assert!(!response.result.is_empty());
    Ok(())
}

#[tokio::test]
async fn ingest_rejects_credential_material_in_source_metadata()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let error = memory_mcp::tools::ingest(
        &service,
        memory_mcp::tools::params::IngestParams {
            source_type: "note".into(),
            source_id: "api_key=sensitive-metadata-canary".into(),
            content: "ordinary meeting notes".into(),
            t_ref: "2026-11-22T12:04:00Z".into(),
            t_ingested: None,
            policy_tags: vec![],
            source_group: None,
            source_position: None,
        },
    )
    .await
    .expect_err("credential-bearing source identity must be rejected");

    assert!(!error.to_string().contains("sensitive-metadata-canary"));
    assert_eq!(episode_count(&service).await?, 0);
    Ok(())
}

/// Integration scenario: an idempotent retry preserves the existing episode.
#[tokio::test]
async fn duplicate_ingestion_preserves_the_existing_episode()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, _) = common::make_service_with_client_result().await?;
    let request = || IngestRequest {
        source_context: None,
        source_type: "inline".to_owned(),
        source_id: "source-a01".to_owned(),
        content: "exact original episode content".to_owned(),
        t_ref: Utc.with_ymd_and_hms(2026, 10, 3, 10, 0, 0).unwrap(),
        t_ingested: Some(Utc.with_ymd_and_hms(2026, 10, 3, 10, 1, 0).unwrap()),
        policy_tags: vec!["reviewed".to_owned()],
    };
    let episode_id = IngestCapability::ingest_from_service(&service, request(), None).await?;
    let before_retry = episode_scan(&service).await?;
    let duplicate_id = IngestCapability::ingest_from_service(&service, request(), None).await?;
    let after_retry = episode_scan(&service).await?;

    assert_eq!(
        duplicate_id, episode_id,
        "the duplicate reuses its stable ID"
    );
    assert_eq!(
        after_retry, before_retry,
        "duplicate ingestion must not replace or add to the stored episode"
    );
    Ok(())
}

#[tokio::test]
async fn ingest_reuses_one_legacy_episode_by_source_identity()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = common::make_service_with_client_result().await?;
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

    let episode_id = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
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
    assert_eq!(episode_count(&service).await?, 1);
    Ok(())
}

#[tokio::test]
async fn ingest_rejects_ambiguous_legacy_episode_identity_without_writing()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = common::make_service_with_client_result().await?;
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

    let result = IngestCapability::ingest_from_service(
        &service,
        IngestRequest {
            source_context: None,
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
                && !message.contains("ambiguous-source")
    ));
    assert_eq!(episode_count(&service).await?, 2);
    Ok(())
}
