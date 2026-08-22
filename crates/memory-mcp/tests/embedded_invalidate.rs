mod common;

use chrono::{Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest, Provenance};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::invalidate::InvalidateCapability;
use memory_mcp::storage::DbClient;

#[tokio::test]
async fn embedded_invalidate_removes_fact_from_context() -> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let now = Utc::now();

    let fact_id = service
        .add_fact(
            "metric",
            "ARR is $1M",
            "ARR is $1M",
            "episode:1",
            now - Duration::days(1),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:1"),
        )
        .await?;

    let as_of_before = Utc::now() + Duration::seconds(1);

    let context_before = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "ARR".to_string(),
            as_of: Some(as_of_before),
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;
    assert!(!context_before.is_empty());

    InvalidateCapability::invalidate(
        &service.build_context(),
        InvalidateRequest {
            fact_id,
            reason: "Superseded".to_string(),
            t_invalid: now - Duration::seconds(1),
        },
        None,
    )
    .await?;

    let as_of_after = Utc::now() + Duration::seconds(2);
    let context_after = AssembleContextCapability::assemble_context(
        &service.build_context(),
        AssembleContextRequest {
            query: "ARR".to_string(),
            as_of: Some(as_of_after),
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,
        },
    )
    .await?;
    assert!(context_after.is_empty());

    Ok(())
}

#[tokio::test]
async fn embedded_invalidate_persists_bitemporal_close_and_reason()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = common::make_service_with_client_result().await?;
    let now = Utc::now();
    let fact_id = service
        .add_fact(
            "metric",
            "ARR is $2M",
            "ARR is $2M",
            "episode:close-test",
            now - Duration::days(1),
            0.9,
            vec![],
            vec![],
            Provenance::agent_observation("episode:close-test"),
        )
        .await?;

    InvalidateCapability::invalidate(
        &service.build_context(),
        InvalidateRequest {
            fact_id: fact_id.clone(),
            reason: "Superseded by a newer ARR report".to_string(),
            t_invalid: now - Duration::seconds(1),
        },
        None,
    )
    .await?;

    let stored = db_client
        .select_one(&fact_id, "org")
        .await?
        .expect("invalidated fact should remain stored");
    assert!(
        stored
            .get("t_invalid")
            .is_some_and(|value| !value.is_null())
    );
    assert!(
        stored
            .get("t_invalid_ingested")
            .is_some_and(|value| !value.is_null())
    );
    assert_eq!(
        stored
            .get("invalidation_reason")
            .and_then(|value| value.as_str()),
        Some("Superseded by a newer ARR report")
    );

    Ok(())
}

#[tokio::test]
async fn embedded_relate_invalidates_previous_active_edge_version()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, db_client) = common::make_service_with_client_result().await?;

    let alice = service.resolve_entity("person", "Alice").await?;
    let bob = service.resolve_entity("person", "Bob").await?;

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
