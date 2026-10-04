//! Consumer-owned memory dependencies: capabilities depend on
//! the ports they actually use, not on a shared service
//! container.

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::knowledge::KnowledgeGraphStore;
use memory_mcp::memory::api::{
    EntityResolutionPort, RateLimitPort, ResolveCommand, resolve_entity,
};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability;
use memory_mcp::shared::ids::deterministic_entity_id;
use memory_mcp::storage::{DbClient, SurrealDbClient};

struct AllowAllRateLimit;

impl RateLimitPort for AllowAllRateLimit {
    fn check(&self, caller: Option<&str>) -> Result<(), MemoryError> {
        let _ = caller;
        Ok(())
    }
}

struct FixedResolver {
    created: bool,
}

#[async_trait::async_trait]
impl EntityResolutionPort for FixedResolver {
    async fn resolve_or_create(
        &self,
        candidate: memory_mcp::models::EntityCandidate,
    ) -> Result<(String, bool), MemoryError> {
        Ok((
            format!("entity:{}", candidate.canonical_name.to_lowercase()),
            self.created,
        ))
    }
}

/// Bridges the port use case to the real resolver and embedded persistence.
///
/// The production adapter is intentionally private; this uses the public
/// service composition edge rather than widening visibility for the test.
struct PersistentResolver<'a> {
    service: &'a MemoryService,
}

#[async_trait::async_trait]
impl EntityResolutionPort for PersistentResolver<'_> {
    async fn resolve_or_create(
        &self,
        candidate: memory_mcp::models::EntityCandidate,
    ) -> Result<(String, bool), MemoryError> {
        let candidate_id =
            deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
        let graph = KnowledgeGraphStore::new(
            self.service.db_client_for_port(),
            self.service.namespace_for_port(),
        );
        let existed_before = graph.select_entity(&candidate_id).await?.is_some();
        let entity_id =
            ResolveCapability::resolve_from_service(self.service, candidate, None).await?;

        Ok((
            entity_id.clone(),
            !existed_before && entity_id == candidate_id,
        ))
    }
}

fn candidate(name: &str) -> memory_mcp::models::EntityCandidate {
    memory_mcp::models::EntityCandidate {
        canonical_name: name.to_owned(),
        aliases: Vec::new(),
        entity_type: "person".to_owned(),
    }
}

/// Integration scenario: real resolution must not persist a refused candidate.
#[tokio::test]
async fn resolve_rate_limit_refusal_does_not_persist_an_entity() {
    struct DenyAll;

    impl RateLimitPort for DenyAll {
        fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
            Err(MemoryError::Validation("rate limit exceeded".into()))
        }
    }

    let db = Arc::new(
        SurrealDbClient::connect_in_memory("memory_consumer_ports_resolve_refusal", "org", "warn")
            .await
            .expect("connect in-memory database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply in-memory migrations");
    let service = MemoryService::new(db.clone(), "org".into(), "warn".into(), 50, 100)
        .expect("build memory service");
    let resolver = PersistentResolver { service: &service };
    let candidate = candidate("Ada Lovelace");
    let command = ResolveCommand {
        candidate: candidate.clone(),
        caller_id: Some("user-1".into()),
    };

    let error = resolve_entity(&resolver, &DenyAll, &command)
        .await
        .expect_err("a refused caller must not resolve");
    assert!(
        matches!(&error, MemoryError::Validation(message) if message == "rate limit exceeded"),
        "expected the rate-limit refusal, got {error:?}"
    );

    let graph = KnowledgeGraphStore::new(db, "org");
    let entity_id = deterministic_entity_id(&candidate.entity_type, &candidate.canonical_name);
    assert!(
        graph
            .select_entity(&entity_id)
            .await
            .expect("read entity through knowledge owner")
            .is_none(),
        "a refused resolution must not persist the candidate entity"
    );
}

#[tokio::test]
async fn resolve_returns_the_canonical_id_and_creation_flag() {
    let resolver = FixedResolver { created: true };
    let command = ResolveCommand {
        candidate: candidate("Ada Lovelace"),
        caller_id: None,
    };

    let (entity_id, created) = resolve_entity(&resolver, &AllowAllRateLimit, &command)
        .await
        .expect("resolve succeeds");

    assert_eq!(entity_id, "entity:ada lovelace");
    assert!(created, "the port reports that it created the entity");
}
