use std::sync::Arc;

use crate::embedding::providers::task_runner::BackgroundTaskRunner;
use crate::http::middleware::preflight_budget::PreflightBudget;
use crate::http::runtime::pool::Pool;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct HttpMemorySnapshot {
    pub(crate) reserved_requests: usize,
    pub(crate) reserved_bytes: usize,
    pub(crate) resident_runtime_count: usize,
    pub(crate) context_cache_accounted_bytes: usize,
    pub(crate) query_cache_accounted_bytes: usize,
    pub(crate) background_embedding_admitted_tasks: usize,
    pub(crate) background_embedding_running_tasks: usize,
    pub(crate) background_embedding_retained_bytes: usize,
}

#[derive(Clone)]
pub struct HttpMemorySnapshotSource {
    pool: Arc<Pool>,
    preflight_budget: Arc<PreflightBudget>,
    background_task_runner: Arc<BackgroundTaskRunner>,
}

impl HttpMemorySnapshotSource {
    pub(crate) fn new(
        pool: Arc<Pool>,
        preflight_budget: Arc<PreflightBudget>,
        background_task_runner: Arc<BackgroundTaskRunner>,
    ) -> Self {
        Self {
            pool,
            preflight_budget,
            background_task_runner,
        }
    }

    pub(crate) async fn capture(&self) -> HttpMemorySnapshot {
        let preflight = self.preflight_budget.resource_snapshot();
        let runtimes = self.pool.resident_runtime_handles();
        let resident_runtime_count = runtimes.len();
        let mut context_cache_accounted_bytes = 0_usize;
        let mut query_cache_accounted_bytes = 0_usize;
        for runtime in runtimes {
            let retained = runtime.retained_resource_snapshot().await;
            context_cache_accounted_bytes = context_cache_accounted_bytes
                .saturating_add(retained.context_cache_accounted_bytes);
            query_cache_accounted_bytes =
                query_cache_accounted_bytes.saturating_add(retained.query_cache_accounted_bytes);
        }
        let background = self.background_task_runner.resource_snapshot();

        HttpMemorySnapshot {
            reserved_requests: preflight.reserved_requests,
            reserved_bytes: preflight.reserved_bytes,
            resident_runtime_count,
            context_cache_accounted_bytes,
            query_cache_accounted_bytes,
            background_embedding_admitted_tasks: background.admitted_tasks,
            background_embedding_running_tasks: background.running_tasks,
            background_embedding_retained_bytes: background.retained_bytes,
        }
    }
}

#[cfg(feature = "test-fixtures")]
pub struct HttpMemorySnapshotTestFixture {
    source: HttpMemorySnapshotSource,
    preflight_budget: Arc<PreflightBudget>,
}

#[cfg(feature = "test-fixtures")]
impl HttpMemorySnapshotTestFixture {
    pub async fn new() -> Result<Self, crate::error::MemoryError> {
        let config = crate::http::config::HttpConfig::default_for_test();
        let background_task_runner = Arc::new(BackgroundTaskRunner::new());
        let registry = Arc::new(
            crate::http::registry::RegistryHandle::in_memory_with_default_mem_engine().await,
        );
        let pool = Arc::new(Pool::from_http_config_with_shutdown(
            &config,
            registry,
            crate::http::shutdown::ShutdownState::new(),
            None,
            Arc::clone(&background_task_runner),
        ));
        let preflight_budget = Arc::new(PreflightBudget::new(
            config.preflight_request_limit,
            config.preflight_bytes,
        )?);
        let source = HttpMemorySnapshotSource::new(
            pool,
            Arc::clone(&preflight_budget),
            background_task_runner,
        );

        Ok(Self {
            source,
            preflight_budget,
        })
    }

    pub fn source(&self) -> Arc<HttpMemorySnapshotSource> {
        Arc::new(self.source.clone())
    }

