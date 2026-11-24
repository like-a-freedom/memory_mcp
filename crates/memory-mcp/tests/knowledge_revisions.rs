use std::sync::Arc;

use memory_mcp::models::Provenance;
use memory_mcp::service::MemoryService;
use memory_mcp::storage::{DbClient, EpisodeStoreClient, SurrealDbClient};
use memory_mcp::tools::context::ToolContext;
use serde_json::{Value, json};
use surrealdb::engine::local::SurrealKv;

#[cfg(feature = "eval-support")]
#[derive(Default)]
struct RevisionTimingCollector(std::sync::Mutex<Vec<(u64, u64)>>);

#[cfg(feature = "eval-support")]
impl memory_mcp::eval_support::QueryObserver for RevisionTimingCollector {
    fn observe(&self, _observation: &memory_mcp::eval_support::SqlObservation) {}

    fn observe_revision_transaction(&self, lock_wait_micros: u64, transaction_micros: u64) {
        self.0
            .lock()
            .expect("revision timing collector lock")
            .push((lock_wait_micros, transaction_micros));
    }
}

async fn database() -> Arc<SurrealDbClient> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock after epoch")
        .as_nanos();
    let name = format!("knowledge_revisions_{nonce}");
    let database = Arc::new(
        SurrealDbClient::connect_in_memory(&name, "org", "warn")
            .await
            .expect("open isolated SurrealDB"),
    );
    database
        .apply_migrations("org")
        .await
        .expect("apply base migrations");
    database
}

async fn durable_database(path: &std::path::Path) -> Arc<SurrealDbClient> {
    let raw = surrealdb::Surreal::new::<SurrealKv>(path.to_string_lossy().to_string())
        .await
        .expect("open isolated SurrealKV database");
    raw.use_ns("knowledge_revisions")
        .use_db("knowledge_revisions")
        .await
        .expect("bind namespace and database");
    let database = Arc::new(SurrealDbClient::from_prebound(
        raw,
        "knowledge_revisions",
        "warn",
    ));
    database
        .apply_migrations("knowledge_revisions")
        .await
        .expect("apply base migrations");
    database
}

fn episode(id: &str) -> Value {
    json!({
        "episode_id": format!("episode:{id}"),
        "source_type": "conversation",
        "source_id": format!("source:{id}"),
        "content": format!("A verified event for {id}"),
        "t_ref": "2026-05-01T10:00:00Z",
        "t_ingested": "2026-11-21T10:00:00Z",
        "scope": "org",
        "visibility_scope": "org",
        "policy_tags": []
    })
}

async fn revisions(database: &Arc<SurrealDbClient>) -> Vec<i64> {
    revisions_in(database, "org").await
}

async fn revisions_in(database: &Arc<SurrealDbClient>, namespace: &str) -> Vec<i64> {
    let rows = database
        .query(
            "SELECT VALUE revision FROM knowledge_revision ORDER BY revision ASC",
            None,
            namespace,
        )
        .await
        .expect("read committed revision ledger");
    serde_json::from_value(rows).expect("decode revision values")
}

#[tokio::test]
async fn episode_visibility_and_revision_ledger_commit_together() {
    let database = database().await;
    let store = EpisodeStoreClient::new(database.clone(), "org");

    store
        .create("episode:event-1", episode("event-1"))
        .await
        .expect("create revisioned episode");

    let stored = database
        .query(
            "SELECT known_revision FROM episode:⟨event-1⟩ LIMIT 1",
            None,
            "org",
        )
        .await
        .expect("read episode revision");
    assert_eq!(stored[0]["known_revision"], 1);
    assert_eq!(revisions(&database).await, vec![1]);
}

#[tokio::test]
async fn ingestion_cannot_backdate_server_transaction_time() {
    let database = database().await;
    let service = MemoryService::new(database.clone(), "org".into(), "warn".into(), 50, 100)
        .expect("create memory service");
    let caller_time = chrono::DateTime::parse_from_rfc3339("2020-01-01T00:00:00Z")
        .expect("valid caller time")
        .with_timezone(&chrono::Utc);
    let started_at = chrono::Utc::now();

    let episode_id = service
        .ingest(
            memory_mcp::models::IngestRequest {
                source_type: "conversation".into(),
                source_id: "caller-backdated".into(),
                content: "The maintenance window is next week.".into(),
                t_ref: chrono::Utc::now(),
                t_ingested: Some(caller_time),
                policy_tags: Vec::new(),
                source_context: None,
            },
            None,
        )
        .await
        .expect("ingest episode");

    let stored = database
        .select_one(&episode_id, "org")
        .await
        .expect("read ingested episode")
        .expect("episode exists");
    let recorded_at = stored["t_ingested"]
        .as_str()
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
        .expect("server transaction time")
        .with_timezone(&chrono::Utc);
    assert!(recorded_at >= started_at);
    assert_ne!(recorded_at, caller_time);
    assert_eq!(stored["known_revision"], 1);
}

