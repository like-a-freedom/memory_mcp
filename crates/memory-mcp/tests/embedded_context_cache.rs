mod common;

use chrono::{DateTime, Duration, Utc};
use memory_mcp::models::{AssembleContextRequest, InvalidateRequest, Provenance};
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_invalidate::InvalidateCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use tokio::sync::{Notify, Semaphore};
use tokio::time::timeout;

async fn service_with_recallable_fact()
-> Result<(MemoryService, DateTime<Utc>, String), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let now = Utc::now();
    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache",
            now - Duration::days(1),
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache"),
        )
        .await?;
    Ok((service, now, fact_id))
}

fn request() -> AssembleContextRequest {
    AssembleContextRequest {
        query: "ARR".to_string(),
        as_of: None,
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    }
}

fn historical_request(as_of: DateTime<Utc>) -> AssembleContextRequest {
    AssembleContextRequest {
        as_of: Some(as_of),
        ..request()
    }
}

async fn service_with_future_valid_fact(
    t_valid: DateTime<Utc>,
) -> Result<(MemoryService, String), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache-history",
            t_valid,
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache-history"),
        )
        .await?;
    Ok((service, fact_id))
}

#[tokio::test]
async fn embedded_context_cache_returns_same_results() -> Result<(), Box<dyn std::error::Error>> {
    let (service, _, _) = service_with_recallable_fact().await?;
    let first =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    let second =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;

    assert_eq!(first, second);
    Ok(())
}

#[tokio::test]
async fn invalidation_clears_cached_context_before_the_next_assembly()
-> Result<(), Box<dyn std::error::Error>> {
    let (service, now, fact_id) = service_with_recallable_fact().await?;
    let before =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert_eq!(before.len(), 1);

    InvalidateCapability::invalidate_from_service(
        &service,
        InvalidateRequest {
            fact_id,
            reason: "no longer current".to_string(),
            t_invalid: now - Duration::seconds(1),
        },
        None,
    )
    .await?;

    let after =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert!(after.is_empty());
    Ok(())
}

#[tokio::test]
async fn ingestion_invalidates_a_warmed_empty_current_context()
-> Result<(), Box<dyn std::error::Error>> {
    let service = common::make_service().await;
    let empty =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert!(empty.is_empty());

    let fact_id = service
        .add_fact(
            "metric",
            "ARR $5M",
            "ARR $5M",
            "episode:cache-warmed-empty",
            Utc::now() - Duration::days(1),
            0.8,
            vec![],
            vec![],
            Provenance::agent_observation("episode:cache-warmed-empty"),
        )
        .await?;

    let after_ingest =
        AssembleContextCapability::assemble_context_from_service(&service, request()).await?;
    assert_eq!(after_ingest.len(), 1);
    assert_eq!(after_ingest[0].fact_id, fact_id);
    Ok(())
}

#[tokio::test]
async fn historical_cutoffs_in_one_bucket_do_not_reuse_an_empty_result()
-> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let next_bucket = now.timestamp().div_euclid(300) * 300 + 300;
    let before_valid_time = DateTime::from_timestamp(next_bucket + 30, 0)
        .ok_or("before cutoff is outside chrono range")?;
    let valid_time = DateTime::from_timestamp(next_bucket + 90, 0)
        .ok_or("valid time is outside chrono range")?;
    let after_valid_time = DateTime::from_timestamp(next_bucket + 150, 0)
        .ok_or("after cutoff is outside chrono range")?;
    let (service, fact_id) = service_with_future_valid_fact(valid_time).await?;

    let before = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(before_valid_time),
    )
    .await?;
    assert!(before.is_empty());

    let after = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(after_valid_time),
    )
    .await?;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].fact_id, fact_id);
    Ok(())
}

#[tokio::test]
async fn historical_cutoffs_in_one_bucket_do_not_reuse_a_later_result()
-> Result<(), Box<dyn std::error::Error>> {
    let now = Utc::now();
    let next_bucket = now.timestamp().div_euclid(300) * 300 + 300;
    let before_valid_time = DateTime::from_timestamp(next_bucket + 30, 0)
        .ok_or("before cutoff is outside chrono range")?;
    let valid_time = DateTime::from_timestamp(next_bucket + 90, 0)
        .ok_or("valid time is outside chrono range")?;
    let after_valid_time = DateTime::from_timestamp(next_bucket + 150, 0)
        .ok_or("after cutoff is outside chrono range")?;
    let (service, fact_id) = service_with_future_valid_fact(valid_time).await?;

    let after = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(after_valid_time),
    )
    .await?;
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].fact_id, fact_id);

    let before = AssembleContextCapability::assemble_context_from_service(
        &service,
        historical_request(before_valid_time),
    )
    .await?;
    assert!(before.is_empty());
    Ok(())
}

