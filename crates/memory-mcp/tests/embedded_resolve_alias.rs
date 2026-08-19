mod embedded_support;

use memory_mcp::models::EntityCandidate;
use memory_mcp::service::capabilities::resolve::ResolveCapability;

#[tokio::test]
async fn embedded_resolve_idempotent_for_canonical_name() -> Result<(), Box<dyn std::error::Error>>
{
    let service = embedded_support::setup_embedded_service().await?;

    let canonical_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Dmitry Ivanov".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    let second_id = ResolveCapability::resolve(
        &service.build_context(),
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

    let canonical_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Dmitry Ivanov".to_string(),
            aliases: vec!["Dima Ivanov".to_string()],
        },
        None,
    )
    .await?;

    let alias_id = ResolveCapability::resolve(
        &service.build_context(),
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
    let entity_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Alice Smith".to_string(),
            aliases: vec!["Alice S.".to_string(), "AS".to_string()],
        },
        None,
    )
    .await?;

    // Resolve by alias should return the same entity ID
    let resolved_by_alias = ResolveCapability::resolve(
        &service.build_context(),
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

#[tokio::test]
async fn embedded_resolve_creates_new_entity_when_not_found()
-> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    let entity_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Ivan Petrov".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    assert!(!entity_id.is_empty(), "new entity should have an ID");
    Ok(())
}

#[tokio::test]
async fn embedded_resolve_fuzzy_matches_non_identical_cyrillic_name_and_persists_alias()
-> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    let canonical_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Иван Петров".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    // "Петрёв" is not equal to the canonical "Петров" after normalization,
    // but the one-character difference is above the default fuzzy threshold.
    let fuzzy_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Иван Петрёв".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    assert_eq!(
        canonical_id, fuzzy_id,
        "above-threshold fuzzy match should merge"
    );

    let persisted_alias_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "ИВАН ПЕТРЁВ".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    assert_eq!(
        canonical_id, persisted_alias_id,
        "fuzzy candidate should be persisted as an alias"
    );
    Ok(())
}

#[tokio::test]
async fn embedded_resolve_below_threshold_creates_new_entity()
-> Result<(), Box<dyn std::error::Error>> {
    let service = embedded_support::setup_embedded_service().await?;

    let existing_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Bob Smith".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    let new_id = ResolveCapability::resolve(
        &service.build_context(),
        EntityCandidate {
            entity_type: "person".to_string(),
            canonical_name: "Bob Jones".to_string(),
            aliases: vec![],
        },
        None,
    )
    .await?;

    assert_ne!(
        existing_id, new_id,
        "below-threshold candidate should create a separate entity"
    );
    Ok(())
}
