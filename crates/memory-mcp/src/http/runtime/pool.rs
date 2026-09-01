//! Runtime pool + admission gate.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use lru::LruCache;

/// Local warn helper. The workspace does not depend on
/// `tracing` or `log`; for now the helper is a no-op and
/// the caller surfaces the error in the response.
#[allow(dead_code)]
fn tracing_warn(message: &str) {
    let _ = message;
}

use crate::error::MemoryError;
use crate::http::registry::models::Tenant;

use super::lifecycle::{RuntimePhase, TenantRuntimeSlot};
use super::storage::build_runtime;

/// Errors returned by bounded runtime acquisition.
#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("runtime capacity wait timed out")]
    CapacityTimeout,
    #[error("tenant runtime activation failed")]
    ActivationFailed,
    #[error("server is shutting down")]
    ShuttingDown,
}

// ─── Admission gate ───────────────────────────────────────

/// Admission gate. Provides global request and subscription
/// budgets plus the `AdmissionPermit` RAII handle.
pub struct AdmissionGate {
    global_limit: u32,
    global_active: AtomicU32,
    subscription_limit: u32,
    subscription_active: AtomicU32,
    closed: AtomicBool,
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self::new(256)
    }
}

impl AdmissionGate {
    /// Default global in-flight request bound. Environment-configurable
    /// override arrives with the quota system.
    pub fn new(global_limit: u32) -> Self {
        Self {
            global_limit,
            global_active: AtomicU32::new(0),
            subscription_limit: 32,
            subscription_active: AtomicU32::new(0),
            closed: AtomicBool::new(false),
        }
    }

    /// Back-compat with the earlier constructor.
    pub fn open() -> Self {
        Self::default()
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }

    /// Try to acquire one request permit.
    #[allow(clippy::result_unit_err)] // Spec uses Result<_, ()> as the boolean.
    pub fn try_acquire(self: &Arc<Self>) -> Result<AdmissionPermit, ()> {
        self.try_acquire_for(false)
    }

    /// Try to acquire either a request or a subscription
    /// permit. Long-lived subscriptions use a separate
    /// bounded budget and never consume ordinary request
    /// capacity.
    #[allow(clippy::result_unit_err)]
    pub fn try_acquire_for(self: &Arc<Self>, subscription: bool) -> Result<AdmissionPermit, ()> {
        if self.is_closed() {
            return Err(());
        }
        let (limit, counter) = if subscription {
            (self.subscription_limit, &self.subscription_active)
        } else {
            (self.global_limit, &self.global_active)
        };
        let mut current = counter.load(Ordering::SeqCst);
        loop {
            if current >= limit {
                return Err(());
            }
            match counter.compare_exchange_weak(
                current,
                current + 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => {
                    return Ok(if subscription {
                        AdmissionPermit::Subscription { gate: self.clone() }
                    } else {
                        AdmissionPermit::Request { gate: self.clone() }
                    });
                }
                Err(actual) => current = actual,
            }
        }
    }
}

/// Owned RAII permit. Moving the permit into a
/// `ResponseLease` keeps it alive for the body lifetime.
pub enum AdmissionPermit {
    Request { gate: Arc<AdmissionGate> },
    Subscription { gate: Arc<AdmissionGate> },
}

impl Drop for AdmissionPermit {
    fn drop(&mut self) {
        let counter = match self {
            Self::Request { gate } => &gate.global_active,
            Self::Subscription { gate } => &gate.subscription_active,
        };
        counter.fetch_sub(1, Ordering::SeqCst);
    }
}

// ─── Pool ─────────────────────────────────────────────────

/// Defaults: 32 active, 15-min idle, 2-sec capacity wait,
/// 30-sec activation timeout, 4 per-tenant concurrency.
/// Environment overrides arrive with the quota system.
pub const DEFAULT_POOL_CAP: usize = 32;
pub const DEFAULT_IDLE_TTL: Duration = Duration::from_secs(15 * 60);
pub const DEFAULT_CAPACITY_WAIT: Duration = Duration::from_secs(2);
pub const DEFAULT_ACTIVATION_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_PER_TENANT_CONCURRENCY: u32 = 4;