#[tokio::test]
async fn fact_creation_invalidates_context_warmed_during_claim_projection()
-> Result<(), Box<dyn std::error::Error>> {
    let inner =
        Arc::new(SurrealDbClient::connect_in_memory("cache_projection_race", "org", "warn").await?);
    inner.apply_migrations("org").await?;

    let gate = Arc::new(ClaimProjectionGate::default());
    let database = Arc::new(ProjectionBarrierDb {
        inner,
        gate: gate.clone(),
        fact_query_count: Arc::new(AtomicUsize::new(0)),
    });
    let fact_query_count = database.fact_query_count.clone();
    let service = Arc::new(
        MemoryService::new(database, "org".to_string(), "warn".to_string(), 50, 100)?
            .with_claim_rollout_stage("evidence")?,
    );
    let now = Utc::now();
    let add_service = service.clone();
    let add_fact = tokio::spawn(async move {
        add_service
            .add_fact(
                "metric",
                "Temperature is 36.5 celsius",
                "Temperature is 36.5 celsius",
                "episode:cache-projection-race",
                now,
                0.9,
                vec![],
                vec![],
                Provenance::agent_observation("episode:cache-projection-race"),
            )
            .await
    });

    timeout(std::time::Duration::from_secs(5), gate.started.notified())
        .await
        .expect("fact projection must reach its persisted claim before timeout");

    let request = AssembleContextRequest {
        query: "temperature 36.5 celsius".to_string(),
        as_of: Some(now + Duration::seconds(1)),
        budget: 5,
        fact_types: vec![],
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        compact: false,
    };
    let queries_before_warm = fact_query_count.load(Ordering::SeqCst);
    let during_projection = timeout(
        std::time::Duration::from_secs(5),
        AssembleContextCapability::assemble_context_from_service(&service, request.clone()),
    )
    .await
    .expect("assembly during claim projection must finish")?;
    assert_eq!(during_projection.len(), 1);
    let fact_id = during_projection[0].fact_id.clone();
    let queries_after_warm = fact_query_count.load(Ordering::SeqCst);
    assert!(
        queries_after_warm > queries_before_warm,
        "assembly during projection must read the persisted fact and warm the cache"
    );

    gate.release.add_permits(1);
    let persisted_id = timeout(std::time::Duration::from_secs(5), add_fact)
        .await
        .expect("fact creation must finish after claim projection is released")??;
    assert_eq!(persisted_id, fact_id);

    let queries_before_recall = fact_query_count.load(Ordering::SeqCst);
    let after_projection = timeout(
        std::time::Duration::from_secs(5),
        AssembleContextCapability::assemble_context_from_service(&service, request),
    )
    .await
    .expect("assembly after claim projection must finish")?;
    assert_eq!(after_projection.len(), 1);
    assert_eq!(after_projection[0].fact_id, fact_id);
    assert!(
        fact_query_count.load(Ordering::SeqCst) > queries_before_recall,
        "assembly after projection must miss the result cached while projection was pending"
    );

    Ok(())
}

struct ClaimProjectionGate {
    started: Notify,
    release: Semaphore,
    blocked: AtomicBool,
}

impl Default for ClaimProjectionGate {
    fn default() -> Self {
        Self {
            started: Notify::new(),
            release: Semaphore::new(0),
            blocked: AtomicBool::new(false),
        }
    }
}

struct ProjectionBarrierDb {
    inner: Arc<SurrealDbClient>,
    gate: Arc<ClaimProjectionGate>,
    fact_query_count: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl DbClient for ProjectionBarrierDb {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<Value>, memory_mcp::MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<Value>, memory_mcp::MemoryError> {
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
        if record_id.starts_with("claim:") && !self.gate.blocked.swap(true, Ordering::SeqCst) {
            self.gate.started.notify_one();
            self.gate
                .release
                .acquire()
                .await
                .expect("claim projection release semaphore remains open")
                .forget();
        }
        self.inner
            .create(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn update(
        &self,
        record_id: &str,
        content: Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<Value, memory_mcp::MemoryError> {
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<Value>,
        namespace: &str,
    ) -> Result<Value, memory_mcp::MemoryError> {
        if sql.contains("FROM fact") {
            self.fact_query_count.fetch_add(1, Ordering::SeqCst);
        }
        self.inner.query(sql, vars, namespace).await
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), memory_mcp::MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
}