#[tokio::test]
async fn rejected_episode_write_does_not_advance_the_knowledge_clock() {
    let database = database().await;
    let store = EpisodeStoreClient::new(database.clone(), "org");

    let result = store
        .create(
            "episode:invalid",
            json!({"content": "missing required fields"}),
        )
        .await;

    assert!(result.is_err());
    assert!(revisions(&database).await.is_empty());
    let clock = database
        .query(
            "SELECT latest_revision FROM knowledge_revision_clock:active LIMIT 1",
            None,
            "org",
        )
        .await
        .expect("read knowledge clock");
    assert_eq!(clock[0]["latest_revision"], 0);
}

#[tokio::test]
async fn replaying_the_same_deterministic_episode_write_keeps_one_revision() {
    let database = database().await;
    let store = EpisodeStoreClient::new(database.clone(), "org");
    let content = episode("event-1");

    store
        .create("episode:event-1", content.clone())
        .await
        .expect("first write");
    store
        .create("episode:event-1", content)
        .await
        .expect("idempotent replay");

    assert_eq!(revisions(&database).await, vec![1]);
}

#[tokio::test]
async fn concurrent_episode_commits_receive_distinct_ordered_revisions() {
    let database = database().await;
    let first = EpisodeStoreClient::new(database.clone(), "org");
    let second = EpisodeStoreClient::new(database.clone(), "org");

    let (first, second) = tokio::join!(
        first.create("episode:event-1", episode("event-1")),
        second.create("episode:event-2", episode("event-2")),
    );

    first.expect("first concurrent write");
    second.expect("second concurrent write");
    assert_eq!(revisions(&database).await, vec![1, 2]);
}

#[tokio::test]
async fn concurrent_surreal_kv_commits_retry_conflicts_without_losing_revisions() {
    let directory = tempfile::tempdir().expect("temporary SurrealKV directory");
    let database = durable_database(directory.path()).await;
    let store = EpisodeStoreClient::new(database.clone(), "knowledge_revisions");
    let outcomes = tokio::join!(
        store.create("episode:concurrent-0", episode("concurrent-0")),
        store.create("episode:concurrent-1", episode("concurrent-1")),
        store.create("episode:concurrent-2", episode("concurrent-2")),
        store.create("episode:concurrent-3", episode("concurrent-3")),
        store.create("episode:concurrent-4", episode("concurrent-4")),
        store.create("episode:concurrent-5", episode("concurrent-5")),
        store.create("episode:concurrent-6", episode("concurrent-6")),
        store.create("episode:concurrent-7", episode("concurrent-7")),
    );

    assert!(outcomes.0.is_ok());
    assert!(outcomes.1.is_ok());
    assert!(outcomes.2.is_ok());
    assert!(outcomes.3.is_ok());
    assert!(outcomes.4.is_ok());
    assert!(outcomes.5.is_ok());
    assert!(outcomes.6.is_ok());
    assert!(outcomes.7.is_ok());
    assert_eq!(
        revisions_in(&database, "knowledge_revisions").await,
        (1..=8).collect::<Vec<_>>()
    );
    drop(store);
    database
        .shutdown()
        .await
        .expect("gracefully close concurrent test database");
}

#[tokio::test]
async fn canonical_fact_creation_is_stamped_with_its_commit_revision() {
    let database = database().await;
    let service = MemoryService::new(database.clone(), "org".into(), "warn".into(), 50, 100)
        .expect("create memory service");

    let fact_id = service
        .add_fact(
            "note",
            "The pump inspection is due next week.",
            "The pump inspection is due next week.",
            "episode:inspection-note",
            chrono::DateTime::parse_from_rfc3339("2026-05-01T10:00:00Z")
                .expect("valid fact time")
                .with_timezone(&chrono::Utc),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::manual(),
        )
        .await
        .expect("create canonical fact");

    let fact = database
        .select_one(&fact_id, "org")
        .await
        .expect("read canonical fact")
        .expect("fact exists");
    assert_eq!(fact["known_revision"], 1);
    assert_eq!(revisions(&database).await, vec![1]);
}

