use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::{DateTime, Utc};
use memory_mcp::models::{EntityCandidate, IngestRequest, Provenance};
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_resolve::ResolveCapability;
use memory_mcp::service::{MemoryService, normalize_dt, normalize_text};
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::json;

pub mod http_server;

static TEST_DB_COUNTER: AtomicUsize = AtomicUsize::new(1);

/// The single namespace used by ordinary in-memory test fixtures.
///
/// A few tests still pass legacy scope labels to seed records so they can prove
/// that old metadata is readable. Those labels must never select storage.
const TEST_ACTIVE_NAMESPACE: &str = "org";

fn next_test_db_name() -> String {
    let seq = TEST_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("memory_test_{seq}")
}

pub struct TestMemory {
    pub service: MemoryService,
    pub db_client: Arc<SurrealDbClient>,
}

impl TestMemory {
    pub async fn new(query_logging_enabled: bool) -> Self {
        let namespaces = vec!["org".to_string()];
        let db_name = next_test_db_name();
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(&db_name, &namespaces, "warn")
                .await
                .expect("connect in memory service"),
        );
        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply in-memory migrations");
        }

        let service = MemoryService::new(
            db_client.clone(),
            TEST_ACTIVE_NAMESPACE.to_string(),
            "warn".to_string(),
            50,
            100,
        )
        .expect("service init")
        .with_query_logging_enabled(query_logging_enabled);

        Self { service, db_client }
    }
}

#[allow(dead_code)]
pub async fn make_service() -> MemoryService {
    make_service_with_client_and_query_logging(false).await.0
}

/// Resolves an entity by its type and canonical name.
///
/// `MemoryService::resolve_entity` used to provide this. No production
/// caller reached it — `memory/capabilities/resolve.rs` reaches
/// `memory::api::resolve_entity` instead — so the container's copy was a
/// second name for a capability the crate already exposes.
#[allow(dead_code)]
pub async fn resolve_entity(
    service: &MemoryService,
    entity_type: &str,
    name: &str,
) -> Result<String, memory_mcp::MemoryError> {
    ResolveCapability::resolve_from_service(
        service,
        EntityCandidate {
            entity_type: entity_type.to_string(),
            canonical_name: name.to_string(),
            aliases: Vec::new(),
        },
        None,
    )
    .await
}

#[allow(dead_code)]
pub async fn make_service_with_client() -> (MemoryService, Arc<SurrealDbClient>) {
    make_service_with_client_and_query_logging(false).await
}

#[allow(dead_code)]
pub async fn make_service_with_client_and_query_logging(
    query_logging_enabled: bool,
) -> (MemoryService, Arc<SurrealDbClient>) {
    let memory = TestMemory::new(query_logging_enabled).await;
    (memory.service, memory.db_client)
}

/// Same as `make_service_with_client`, but returns `Result` so integration
/// tests whose signatures already return `Box<dyn Error>` can `?` through it.
#[allow(dead_code)]
pub async fn make_service_with_client_result()
-> Result<(MemoryService, Arc<SurrealDbClient>), Box<dyn std::error::Error>> {
    Ok(make_service_with_client().await)
}

#[allow(dead_code)]
pub async fn ingest_episode(service: &MemoryService, source_id: &str, content: &str) -> String {
    let request = IngestRequest {
        source_type: "chat".to_string(),
        source_id: source_id.to_string(),
        content: content.to_string(),
        t_ref: "2026-03-01T10:00:00Z"
            .parse::<DateTime<Utc>>()
            .expect("static timestamp should parse"),
        t_ingested: None,
        policy_tags: vec![],
    };
    let episode_id = IngestCapability::ingest_from_service(service, request, None)
        .await
        .expect("ingest should succeed");
    ExtractCapability::extract_from_service(service, &episode_id, None, None)
        .await
        .expect("extract should succeed");
    episode_id
}

#[allow(dead_code)]
pub async fn seed_fact_at(
    service: &MemoryService,
    scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
) -> String {
    seed_fact_with_links(service, scope, content, t_valid, Vec::new()).await
}

#[allow(dead_code)]
pub async fn seed_fact_with_links(
    service: &MemoryService,
    _legacy_scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    entity_links: Vec<String>,
) -> String {
    service
        .add_fact(
            "note",
            content,
            content,
            "episode:seed",
            t_valid,
            0.9,
            entity_links,
            vec![],
            Provenance::agent_observation("episode:seed"),
        )
        .await
        .expect("seed fact should succeed")
}

#[allow(dead_code)]
pub async fn seed_episode_backed_fact_with_source_id(
    service: &MemoryService,
    _legacy_scope: &str,
    content: &str,
    t_valid: DateTime<Utc>,
    source_id: &str,
) -> String {
    let episode_id = IngestCapability::ingest_from_service(
        service,
        IngestRequest {
            source_type: "seed".to_string(),
            source_id: source_id.to_string(),
            content: content.to_string(),
            t_ref: t_valid,
            t_ingested: Some(t_valid),
            policy_tags: vec![],
        },
        None,
    )
    .await
    .expect("seed episode should succeed");

    let extracted = ExtractCapability::extract_from_service(service, &episode_id, None, None)
        .await
        .expect("seed extraction should succeed");
    let entity_links = extracted
        .entities
        .into_iter()
        .map(|entity| entity.entity_id)
        .collect::<Vec<_>>();

    service
        .add_fact(
            "note",
            content,
            content,
            &episode_id,
            t_valid,
            0.9,
            entity_links,
            vec![],
            Provenance::extraction(&episode_id, "seed", source_id, "manual"),
        )
        .await
        .expect("seed note fact should succeed")
}

