//! Consumer-owned memory dependencies: capabilities depend on
//! the ports they actually use, not on a shared service
//! container.

use std::sync::Arc;

use memory_mcp::MemoryError;
use memory_mcp::memory::api::{
    EntityResolutionPort, IngestionPort, RateLimitPort, ResolveCommand, resolve_entity,
};

struct IngestRecorder {
    calls: std::sync::Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl IngestionPort for IngestRecorder {
    async fn ingest_episode(&self, source_id: String) -> Result<String, MemoryError> {
        self.calls
            .lock()
            .expect("calls lock")
            .push(source_id.clone());
        Ok(format!("episode:{source_id}"))
    }
}

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

fn candidate(name: &str) -> memory_mcp::models::EntityCandidate {
    memory_mcp::models::EntityCandidate {
        canonical_name: name.to_owned(),
        aliases: Vec::new(),
        entity_type: "person".to_owned(),
    }
}

#[tokio::test]
async fn an_ingest_capability_only_needs_the_ingestion_port() {
    let port = IngestRecorder {
        calls: std::sync::Mutex::new(Vec::new()),
    };

    let episode = memory_mcp::memory::api::ingest_episode(&port, "MSG-1".into())
        .await
        .expect("ingest succeeds");

    assert_eq!(episode, "episode:MSG-1");
    assert_eq!(
        port.calls.lock().expect("calls lock").clone(),
        vec!["MSG-1"]
    );
}

#[tokio::test]
async fn resolve_enforces_the_rate_limit_before_touching_the_resolver() {
    struct DenyAll;

    impl RateLimitPort for DenyAll {
        fn check(&self, _caller: Option<&str>) -> Result<(), MemoryError> {
            Err(MemoryError::Validation("rate limit exceeded".into()))
        }
    }

    let resolver = FixedResolver { created: false };
    let command = ResolveCommand {
        candidate: candidate("Ada Lovelace"),
        caller_id: Some("user-1".into()),
    };

    let error = resolve_entity(&resolver, &DenyAll, &command)
        .await
        .expect_err("a refused caller must not resolve");
    assert!(
        matches!(&error, MemoryError::Validation(message) if message == "rate limit exceeded"),
        "expected the rate-limit refusal, got {error:?}"
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

#[tokio::test]
async fn the_ports_are_object_safe_so_they_can_be_injected() {
    let port: Arc<dyn IngestionPort> = Arc::new(IngestRecorder {
        calls: std::sync::Mutex::new(Vec::new()),
    });
    let episode = memory_mcp::memory::api::ingest_episode(port.as_ref(), "MSG-2".into())
        .await
        .expect("a trait object port is usable");
    assert_eq!(episode, "episode:MSG-2");
}
