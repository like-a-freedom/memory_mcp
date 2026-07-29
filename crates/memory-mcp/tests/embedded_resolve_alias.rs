mod embedded_support;

use memory_mcp::models::EntityCandidate;

#[tokio::test]
async fn embedded_resolve_idempotent_for_canonical_name() -> Result<(), Box<dyn std::error::Error>>
{
    let service = embedded_support::setup_embedded_service().await?;

    let canonical_id = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dmitry Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await?;

    let second_id = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dmitry Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await?;

    assert_eq!(canonical_id, second_id);
    Ok(())
}

#[tokio::test]
async fn embedded_resolve_matches_existing_alias() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    let canonical_id = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dmitry Ivanov".to_string(),
                aliases: vec!["Dima Ivanov".to_string()],
            },
            None,
        )
        .await?;

    let alias_id = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Dima Ivanov".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await?;

    assert_eq!(canonical_id, alias_id);
    Ok(())
}

#[tokio::test]
async fn embedded_batch_lookup_finds_entity_by_alias() -> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    // Create entity with alias
    let entity_id = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Alice Smith".to_string(),
                aliases: vec!["Alice S.".to_string(), "AS".to_string()],
            },
            None,
        )
        .await?;

    // Resolve by alias should return the same entity ID
    let resolved_by_alias = service
        .resolve(
            EntityCandidate {
                entity_type: "person".to_string(),
                canonical_name: "Alice S.".to_string(),
                aliases: vec![],
            },
            None,
        )
        .await?;

    assert_eq!(
        entity_id, resolved_by_alias,
        "resolve by alias should return same entity ID"
    );

    Ok(())
}