/// LRU pool of Tenant Runtimes. `acquire_or_wait` is the
/// single production entry point used by the HTTP pipeline.
/// The pool holds a `RegistryHandle` so it can call
/// `build_runtime`; the handle is cheap to clone.
pub struct Pool {
    map: Mutex<LruCache<String, Arc<Mutex<TenantRuntimeSlot>>>>,
    registry: Arc<crate::http::registry::RegistryHandle>,
    cap: usize,
    #[allow(dead_code)] // Read by future idle-eviction tick.
    idle_ttl: Duration,
    capacity_wait: Duration,
    #[allow(dead_code)] // Used by future per-tenant activation timeout.
    activation_timeout: Duration,
    #[allow(dead_code)] // Used by future per-tenant semaphore.
    per_tenant_concurrency: u32,
}

impl Pool {
    pub fn new(
        cap: usize,
        idle_ttl: Duration,
        capacity_wait: Duration,
        activation_timeout: Duration,
        per_tenant_concurrency: u32,
        registry: Arc<crate::http::registry::RegistryHandle>,
    ) -> Self {
        let cap = cap.max(1);
        Self {
            map: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(cap).unwrap_or(std::num::NonZeroUsize::MIN),
            )),
            registry,
            cap,
            idle_ttl,
            capacity_wait,
            activation_timeout,
            per_tenant_concurrency,
        }
    }

    /// Spec-default pool: 32 / 15-min / 2-sec / 30-sec / 4.
    pub fn with_defaults(registry: Arc<crate::http::registry::RegistryHandle>) -> Self {
        Self::new(
            DEFAULT_POOL_CAP,
            DEFAULT_IDLE_TTL,
            DEFAULT_CAPACITY_WAIT,
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        )
    }

    /// Capacity as reported to /health/ready and metrics.
    #[allow(dead_code)]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Acquire a runtime for the given tenant. Single-flights
    /// concurrent activations via the slot's broadcast
    /// channel. Pins the slot on success.
    pub async fn acquire_or_wait(
        self: &Arc<Self>,
        tenant: &Tenant,
    ) -> Result<super::guard::OperationGuard, PoolError> {
        let slot = self.slot_for(tenant).await?;
        let mut guard = slot.lock().await;

        // Fast path: already Ready.
        if guard.phase == RuntimePhase::Ready
            && let Some(runtime) = guard.runtime.clone()
        {
            guard.last_used = Instant::now();
            let pin = guard.pin_count.clone();
            drop(guard);
            return Ok(super::guard::OperationGuard::new(runtime, pin));
        }

        // Negative cache: short-circuit to ActivationFailed.
        if guard.activation.in_negative_backoff() {
            return Err(PoolError::ActivationFailed);
        }

        // Subscribe to the in-flight activation if one exists.
        if let Some(sender) = guard.activation.in_flight.clone() {
            let mut rx = sender.subscribe();
            drop(guard);
            match rx.recv().await {
                Ok(runtime) => {
                    let slot = match self.slot_for(tenant).await {
                        Ok(s) => s,
                        Err(e) => return Err(e),
                    };
                    let mut guard = slot.lock().await;
                    guard.last_used = Instant::now();
                    let pin = guard.pin_count.clone();
                    return Ok(super::guard::OperationGuard::new(runtime, pin));
                }
                Err(_) => {
                    let mut guard = slot.lock().await;
                    guard.activation.in_flight = None;
                    return Err(PoolError::ActivationFailed);
                }
            }
        }

        // First arriver: kick off the activation.
        let _rx = guard.activation.begin();
        let registry = self.registry.clone();
        let tenant_clone = tenant.clone();
        let tenant_id = tenant.id.clone();
        drop(guard);
        let activation_result = tokio::time::timeout(
            self.activation_timeout,
            build_runtime(&registry, &tenant_clone),
        )
        .await
        .map_err(|_| MemoryError::Unavailable("tenant runtime activation timed out".into()))
        .and_then(|result| result);
        let slot = match self.slot_for(tenant).await {
            Ok(s) => s,
            Err(e) => return Err(e),
        };
        let mut guard = slot.lock().await;
        match activation_result {
            Ok(runtime) => {
                let runtime = Arc::new(runtime);
                guard.runtime = Some(runtime.clone());
                guard.phase = RuntimePhase::Ready;
                if let Some(sender) = guard.activation.in_flight.take() {
                    let _ = sender.send(runtime.clone());
                }
                guard.last_used = Instant::now();
                let pin = guard.pin_count.clone();
                Ok(super::guard::OperationGuard::new(runtime, pin))
            }
            Err(error) => {
                guard.phase = RuntimePhase::Failed;
                guard.activation.in_flight = None;
                guard.activation.negative_backoff_until =
                    Some(Instant::now() + Duration::from_secs(5));
                tracing_warn(&format!(
                    "tenant runtime activation failed for {tenant_id}: {error}"
                ));
                Err(PoolError::ActivationFailed)
            }
        }
    }

    /// Get or create a slot for the tenant id. If the LRU
    /// is at capacity and the tenant is not already in the
    /// map, wait up to `capacity_wait` for a slot to free;
    /// return `CapacityTimeout` if none does.
    async fn slot_for(
        self: &Arc<Self>,
        tenant: &Tenant,
    ) -> Result<Arc<Mutex<TenantRuntimeSlot>>, PoolError> {
        let key = tenant.id.clone();
        loop {
            {
                let mut map = self.map.lock().await;
                if let Some(slot) = map.get(&key) {
                    return Ok(slot.clone());
                }
                if map.len() < self.cap {
                    let slot = Arc::new(Mutex::new(TenantRuntimeSlot::new()));
                    map.put(key.clone(), slot.clone());
                    return Ok(slot);
                }

                // Recover capacity synchronously before waiting. Only an
                // idle, unpinned Ready runtime may be removed; an activation
                // in flight or a pinned response remains protected.
                let threshold = Instant::now().checked_sub(self.idle_ttl);
                let candidate = map.iter().find_map(|(tenant_id, slot)| {
                    let mut slot_guard = slot.try_lock().ok()?;
                    if slot_guard.phase == RuntimePhase::Ready
                        && slot_guard.pin_count.load(Ordering::SeqCst) == 0
                        && threshold.is_some_and(|limit| slot_guard.last_used <= limit)
                    {
                        slot_guard.phase = RuntimePhase::Draining;
                        slot_guard.runtime = None;
                        slot_guard.phase = RuntimePhase::Unloaded;
                        Some(tenant_id.clone())
                    } else {
                        None
                    }
                });
                if let Some(candidate) = candidate {
                    map.pop(&candidate);
                    let slot = Arc::new(Mutex::new(TenantRuntimeSlot::new()));
                    map.put(key.clone(), slot.clone());
                    return Ok(slot);
                }
            }
            // At cap and tenant not present. Bounded wait for
            // a slot to free.
            match tokio::time::timeout(self.capacity_wait, async {
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                    let map = self.map.lock().await;
                    if map.len() < self.cap || map.contains(&key) {
                        return;
                    }
                }
            })
            .await
            {
                Ok(()) => continue,
                Err(_) => return Err(PoolError::CapacityTimeout),
            }
        }
    }

    /// Test-only: mark a slot as Draining if it has been idle
    /// since `threshold`. The current implementation leaves the
    /// eviction tick to the scheduler; this helper is the
    /// production path used by the unit tests.
    #[allow(dead_code)]
    pub async fn mark_draining_if_idle(
        &self,
        tenant_id: &str,
        threshold: Instant,
    ) -> Option<RuntimePhase> {
        let mut map = self.map.lock().await;
        let slot = map.get(tenant_id)?;
        let guard = slot.try_lock();
        if let Ok(mut g) = guard
            && g.pin_count.load(Ordering::SeqCst) == 0
            && g.last_used <= threshold
        {
            g.phase = RuntimePhase::Draining;
            g.runtime = None;
            g.phase = RuntimePhase::Unloaded;
            return Some(RuntimePhase::Unloaded);
        }
        None
    }

    /// Test-only: True if the slot is in the Ready state and
    /// `runtime` is Some.
    #[allow(dead_code)]
    pub async fn contains_ready(&self, tenant_id: &str) -> bool {
        let mut map = self.map.lock().await;
        let Some(slot) = map.get(tenant_id) else {
            return false;
        };
        let Ok(g) = slot.try_lock() else {
            return false;
        };
        g.phase == RuntimePhase::Ready && g.runtime.is_some()
    }

    /// Test-only: activation count for a tenant, derived
    /// from the `ActivationSlot.generation` counter.
    #[allow(dead_code)]
    pub async fn activation_count(&self, tenant_id: &str) -> u64 {
        let map = self.map.lock().await;
        let Some(slot) = map.peek(tenant_id) else {
            return 0;
        };
        let Ok(g) = slot.try_lock() else {
            return 0;
        };
        g.activation.generation.load(Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use chrono::Utc;
    use surrealdb::Surreal;
    use surrealdb::engine::local::Mem;

    fn ready_tenant(id: &str, ns: &str) -> Tenant {
        Tenant {
            id: id.to_string(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: ns.to_string(),
                database: "memory".into(),
            },
            plan_version: 1,
            schema_version: 0,
            retry_stage: None,
            provisioning_lease: None,
            created_at: Utc::now(),
            version: 0,
        }
    }

    async fn test_pool() -> Arc<Pool> {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        Arc::new(Pool::with_defaults(registry))
    }

    #[tokio::test]
    async fn capacity_one_returns_capacity_timeout_when_pinned() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            1,
            Duration::ZERO,
            Duration::from_millis(20),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));
        let guard = pool
            .acquire_or_wait(&ready_tenant("ten_x", "tns_x"))
            .await
            .expect("first runtime acquires");
        let result = pool.acquire_or_wait(&ready_tenant("ten_y", "tns_y")).await;
        assert!(matches!(result, Err(PoolError::CapacityTimeout)));
        drop(guard);
    }

    #[tokio::test]
    async fn idle_runtime_is_evicted_to_recover_capacity() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            1,
            Duration::ZERO,
            Duration::from_millis(20),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));
        let guard = pool
            .acquire_or_wait(&ready_tenant("ten_x", "tns_x"))
            .await
            .expect("first runtime acquires");
        drop(guard);
        let second = pool
            .acquire_or_wait(&ready_tenant("ten_y", "tns_y"))
            .await
            .expect("idle runtime makes capacity available");
        assert!(pool.contains_ready("ten_y").await);
        drop(second);
    }

    #[tokio::test]
    async fn admission_gate_request_capacity() {
        let gate = Arc::new(AdmissionGate::new(1));
        let _p1 = gate.try_acquire().expect("first permit");
        let p2 = gate.try_acquire();
        assert!(p2.is_err());
    }

    #[tokio::test]
    async fn admission_gate_subscription_separate_budget() {
        let gate = Arc::new(AdmissionGate::new(1));
        let req_permit = gate.try_acquire().expect("request permit");
        let sub_permit = gate.try_acquire_for(true).expect("subscription permit");
        // Drop before the test ends so the permits are not
        // held when the next test case starts.
        drop(req_permit);
        drop(sub_permit);
    }

    #[tokio::test]
    async fn contains_ready_after_acquire() {
        let pool = test_pool().await;
        let tenant = ready_tenant("ten_y", "tns_y");
        let _g = pool.acquire_or_wait(&tenant).await.unwrap();
        assert!(pool.contains_ready("ten_y").await);
    }

    // ─── Pool contract tests ──────────────────────────────────────

    /// Pool test 1: 8 concurrent acquirers for the same
    /// tenant should single-flight into a single activation.
    #[tokio::test]
    async fn single_flight_activation_runs_once() {
        // Build a pool that counts activations. The fixture
        // uses the real in-memory engine; activation count is
        // measured by the `ActivationSlot.generation`
        // counter, which is bumped exactly once per (re)activation.
        let pool = test_pool().await;
        let tenant = ready_tenant("ten_single", "tns_single");
        let mut joins = tokio::task::JoinSet::new();
        for _ in 0..8 {
            let pool = pool.clone();
            let tenant = tenant.clone();
            joins.spawn(async move { pool.acquire_or_wait(&tenant).await });
        }
        let mut ok = 0;
        while let Some(r) = joins.join_next().await {
            assert!(r.expect("task").is_ok());
            ok += 1;
        }
        assert_eq!(ok, 8);
        // ActivationSlot.generation was bumped once for the
        // first arriver; subsequent acquirers subscribed to
        // the in-flight channel and did not trigger a new
        // activation. The generation counter therefore stays
        // at 1.
        assert_eq!(pool.activation_count("ten_single").await, 1);
    }

    /// Pool test 2: a pinned runtime is not evicted by
    /// the idle-eviction path. The pin holds the slot
    /// until the guard is dropped.
    #[tokio::test]
    async fn pinned_runtime_is_not_evicted() {
        let pool = test_pool().await;
        let tenant = ready_tenant("ten_pinned", "tns_pinned");
        let guard = pool.acquire_or_wait(&tenant).await.expect("acquire");
        let pin_counter = guard.pin_counter();
        assert_eq!(pin_counter.load(Ordering::SeqCst), 1);
        // mark_draining_if_idle evicts only if pin_count == 0.
        // With the guard held, pin_count is 1, so the call
        // must return None and the slot must remain Ready.
        let threshold = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        let result = pool.mark_draining_if_idle("ten_pinned", threshold).await;
        assert!(result.is_none(), "pinned runtime must not be evicted");
        assert!(pool.contains_ready("ten_pinned").await);
        drop(guard);
        assert_eq!(pin_counter.load(Ordering::SeqCst), 0);
    }

    /// Pool test 3: the response body holds the pin and
    /// the admission permit until it is dropped. The test does
    /// not need a real OperationGuard; it directly checks
    /// the AdmissionPermit RAII lifecycle through
    /// `ResponseLease` + `LeasedBody`.
    #[tokio::test]
    async fn response_body_keeps_pin_and_global_admission_until_drop() {
        use crate::http::runtime::guard::{LeasedBody, ResponseLease};
        use http_body_util::BodyExt;
        // 1 global permit; we hold it via the lease.
        let gate = Arc::new(AdmissionGate::new(1));
        let permit = gate.try_acquire().expect("first permit");
        let lease = ResponseLease::new(None, Arc::new(permit));
        let inner = axum::body::Body::from("hello");
        let body = LeasedBody::new(inner, lease);
        // Drive the body to completion by collecting it; the
        // lease must release on drop (terminal frame).
        let _ = body.collect().await;
        // After collect the body is dropped, the lease is
        // released, and the permit becomes available again.
        assert!(
            gate.try_acquire().is_ok(),
            "permit must be released after body collect"
        );
    }

    /// Pool test 4: capacity overflow returns
    /// `PoolError::CapacityTimeout`; the HTTP middleware
    /// maps that to 503.
    #[tokio::test]
    async fn capacity_overflow_returns_503() {
        // cap=1 means the second distinct tenant cannot
        // acquire a slot while the first is pinned.
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            1,
            DEFAULT_IDLE_TTL,
            // Short wait so the test does not block.
            std::time::Duration::from_millis(50),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));
        let t1 = ready_tenant("ten_cap_1", "tns_cap_1");
        let t2 = ready_tenant("ten_cap_2", "tns_cap_2");
        let _first = pool.acquire_or_wait(&t1).await.expect("first acquire");
        let r = pool.acquire_or_wait(&t2).await;
        assert!(
            matches!(r, Err(PoolError::CapacityTimeout)),
            "second acquire must time out"
        );
    }

    /// Pool test 5: a failed activation puts the slot in
    /// negative backoff; subsequent acquirers see
    /// `ActivationFailed` without re-attempting.
    #[tokio::test]
    async fn negative_cache_swallows_repeated_failures() {
        // Use a registry whose engine init fails so
        // `build_runtime` fails. The simplest path: use a
        // pool whose registry was built with a
        // `RegistryHandle::new()` placeholder; that returns
        // `Storage("no engine")` error from `tenant_engine()`,
        // which `build_runtime` propagates as `MemoryError`.
        let pool = Arc::new(Pool::with_defaults(Arc::new(RegistryHandle::new())));
        let tenant = ready_tenant("ten_neg", "tns_neg");
        let r1 = pool.acquire_or_wait(&tenant).await;
        assert!(matches!(r1, Err(PoolError::ActivationFailed)));
        let r2 = pool.acquire_or_wait(&tenant).await;
        assert!(matches!(r2, Err(PoolError::ActivationFailed)));
        // activation_count stays at 1 because the negative
        // cache short-circuited the second call.
        assert_eq!(pool.activation_count("ten_neg").await, 1);
    }
}