#[allow(dead_code)]
pub async fn seed_entity(
    db_client: &Arc<SurrealDbClient>,
    _legacy_scope: &str,
    entity_id: &str,
    entity_type: &str,
    canonical_name: &str,
    aliases: &[String],
) {
    db_client
        .create(
            entity_id,
            json!({
                "entity_id": entity_id,
                "entity_type": entity_type,
                "canonical_name": canonical_name,
                "canonical_name_normalized": normalize_text(canonical_name),
                "aliases": aliases,
            }),
            TEST_ACTIVE_NAMESPACE,
            memory_mcp::knowledge::queries::ENTITY_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed entity should succeed");
}

#[allow(dead_code)]
pub async fn seed_community(
    db_client: &Arc<SurrealDbClient>,
    _legacy_scope: &str,
    community_id: &str,
    member_entities: &[String],
    summary: &str,
    updated_at: DateTime<Utc>,
) {
    db_client
        .create(
            community_id,
            json!({
                "community_id": community_id,
                "member_entities": member_entities,
                "summary": summary,
                "updated_at": normalize_dt(updated_at),
            }),
            TEST_ACTIVE_NAMESPACE,
            memory_mcp::knowledge::queries::COMMUNITY_TEMPORAL_FIELDS,
        )
        .await
        .expect("seed community should succeed");
}

/// The claim rollout stage at which the read path discloses relations.
///
/// Relations are persisted at any stage except `disabled`, but
/// `assemble_context` serves them only at `evidence` — see
/// `knowledge::api::SurrealRelationReader` and
/// `docs/evals/CLAIM_RECONCILIATION.md`. Tests asserting disclosure set this;
/// tests that only assert relations were *persisted* do not need it.
#[allow(dead_code)]
pub const CLAIM_STAGE_EVIDENCE: &str = "evidence";

/// Put a service on the claim stage that discloses relations.
///
/// Returns `Result` so a test can `.expect("evidence is a valid stage")`
/// rather than unwrap, matching the shape of the capability calls beside it.
#[allow(dead_code)]
pub fn exposing_claims(service: MemoryService) -> Result<MemoryService, memory_mcp::MemoryError> {
    service.with_claim_rollout_stage(CLAIM_STAGE_EVIDENCE)
}

/// Persist an episode with an explicit `source_lineage`, then run extract.
///
/// `ingest` derives lineage from `source_id`, and ADR-0008's source gate
/// refuses automatic supersession unless both sides share a lineage while
/// coming from different facts. Two episodes with distinct source ids would
/// therefore never reconcile, so lineage is the thing this pins — not content.
#[allow(dead_code)]
pub async fn ingest_lineage_episode(
    service: &MemoryService,
    db_client: &SurrealDbClient,
    episode_id: &str,
    source_id: &str,
    lineage: &str,
    content: &str,
    t_ref: DateTime<Utc>,
) {
    let iso = t_ref.to_rfc3339();
    db_client
        .create(
            episode_id,
            json!({
                "episode_id": episode_id,
                "source_type": "document",
                "source_id": source_id,
                "content": content,
                "t_ref": iso,
                "t_ingested": iso,
                "policy_tags": [],
                "source_lineage": lineage,
            }),
            TEST_ACTIVE_NAMESPACE,
            memory_mcp::memory::queries::EPISODE_TEMPORAL_FIELDS,
        )
        .await
        .expect("create episode with lineage");

    ExtractCapability::extract_from_service(service, episode_id, None, None)
        .await
        .expect("extract episode with lineage");
}

/// Count active supersession rows. Read-only, so a test can assert on the
/// pipeline's own decision rather than re-deriving it from content.
#[allow(dead_code)]
pub async fn supersession_count(db_client: &SurrealDbClient) -> usize {
    db_client
        .query(
            "SELECT count() AS cnt FROM claim_relation WHERE outcome = 'supersession' AND (t_invalid_ingested IS NONE OR t_invalid_ingested IS NULL)",
            None,
            TEST_ACTIVE_NAMESPACE,
        )
        .await
        .map(|v| serde_json::from_value::<Vec<serde_json::Value>>(v).unwrap_or_default())
        .map(|rows| {
            rows.first()
                .and_then(|r| r.get("cnt").and_then(|c| c.as_i64()))
                .unwrap_or(0) as usize
        })
        .unwrap_or(0)
}

/// Poll until at least `want` active supersessions exist.
///
/// Claim projection is fire-and-forget off `add_fact`, so the rows land after
/// `extract` returns. Bounded at two hundred attempts with a yield between
/// them: a real deadline, not a fixed sleep.
#[allow(dead_code)]
pub async fn wait_for_supersessions(db_client: &SurrealDbClient, want: usize) -> bool {
    for _ in 0..200 {
        if supersession_count(db_client).await >= want {
            return true;
        }
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    false
}