#[cfg(feature = "eval-support")]
#[tokio::test]
async fn eval_observer_separates_revision_lock_wait_from_transaction_duration() {
    let database = database().await;
    let timings = Arc::new(RevisionTimingCollector::default());
    database.set_eval_query_observer(timings.clone());
    let service = MemoryService::new(database.clone(), "org".into(), "warn".into(), 50, 100)
        .expect("create memory service");

    service
        .add_fact(
            "note",
            "The revision timing probe is persisted as a fact.",
            "The revision timing probe is persisted as a fact.",
            "episode:revision-timing-probe",
            chrono::Utc::now(),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::manual(),
        )
        .await
        .expect("revisioned fact write");

    let observed = timings.0.lock().expect("revision timing collector lock");
    let [(lock_wait, transaction)] = observed.as_slice() else {
        panic!("expected one isolated revision transaction observation, got {observed:?}");
    };
    assert!(
        *lock_wait < 1_000_000,
        "lock wait should be below one second"
    );
    assert!(
        *transaction > 0,
        "transaction execution must be observed independently from lock wait"
    );
}

#[tokio::test]
async fn invalidation_commits_fact_and_derived_claims_at_one_revision() {
    let database = database().await;
    let service = MemoryService::new(database.clone(), "org".into(), "warn".into(), 50, 100)
        .expect("create memory service");
    let fact_id = service
        .add_fact(
            "note",
            "The pump inspection is due next week.",
            "The pump inspection is due next week.",
            "episode:inspection-note",
            chrono::DateTime::parse_from_rfc3339("2026-05-01T10:00:00Z")
                .expect("valid fact time")
                .with_timezone(&chrono::Utc),
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::manual(),
        )
        .await
        .expect("create canonical fact");

    database
        .create(
            "claim:derived",
            json!({
                "claim_id": "claim:derived",
                "namespace": "org",
                "source_fact_id": fact_id,
                "source_episode_id": "episode:inspection-note",
                "scope": "org",
                "project_identity": "",
                "policy_tags": [],
                "access_policy_fingerprint": "test",
                "schema_family": "attribute",
                "schema_version": 1,
                "subject": {"kind": "entity", "value": "pump"},
                "subject_key": "pump",
                "comparison_key": {},
                "comparison_key_hash": "comparison",
                "qualifiers": {},
                "qualifier_hash": "qualifier",
                "slot_fingerprint": "slot",
                "value": {"kind": "text", "value": "inspection next week"},
                "cardinality": "single_valued",
                "observed_at": "2026-05-01T10:00:00Z",
                "valid_from": "2026-05-01T10:00:00Z",
                "valid_to": null,
                "validity_source": "source",
                "source_lineage": "manual",
                "derivation": {},
                "extractor_fingerprint": "test",
                "t_ingested": "2026-11-21T10:00:00Z",
                "t_invalid_ingested": null
            }),
            "org",
            &["observed_at", "valid_from", "t_ingested"],
        )
        .await
        .expect("seed derived claim");
    database
        .create(
            "claim_relation:derived",
            json!({
                "claim_relation_id": "claim_relation:derived",
                "left_claim_id": "claim:derived",
                "right_claim_id": "claim:other",
                "pair_fingerprint": "pair",
                "outcome": "contradicts",
                "predecessor_claim_id": null,
                "successor_claim_id": null,
                "reason_code": "test",
                "evidence": {},
                "evaluator_version": "test",
                "context_fingerprint": "context",
                "evaluated_at": "2026-11-21T10:00:00Z",
                "supersedes_relation_id": null,
                "scope": "org",
                "project": null,
                "policy_tags": [],
                "t_ingested": "2026-11-21T10:00:00Z",
                "t_invalid_ingested": null,
                "left_fact_id": fact_id,
                "right_fact_id": "fact:other"
            }),
            "org",
            &["evaluated_at", "t_ingested"],
        )
        .await
        .expect("seed derived relation");

    service
        .invalidate(
            memory_mcp::models::InvalidateRequest {
                fact_id: fact_id.clone(),
                t_invalid: chrono::DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z")
                    .expect("valid close time")
                    .with_timezone(&chrono::Utc),
                reason: "corrected source".into(),
            },
            None,
        )
        .await
        .expect("invalidate fact");

    let fact = database
        .select_one(&fact_id, "org")
        .await
        .expect("read closed fact")
        .expect("fact exists");
    let claim = database
        .select_one("claim:derived", "org")
        .await
        .expect("read closed claim")
        .expect("claim exists");
    let relation = database
        .select_one("claim_relation:derived", "org")
        .await
        .expect("read closed relation")
        .expect("relation exists");

    assert_eq!(fact["closed_revision"], 2);
    assert_eq!(claim["closed_revision"], 2);
    assert_eq!(relation["closed_revision"], 2);
    assert_eq!(revisions(&database).await, vec![1, 2]);
    let ledger = database
        .query(
            "SELECT affected_record_ids FROM knowledge_revision WHERE revision = 2 LIMIT 1",
            None,
            "org",
        )
        .await
        .expect("read invalidation revision ledger");
    let affected: Vec<String> = serde_json::from_value(ledger[0]["affected_record_ids"].clone())
        .expect("decode affected record ids");
    assert_eq!(affected.len(), 3);
    assert!(affected.contains(&fact_id));
    assert!(affected.contains(&"claim:derived".to_string()));
    assert!(affected.contains(&"claim_relation:derived".to_string()));
}

