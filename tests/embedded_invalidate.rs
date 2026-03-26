mod embedded_support;

use chrono::{Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest};
use memory_mcp::service::MemoryService;
use memory_mcp::storage::DbClient;
use memory_mcp::storage::SurrealDbClient;

async fn setup_embedded_service_with_client()
-> Result<(MemoryService, std::sync::Arc<SurrealDbClient>), Box<dyn std::error::Error>> {
    let db_client = std::sync::Arc::new(
        SurrealDbClient::connect_in_memory("embedded_test", "org", "warn").await?,
    );
    db_client.apply_migrations("org").await?;

    let service = MemoryService::new(
        db_client.clone(),
        vec![
            "org".to_string(),
            "personal".to_string(),
            "private".to_string(),
        ],
        "warn".to_string(),
        50,
        100,
    )?;

    Ok((service, db_client))
}

#[tokio::test]
async fn embedded_invalidate_removes_fact_from_context() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;
    let now = Utc::now();

    let fact_id = service
        .add_fact(
            "metric",
            "ARR is $1M",
            "ARR is $1M",
            "episode:1",
            now - Duration::days(1),
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:1"}),
        )
        .await?;

    let as_of_before = Utc::now() + Duration::seconds(1);

    let context_before = service
        .assemble_context(AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of_before),
            budget: 5,
            access: None,
        })
        .await?;
    assert!(!context_before.is_empty());

    service
        .invalidate(
            InvalidateRequest {
                fact_id,
                reason: "Superseded".to_string(),
                t_invalid: now - Duration::seconds(1),
            },
            None,
        )
        .await?;

    let as_of_after = Utc::now() + Duration::seconds(2);
    let context_after = service
        .assemble_context(AssembleContextRequest {
            query: "ARR".to_string(),
            scope: "org".to_string(),
            as_of: Some(as_of_after),
            budget: 5,
            access: None,
        })
        .await?;
    assert!(context_after.is_empty());

    Ok(())
}

#[tokio::test]
async fn embedded_relate_invalidates_previous_active_edge_version()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = setup_embedded_service_with_client().await?;

    let alice = service.resolve_person("Alice").await?;
    let bob = service.resolve_person("Bob").await?;

    service.relate(&alice, "knows", &bob).await?;
    tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    service.relate(&alice, "knows", &bob).await?;

    let edges = db_client.select_table("edge", "org").await?;
    let to_record_id = |record_id: &str| {
        let (table, key) = record_id
            .split_once(':')
            .expect("record id should contain table prefix");
        serde_json::json!({"RecordId": {"table": table, "key": key}})
    };
    let knows_edges: Vec<_> = edges
        .into_iter()
        .filter_map(|edge| edge.as_object().cloned())
        .filter(|edge| {
            edge.get("in") == Some(&to_record_id(&alice))
                && edge.get("relation").and_then(|value| value.as_str()) == Some("knows")
                && edge.get("out") == Some(&to_record_id(&bob))
        })
        .collect();

    assert_eq!(knows_edges.len(), 2);
    assert_eq!(
        knows_edges
            .iter()
            .filter(|edge| edge.get("t_invalid").is_some())
            .count(),
        1
    );
    assert_eq!(
        knows_edges
            .iter()
            .filter(|edge| edge.get("t_invalid").is_none())
            .count(),
        1
    );
    assert!(
        knows_edges
            .iter()
            .any(|edge| edge.get("t_invalid_ingested").is_some())
    );

    Ok(())
}