    pub fn reserve_preflight(
        &self,
        bytes: usize,
    ) -> Result<
        crate::http::middleware::preflight_budget::PreflightReservation,
        crate::http::middleware::preflight_budget::PreflightRefusal,
    > {
        self.preflight_budget.try_reserve(bytes)
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use crate::embedding::providers::task_runner::BackgroundTaskRunner;
    use crate::http::config::HttpConfig;
    use crate::http::middleware::preflight_budget::PreflightBudget;
    use crate::http::registry::RegistryHandle;
    use crate::http::runtime::pool::{DEFAULT_PER_TENANT_CONCURRENCY, Pool};
    use crate::http::runtime::storage::TenantRuntime;
    use crate::http::shutdown::ShutdownState;
    use crate::memory::context_cache::{CacheInsertOutcome, ContextCacheLookup};
    use crate::models::AssembledContextItem;
    use crate::platform::context_cache_key::{CacheKey, CacheView};
    use crate::tenancy::api::{TenantLifecycleStatus, TenantRuntimeSpec};

    use super::HttpMemorySnapshotSource;

    async fn harness(
        capacity: usize,
    ) -> (
        Arc<Pool>,
        HttpMemorySnapshotSource,
        Arc<BackgroundTaskRunner>,
    ) {
        harness_with_idle_ttl(capacity, Duration::from_secs(60)).await
    }

    async fn harness_with_idle_ttl(
        capacity: usize,
        idle_ttl: Duration,
    ) -> (
        Arc<Pool>,
        HttpMemorySnapshotSource,
        Arc<BackgroundTaskRunner>,
    ) {
        let mut config = HttpConfig::default_for_test();
        config.pool_cap = capacity;
        config.runtime_idle_ttl = idle_ttl;
        let runner = Arc::new(BackgroundTaskRunner::new());
        let registry = Arc::new(RegistryHandle::in_memory_with_default_mem_engine().await);
        let pool = Arc::new(Pool::from_http_config_with_shutdown(
            &config,
            registry,
            ShutdownState::new(),
            None,
            Arc::clone(&runner),
        ));
        let budget = Arc::new(PreflightBudget::new(capacity, 1_024).expect("valid budget"));
        let source = HttpMemorySnapshotSource::new(Arc::clone(&pool), budget, Arc::clone(&runner));
        (pool, source, runner)
    }

    fn runtime_spec(tenant: &str, namespace: &str) -> TenantRuntimeSpec {
        TenantRuntimeSpec {
            tenant_id: tenant.to_owned(),
            namespace: namespace.to_owned(),
            database: "memory".to_owned(),
            plan_version: 1,
            schema_version: 0,
            status: TenantLifecycleStatus::Ready,
        }
    }

    async fn acquire(pool: &Arc<Pool>, tenant: &str, namespace: &str) -> Arc<TenantRuntime> {
        let spec = runtime_spec(tenant, namespace);
        let guard = pool
            .acquire_spec_with_limit(&spec, DEFAULT_PER_TENANT_CONCURRENCY)
            .await
            .expect("runtime activates");
        Arc::clone(guard.runtime())
    }

    async fn retain_cache_entries(runtime: &TenantRuntime, key: &str) -> (usize, usize) {
        let service = runtime.mcp_service.service();
        let context_bytes = {
            let mut context = service.context_cache.write().await;
            let cache_key =
                CacheKey::new(key, chrono::Utc::now(), 1, &[], CacheView::default(), None);
            let generation = match context.lookup(&cache_key) {
                ContextCacheLookup::Miss(generation) => generation,
                ContextCacheLookup::Hit(_) => panic!("fresh runtime cache starts empty"),
            };
            let item = AssembledContextItem {
                fact_id: format!("fact-{key}"),
                content: key.to_owned(),
                quote: key.to_owned(),
                source_episode: format!("episode-{key}"),
                confidence: 1.0,
                relevance: None,
                grounding: None,
                semantic_available: None,
                provenance: serde_json::Value::Null,
                rationale: key.to_owned(),
                retrieval_tier: None,
                reconciliation: None,
            };
            assert_eq!(
                context.insert(generation, cache_key, &[item]),
                CacheInsertOutcome::Stored
            );
            context.accounted_bytes()
        };
        let query_bytes = {
            let mut query = service.query_embedding_cache.lock().await;
            assert_eq!(
                query.insert(key.to_owned(), vec![0.25; 4], Instant::now()),
                crate::embedding::query_cache::QueryCacheInsertOutcome::Stored
            );
            query.accounted_bytes()
        };
        (context_bytes, query_bytes)
    }

    #[tokio::test]
    async fn snapshot_sums_two_resident_runtime_cache_estimates() {
        let (pool, source, _) = harness(2).await;
        let first = acquire(&pool, "snapshot-a", "namespace-a").await;
        let first_bytes = retain_cache_entries(&first, "query-a").await;
        let second = acquire(&pool, "snapshot-b", "namespace-b").await;
        let second_bytes = retain_cache_entries(&second, "query-b").await;
        drop((first, second));

        let snapshot = source.capture().await;
        assert_eq!(
            snapshot.context_cache_accounted_bytes,
            first_bytes.0 + second_bytes.0
        );
        assert_eq!(
            snapshot.query_cache_accounted_bytes,
            first_bytes.1 + second_bytes.1
        );
    }

    #[tokio::test]
    async fn snapshot_counts_shared_coordinator_once() {
        let (pool, source, runner) = harness(2).await;
        let first = acquire(&pool, "snapshot-a", "namespace-a").await;
        let second = acquire(&pool, "snapshot-b", "namespace-b").await;
        let reservation = runner
            .try_admit("snapshot-task", 37)
            .expect("task admitted");
        drop((first, second));

        let snapshot = source.capture().await;
        assert_eq!(snapshot.resident_runtime_count, 2);
        assert_eq!(snapshot.background_embedding_admitted_tasks, 1);
        assert_eq!(snapshot.background_embedding_running_tasks, 0);
        assert_eq!(snapshot.background_embedding_retained_bytes, 37);
        drop(reservation);
    }

    #[tokio::test]
    async fn snapshot_exposes_no_tenant_identity_or_cache_payload() {
        let (pool, source, _) = harness(1).await;
        let tenant_identity = "tenant-private-snapshot-identity";
        let cache_payload = "cache-private-snapshot-payload";
        let runtime = acquire(&pool, tenant_identity, "namespace-private").await;
        retain_cache_entries(&runtime, cache_payload).await;
        drop(runtime);

        let snapshot = source.capture().await;
        let debug_snapshot = format!("{snapshot:?}");
        assert_eq!(snapshot.resident_runtime_count, 1);
        assert!(!debug_snapshot.contains(tenant_identity));
        assert!(!debug_snapshot.contains(cache_payload));
    }

    #[tokio::test]
    async fn snapshot_reflects_runtime_eviction_on_next_capture() {
        let (pool, source, _) = harness_with_idle_ttl(1, Duration::ZERO).await;
        let runtime = acquire(&pool, "snapshot-eviction", "namespace-eviction").await;
        retain_cache_entries(&runtime, "query-eviction").await;
        drop(runtime);

        let before_eviction = source.capture().await;
        assert_eq!(before_eviction.resident_runtime_count, 1);
        assert!(before_eviction.context_cache_accounted_bytes > 0);
        assert!(before_eviction.query_cache_accounted_bytes > 0);

        assert_eq!(pool.evict_idle().await, 1);
        let after_eviction = source.capture().await;
        assert_eq!(after_eviction.resident_runtime_count, 0);
        assert_eq!(after_eviction.context_cache_accounted_bytes, 0);
        assert_eq!(after_eviction.query_cache_accounted_bytes, 0);
    }

    #[tokio::test]
    async fn pool_eviction_proceeds_while_snapshot_waits_for_cache_lock() {
        let (pool, source, _) = harness_with_idle_ttl(1, Duration::ZERO).await;
        let runtime = acquire(&pool, "snapshot-lock", "namespace-lock").await;
        let service = runtime.mcp_service.service();
        let context_guard = service.context_cache.write().await;
        let mut capture = Box::pin(source.capture());
        let first_poll =
            std::future::poll_fn(|cx| std::task::Poll::Ready(capture.as_mut().poll(cx))).await;
        assert!(first_poll.is_pending(), "capture waits for the cache owner");

        let pool_for_eviction = Arc::clone(&pool);
        let eviction = tokio::task::spawn_blocking(move || {
            tokio::runtime::Handle::current().block_on(pool_for_eviction.evict_idle())
        });
        let evicted = tokio::time::timeout(Duration::from_secs(1), eviction)
            .await
            .expect("pool eviction must not wait for the cache lock")
            .expect("eviction task completes");
        assert_eq!(evicted, 1);
        assert!(pool.resident_runtime_handles().is_empty());

        drop(context_guard);
        let captured = tokio::time::timeout(Duration::from_secs(1), capture)
            .await
            .expect("capture completes after the cache lock is released");
        assert_eq!(captured.resident_runtime_count, 1);
        assert_eq!(source.capture().await.resident_runtime_count, 0);
    }
}