#[tokio::test]
async fn revision_clock_and_ledger_survive_surreal_kv_restart() {
    let directory = tempfile::tempdir().expect("temporary SurrealKV directory");
    let first = durable_database(directory.path()).await;
    let first_store = EpisodeStoreClient::new(first.clone(), "knowledge_revisions");
    first_store
        .create("episode:first", episode("first"))
        .await
        .expect("first durable episode");
    assert_eq!(revisions_in(&first, "knowledge_revisions").await, vec![1]);
    drop(first_store);
    first
        .shutdown()
        .await
        .expect("gracefully close first client");
    drop(first);

    let reopened = durable_database(directory.path()).await;
    let reopened_store = EpisodeStoreClient::new(reopened.clone(), "knowledge_revisions");
    assert_eq!(
        revisions_in(&reopened, "knowledge_revisions").await,
        vec![1]
    );
    reopened_store
        .create("episode:second", episode("second"))
        .await
        .expect("second durable episode");

    assert_eq!(
        revisions_in(&reopened, "knowledge_revisions").await,
        vec![1, 2]
    );
    drop(reopened_store);
    reopened
        .shutdown()
        .await
        .expect("gracefully close reopened client");
}

#[tokio::test]
async fn caller_backdated_ingestion_time_does_not_backdate_commit_provenance() {
    let database = database().await;
    let store = EpisodeStoreClient::new(database.clone(), "org");
    let mut content = episode("backdated");
    content["t_ingested"] = json!("2001-01-01T00:00:00Z");

    store
        .create("episode:backdated", content)
        .await
        .expect("create episode with a legacy caller timestamp");

    let ledger = database
        .query(
            "SELECT recorded_at FROM knowledge_revision WHERE revision = 1",
            None,
            "org",
        )
        .await
        .expect("read server commit provenance");
    assert_eq!(ledger.as_array().map(Vec::len), Some(1));
    let recorded_at = ledger[0]["recorded_at"]
        .as_str()
        .expect("revision timestamp is serialized as a timestamp");
    assert!(!recorded_at.starts_with("2001-01-01"));
}

#[tokio::test]
async fn equal_server_timestamps_keep_distinct_ordered_revision_tokens() {
    let database = database().await;
    database
        .query(
            "UPDATE knowledge_revision_clock:active SET \
             last_recorded_at = type::datetime('2099-12-31T23:59:59Z')",
            None,
            "org",
        )
        .await
        .expect("seed an equal timestamp boundary");
    let store = EpisodeStoreClient::new(database.clone(), "org");

    store
        .create("episode:equal-1", episode("equal-1"))
        .await
        .expect("first episode at seeded boundary");
    store
        .create("episode:equal-2", episode("equal-2"))
        .await
        .expect("second episode at seeded boundary");

    let timestamps = database
        .query(
            "SELECT VALUE recorded_at FROM knowledge_revision ORDER BY revision ASC",
            None,
            "org",
        )
        .await
        .expect("read revision timestamps");
    assert_eq!(timestamps[0], timestamps[1]);
    assert_eq!(revisions(&database).await, vec![1, 2]);
}

#[tokio::test]
async fn backwards_wall_clock_keeps_recorded_revision_time_non_decreasing() {
    let database = database().await;
    database
        .query(
            "UPDATE knowledge_revision_clock:active SET \
             last_recorded_at = type::datetime('2099-12-31T23:59:59Z')",
            None,
            "org",
        )
        .await
        .expect("move the recorded boundary ahead of the current wall clock");
    let store = EpisodeStoreClient::new(database.clone(), "org");

    store
        .create("episode:clock-regressed", episode("clock-regressed"))
        .await
        .expect("commit with a regressed wall clock");

    let ledger = database
        .query(
            "SELECT recorded_at FROM knowledge_revision WHERE revision = 1",
            None,
            "org",
        )
        .await
        .expect("read the committed boundary");
    assert_eq!(
        ledger[0]["recorded_at"], "2099-12-31T23:59:59Z",
        "recorded time must not move behind the previous commit boundary"
    );
}
