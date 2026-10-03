//! Runtime pool + admission gate.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use tokio::sync::OwnedSemaphorePermit;
use tokio::sync::Semaphore;

use lru::LruCache;

use crate::error::MemoryError;
use crate::http::registry::models::Tenant;
use crate::http::shutdown::ShutdownState;
use crate::tenancy::api::{
    RuntimeFactoryError, TenantRuntimeFactory, TenantRuntimeIdentity, TenantRuntimeSpec,
};

use crate::http::runtime::guard::{OperationGuard, SlotReservation};
use crate::http::runtime::lifecycle::{RuntimeRevision, SlotState, TenantRuntimeSlot};
use crate::http::runtime::storage::TenantRuntime;

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
        Self::new_with_limits(global_limit, 32)
    }

    pub fn new_with_limits(global_limit: u32, subscription_limit: u32) -> Self {
        Self {
            global_limit: global_limit.max(1),
            global_active: AtomicU32::new(0),
            subscription_limit: subscription_limit.max(1),
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

// Pool defaults live in `crate::http::config` so the 12-factor
// environment loader, the spec-default pool, and the public
// re-exports below share a single source of truth.
pub use crate::http::config::{
    DEFAULT_POOL_CAP, DEFAULT_RUNTIME_ACTIVATION_TIMEOUT as DEFAULT_ACTIVATION_TIMEOUT,
    DEFAULT_RUNTIME_CAPACITY_WAIT as DEFAULT_CAPACITY_WAIT,
    DEFAULT_RUNTIME_IDLE_TTL as DEFAULT_IDLE_TTL,
};
pub use crate::http::registry::models::DEFAULT_PER_TENANT_REQUEST_CONCURRENCY as DEFAULT_PER_TENANT_CONCURRENCY;

/// What one pass over the slot map decided the caller should do.
enum Decision {
    /// The slot holds a runtime this caller may use.
    Serve {
        runtime: Arc<TenantRuntime>,
        semaphore: Arc<Semaphore>,
        reservation: SlotReservation,
    },
    /// Wait for the attempt already running.
    Follow(
        tokio::sync::watch::Receiver<Option<crate::http::runtime::lifecycle::ActivationOutcome>>,
    ),
    /// This caller starts the attempt.
    Lead { generation: u64 },
    /// A recent attempt failed; refuse until its backoff expires.
    Backoff,
    /// Different storage is bound to this tenant id. Never served or replaced.
    ForeignBinding,
    /// No slot free and nothing reclaimable.
    NoRoom,
}

/// LRU pool of Tenant Runtimes, and the only owner of their lifecycle.
///
/// One map, one state machine, one activation timeout. It used to be two: the
/// pool kept slots while `tenancy::api` kept its own runtime cache and
/// activation registry with a second capacity limit and a second timeout. Two
/// owners had to be kept in step, and a request that disappeared mid-activation
/// left both of them holding a producer that no longer existed.
///
/// Acquisition is cancellation-safe by construction: the caller that starts an
/// attempt owns it, the attempt's guard releases the slot when the future is
/// dropped, and followers observe that terminal outcome instead of waiting.
pub struct Pool {
    /// Slot bookkeeping. Held only for short synchronous sections: no guard
    /// lives across an await, and runtimes removed here are dropped after the
    /// lock is released.
    state: Mutex<LruCache<String, TenantRuntimeSlot>>,
    /// Woken whenever a slot changes state, so a capacity wait re-checks.
    waiters: tokio::sync::Notify,
    cap: usize,
    // Read by `evict_idle` and the tracked scheduler job.
    idle_ttl: Duration,
    capacity_wait: Duration,
    // Enforced by `acquire_spec_with_limit` around one factory activation.
    activation_timeout: Duration,
    // Bound for concurrent in-flight requests against a single tenant runtime.
    per_tenant_concurrency: u32,
    factory: Arc<dyn TenantRuntimeFactory<Runtime = TenantRuntime>>,
    shutdown: ShutdownState,
}

/// One activation attempt, owned by the request that started it.
///
/// `Drop` runs when that request is cancelled — a deadline expiring, a client
/// disconnecting — and it is why cancellation cannot strand a tenant. It is
/// synchronous on purpose: cleanup must not depend on another task being
/// scheduled, because the whole failure this closes is work that was left for a
/// task that never ran.
struct ActivationAttempt {
    pool: Arc<Pool>,
    tenant_id: String,
    generation: u64,
    completed: bool,
}

impl ActivationAttempt {
    fn complete(
        &mut self,
        outcome: Result<Arc<TenantRuntime>, MemoryError>,
    ) -> Result<Arc<TenantRuntime>, MemoryError> {
        self.completed = true;
        {
            let mut map = self.pool.lock_state();
            if let Some(slot) = map.get_mut(&self.tenant_id) {
                slot.finish_attempt(self.generation, outcome.clone(), Instant::now());
            }
        }
        self.pool.waiters.notify_waiters();
        outcome
    }
}

impl Drop for ActivationAttempt {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        // The request that started this attempt is gone. Return the slot to
        // `Absent` — retryable, and deliberately without a failure backoff: the
        // activation did not fail, nobody waited for its answer.
        {
            let mut map = self.pool.lock_state();
            if let Some(slot) = map.get_mut(&self.tenant_id) {
                slot.abandon(self.generation);
            }
        }
        self.pool.waiters.notify_waiters();
    }
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
        Self::with_factory(
            cap,
            idle_ttl,
            capacity_wait,
            activation_timeout,
            per_tenant_concurrency,
            Arc::new(
                crate::bootstrap::integration::tenancy_runtime::RegistryTenantRuntimeFactory::new(
                    registry,
                    crate::http::runtime::storage::RuntimeOptions::default(),
                ),
            ),
            ShutdownState::new(),
        )
    }

    /// Build a pool over an explicit factory and shutdown signal.
    ///
    /// The factory seam has two adapters — the registry-backed one in
    /// production and a blocking or failing one in tests — which is what makes
    /// activation behaviour testable without a live control plane. The shutdown
    /// signal is the instance's, so acquisition stops when the server does.
    pub(crate) fn with_factory(
        cap: usize,
        idle_ttl: Duration,
        capacity_wait: Duration,
        activation_timeout: Duration,
        per_tenant_concurrency: u32,
        factory: Arc<dyn TenantRuntimeFactory<Runtime = TenantRuntime>>,
        shutdown: ShutdownState,
    ) -> Self {
        let cap = cap.max(1);
        Self {
            state: Mutex::new(LruCache::new(
                std::num::NonZeroUsize::new(cap).unwrap_or(std::num::NonZeroUsize::MIN),
            )),
            waiters: tokio::sync::Notify::new(),
            cap,
            idle_ttl,
            capacity_wait,
            activation_timeout,
            per_tenant_concurrency,
            factory,
            shutdown,
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

    /// Build the runtime pool from the validated HTTP environment contract.
    pub fn from_http_config(
        config: &crate::http::config::HttpConfig,
        registry: Arc<crate::http::registry::RegistryHandle>,
    ) -> Self {
        Self::from_http_config_with_shutdown(config, registry, ShutdownState::new())
    }

    /// As [`Pool::from_http_config`], sharing the instance's shutdown signal.
    pub(crate) fn from_http_config_with_shutdown(
        config: &crate::http::config::HttpConfig,
        registry: Arc<crate::http::registry::RegistryHandle>,
        shutdown: ShutdownState,
    ) -> Self {
        let per_tenant_concurrency = config
            .signup_plan_limits
            .as_ref()
            .map_or(DEFAULT_PER_TENANT_CONCURRENCY, |limits| {
                limits.per_tenant_request_concurrency
            });
        Self::with_factory(
            config.pool_cap,
            config.runtime_idle_ttl,
            config.runtime_capacity_wait,
            config.runtime_activation_timeout,
            per_tenant_concurrency,
            Arc::new(
                crate::bootstrap::integration::tenancy_runtime::RegistryTenantRuntimeFactory::new(
                    registry,
                    crate::http::runtime::storage::RuntimeOptions::from_http_config(config),
                ),
            ),
            shutdown,
        )
    }

    /// The configured maximum number of concurrently resident runtimes.
    ///
    /// No caller reads this today: `/health/ready` and the metrics
    /// report the configured value from `HttpConfig` rather than from
    /// the pool, so this is the pool's own view of its bound and is
    /// kept as the place a caller would read it from.
    pub fn capacity(&self) -> usize {
        self.cap
    }

    /// Take the slot bookkeeping lock.
    ///
    /// Poisoning is ignored rather than propagated: every critical section here
    /// is a few field writes with no fallible step, so a panic elsewhere cannot
    /// leave the map inconsistent, and refusing every later request because an
    /// unrelated task panicked would turn one failure into an outage.
    fn lock_state(&self) -> MutexGuard<'_, LruCache<String, TenantRuntimeSlot>> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn revision(&self, spec: &TenantRuntimeSpec, per_tenant_concurrency: u32) -> RuntimeRevision {
        RuntimeRevision {
            plan_version: spec.plan_version,
            schema_version: spec.schema_version,
            concurrency: per_tenant_concurrency.max(1),
        }
    }

    /// Decide what the caller should do, mutating the map where that is the
    /// decision (starting an attempt, reclaiming room).
    fn decide(
        &self,
        map: &mut LruCache<String, TenantRuntimeSlot>,
        identity: &TenantRuntimeIdentity,
        revision: RuntimeRevision,
        now: Instant,
    ) -> Decision {
        if let Some(slot) = map.get_mut(&identity.tenant_id) {
            if &slot.identity != identity {
                return Decision::ForeignBinding;
            }
            if let Some(runtime) = slot.ready_runtime() {
                if slot.revision == revision {
                    let runtime = Arc::clone(runtime);
                    let semaphore = Arc::clone(&slot.concurrency);
                    let reservation = SlotReservation::acquire(&slot.pins);
                    slot.last_used = now;
                    return Decision::Serve {
                        runtime,
                        semaphore,
                        reservation,
                    };
                }
                // Same binding, different runtime inputs: the resident runtime
                // is stale, not wrong. Unload it and build the requested one
                // below. Guards still holding the old `Arc` finish normally; no
                // new caller is served from it.
                slot.state = SlotState::Absent;
                slot.completion = None;
            } else if slot.in_negative_backoff(now) && slot.revision == revision {
                // A refusal belongs to the revision that failed. A changed
                // revision is a different runtime and gets its own attempt.
                return Decision::Backoff;
            }

            // An attempt already in flight is waited on rather than replaced,
            // whatever revision it was started for: the caller that owns it will
            // finish, and this one re-decides afterwards.
            if matches!(slot.state, SlotState::Loading)
                && let Some(receiver) = slot.subscribe()
            {
                return Decision::Follow(receiver);
            }

            slot.revision = revision;
            slot.concurrency = Arc::new(Semaphore::new(revision.concurrency.max(1) as usize));
            let (generation, _receiver) = slot.begin_loading();
            return Decision::Lead { generation };
        }

        if map.len() >= self.cap {
            let victim = map
                .iter()
                .find(|(_, slot)| slot.is_reclaimable(now, self.idle_ttl))
                .map(|(tenant_id, _)| tenant_id.clone());
            match victim {
                // Removing before inserting matters: `LruCache::put` evicts the
                // least-recently-used entry when the map is full, and that entry
                // may be pinned or mid-activation.
                Some(victim) => {
                    map.pop(&victim);
                }
                None => return Decision::NoRoom,
            }
        }
        map.put(
            identity.tenant_id.clone(),
            TenantRuntimeSlot::new(identity.clone(), revision),
        );
        let Some(slot) = map.get_mut(&identity.tenant_id) else {
            // Unreachable: the entry was inserted immediately above.
            return Decision::NoRoom;
        };
        let (generation, _receiver) = slot.begin_loading();
        Decision::Lead { generation }
    }

    /// Remove idle, unpinned Ready runtimes. Dropping the last runtime
    /// handle closes its tenant-bound stores; no data or Registry row is removed.
    pub async fn evict_idle(&self) -> usize {
        let now = Instant::now();
        let mut unloaded = Vec::new();
        {
            let mut map = self.lock_state();
            let candidates: Vec<String> = map
                .iter()
                .filter(|(_, slot)| slot.is_reclaimable(now, self.idle_ttl))
                .map(|(tenant_id, _)| tenant_id.clone())
                .collect();
            for tenant_id in candidates {
                if let Some(mut slot) = map.pop(&tenant_id)
                    && let SlotState::Ready { runtime } =
                        std::mem::replace(&mut slot.state, SlotState::Absent)
                {
                    unloaded.push(runtime);
                }
            }
        }
        // Runtimes are dropped here, outside the lock: closing a tenant's stores
        // can block, and nothing else may be waiting on bookkeeping.
        let evicted = unloaded.len();
        drop(unloaded);
        if evicted > 0 {
            self.waiters.notify_waiters();
        }
        evicted
    }

    /// Tracked scheduler job for idle runtime eviction.
    pub fn eviction_scheduler_job(pool: Arc<Self>) -> crate::http::leases::scheduler::SchedulerJob {
        Arc::new(move |_registry| {
            let pool = pool.clone();
            Box::pin(async move {
                let _ = pool.evict_idle().await;
                Ok(())
            })
        })
    }

    async fn acquire_tenant_permit(
        &self,
        semaphore: Arc<Semaphore>,
        deadline: Instant,
    ) -> Result<OwnedSemaphorePermit, PoolError> {
        let shutdown = self.shutdown.token();
        tokio::select! {
            _ = shutdown.cancelled() => Err(PoolError::ShuttingDown),
            acquired = tokio::time::timeout_at(deadline.into(), semaphore.acquire_owned()) => {
                match acquired {
                    Ok(Ok(permit)) => Ok(permit),
                    // The semaphore is closed only when the slot is gone.
                    Ok(Err(_)) => Err(PoolError::ShuttingDown),
                    Err(_) => Err(PoolError::CapacityTimeout),
                }
            }
        }
    }

    /// Acquire a runtime for the given tenant.
    pub async fn acquire_or_wait(
        self: &Arc<Self>,
        tenant: &Tenant,
    ) -> Result<OperationGuard, PoolError> {
        self.acquire_or_wait_with_limit(tenant, self.per_tenant_concurrency)
            .await
    }

    /// Acquire using the tenant's durable plan concurrency limit. Existing
    /// callers retain `acquire_or_wait`; HTTP passes the loaded plan here so a
    /// plan change cannot be replaced by the process default on cold activation.
    pub async fn acquire_or_wait_with_limit(
        self: &Arc<Self>,
        tenant: &Tenant,
        per_tenant_concurrency: u32,
    ) -> Result<OperationGuard, PoolError> {
        self.acquire_spec_with_limit(
            &crate::tenancy::api::TenantRuntimeSpec {
                tenant_id: tenant.id.clone(),
                namespace: tenant.namespace_binding.namespace.clone(),
                database: tenant.namespace_binding.database.clone(),
                plan_version: tenant.plan_version,
                schema_version: tenant.schema_version,
                // A caller that reached this method already proved the tenant
                // is `Ready`; the request path refuses every other status
                // before it gets here. A maintenance caller wanting to bind a
                // deleting tenant goes through `acquire_spec_with_limit` with
                // a spec that states the status itself.
                status: crate::tenancy::api::TenantLifecycleStatus::Ready,
            },
            per_tenant_concurrency,
        )
        .await
    }

    pub async fn acquire_spec_with_limit(
        self: &Arc<Self>,
        spec: &TenantRuntimeSpec,
        per_tenant_concurrency: u32,
    ) -> Result<OperationGuard, PoolError> {
        let identity = spec.identity();
        let revision = self.revision(spec, per_tenant_concurrency);
        let deadline = Instant::now() + self.capacity_wait;

        loop {
            if self.shutdown.is_shutting_down() {
                return Err(PoolError::ShuttingDown);
            }
            // Registered before the decision, so a release that lands between
            // the check and the wait still wakes this caller.
            let notified = self.waiters.notified();
            let decision = {
                let mut map = self.lock_state();
                self.decide(&mut map, &identity, revision, Instant::now())
            };

            match decision {
                Decision::Serve {
                    runtime,
                    semaphore,
                    reservation,
                } => {
                    let permit = self.acquire_tenant_permit(semaphore, deadline).await?;
                    return Ok(reservation.into_guard(runtime, permit));
                }
                Decision::Backoff => return Err(PoolError::ActivationFailed),
                Decision::ForeignBinding => {
                    crate::http::logging::log_warn(
                        "http.runtime.binding_conflict",
                        &format!(
                            "tenant {} is resident for a different namespace or database",
                            identity.tenant_id
                        ),
                    );
                    return Err(PoolError::ActivationFailed);
                }
                Decision::Follow(mut receiver) => {
                    let shutdown = self.shutdown.token();
                    let outcome = tokio::select! {
                        _ = shutdown.cancelled() => return Err(PoolError::ShuttingDown),
                        changed = receiver.wait_for(|value| value.is_some()) => match changed {
                            Ok(value) => value.clone().unwrap_or_else(|| {
                                Err(MemoryError::Unavailable(
                                    "tenant runtime activation reported no outcome".into(),
                                ))
                            }),
                            // The attempt's guard dropped the sender without an
                            // answer: the request that owned it went away.
                            Err(_) => Err(MemoryError::Unavailable(
                                "tenant runtime activation was abandoned".into(),
                            )),
                        },
                    };
                    match outcome {
                        Ok(()) => continue,
                        Err(_) => return Err(PoolError::ActivationFailed),
                    }
                }
                Decision::Lead { generation } => {
                    let mut attempt = ActivationAttempt {
                        pool: Arc::clone(self),
                        tenant_id: identity.tenant_id.clone(),
                        generation,
                        completed: false,
                    };
                    let shutdown = self.shutdown.token();
                    let activation = tokio::select! {
                        _ = shutdown.cancelled() => {
                            // The guard is dropped with the attempt, returning
                            // the slot to `Absent` rather than leaving a
                            // producer behind.
                            return Err(PoolError::ShuttingDown);
                        }
                        activation = tokio::time::timeout(
                            self.activation_timeout,
                            self.factory.activate(spec.clone()),
                        ) => match activation {
                            Ok(Ok(runtime)) => Ok(Arc::new(runtime)),
                            Ok(Err(RuntimeFactoryError::Storage(error))) => Err(error),
                            Err(_) => Err(MemoryError::Unavailable(
                                "tenant runtime activation timed out".into(),
                            )),
                        },
                    };
                    match attempt.complete(activation) {
                        Ok(_) => continue,
                        Err(error) => {
                            crate::http::logging::log_warn(
                                "http.runtime.activation_failed",
                                &format!("tenant {}: {error}", identity.tenant_id),
                            );
                            return Err(PoolError::ActivationFailed);
                        }
                    }
                }
                Decision::NoRoom => {
                    if Instant::now() >= deadline {
                        return Err(PoolError::CapacityTimeout);
                    }
                    let shutdown = self.shutdown.token();
                    tokio::select! {
                        _ = notified => {}
                        _ = shutdown.cancelled() => return Err(PoolError::ShuttingDown),
                        _ = tokio::time::sleep_until(deadline.into()) => {
                            return Err(PoolError::CapacityTimeout);
                        }
                    }
                }
            }
        }
    }

    /// Test-only: unload a slot that has been idle since `threshold`.
    ///
    /// The production eviction tick is driven by the scheduler; this exposes the
    /// same eligibility rule to the unit tests, which are its only callers.
    pub async fn mark_draining_if_idle(&self, tenant_id: &str, threshold: Instant) -> bool {
        let now = Instant::now();
        let mut map = self.lock_state();
        let Some(slot) = map.get_mut(tenant_id) else {
            return false;
        };
        if slot.pins.load(Ordering::SeqCst) > 0 || slot.last_used > threshold {
            return false;
        }
        if slot.ready_runtime().is_none() {
            return false;
        }
        slot.state = SlotState::Absent;
        slot.last_used = now;
        drop(map);
        self.waiters.notify_waiters();
        true
    }

    /// Test-only: True if the slot is ready with a resident runtime.
    pub async fn contains_ready(&self, tenant_id: &str) -> bool {
        let map = self.lock_state();
        map.peek(tenant_id)
            .map(|slot| slot.ready_runtime().is_some())
            .unwrap_or(false)
    }

    /// Test-only: how many activation attempts this tenant has made.
    pub async fn activation_count(&self, tenant_id: &str) -> u64 {
        let map = self.lock_state();
        map.peek(tenant_id).map(|slot| slot.generation).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use crate::http::registry::storage::InMemoryStore;
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

    fn spec(tenant_id: &str, namespace: &str) -> crate::tenancy::api::TenantRuntimeSpec {
        crate::tenancy::api::TenantRuntimeSpec {
            tenant_id: tenant_id.to_string(),
            namespace: namespace.to_string(),
            database: "memory".to_string(),
            plan_version: 1,
            schema_version: 0,
            status: crate::tenancy::api::TenantLifecycleStatus::Ready,
        }
    }

    /// A factory that can be held inside `activate`, so a test can observe a
    /// cold activation while it is in flight. Semaphores rather than
    /// notifications: a permit set before the test waits is not missed.
    struct BlockingFactory {
        inner: crate::bootstrap::integration::tenancy_runtime::RegistryTenantRuntimeFactory,
        entered: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
        calls: Arc<std::sync::atomic::AtomicU64>,
        panic_next: Arc<std::sync::atomic::AtomicBool>,
    }

    #[async_trait::async_trait]
    impl crate::tenancy::api::TenantRuntimeFactory for BlockingFactory {
        type Runtime = crate::http::runtime::storage::TenantRuntime;

        async fn activate(
            &self,
            spec: crate::tenancy::api::TenantRuntimeSpec,
        ) -> Result<Self::Runtime, crate::tenancy::api::RuntimeFactoryError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.entered.add_permits(1);
            if self.panic_next.swap(false, Ordering::SeqCst) {
                panic!("factory panicked on purpose");
            }
            let permit = self.release.acquire().await.map_err(|_| {
                crate::tenancy::api::RuntimeFactoryError::Storage(
                    crate::error::MemoryError::Unavailable("factory released".into()),
                )
            })?;
            permit.forget();
            crate::tenancy::api::TenantRuntimeFactory::activate(&self.inner, spec).await
        }
    }

    /// Handles onto a [`BlockingFactory`], so a test can hold an activation
    /// inside the factory and observe what the pool does meanwhile.
    struct BlockingHarness {
        entered: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
        calls: Arc<std::sync::atomic::AtomicU64>,
        panic_next: Arc<std::sync::atomic::AtomicBool>,
    }

    impl BlockingHarness {
        /// Wait until one factory call has started.
        async fn wait_until_entered(&self) {
            self.entered
                .acquire()
                .await
                .expect("an activation must enter the factory")
                .forget();
        }

        /// Let one held activation build its runtime.
        fn release_one(&self) {
            self.release.add_permits(1);
        }

        fn calls(&self) -> u64 {
            self.calls.load(Ordering::SeqCst)
        }
    }

    fn blocking_factory(registry: Arc<RegistryHandle>) -> (Arc<BlockingFactory>, BlockingHarness) {
        let entered = Arc::new(tokio::sync::Semaphore::new(0));
        let release = Arc::new(tokio::sync::Semaphore::new(0));
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let panic_next = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let factory = Arc::new(BlockingFactory {
            inner:
                crate::bootstrap::integration::tenancy_runtime::RegistryTenantRuntimeFactory::new(
                    registry,
                    crate::http::runtime::storage::RuntimeOptions::default(),
                ),
            entered: Arc::clone(&entered),
            release: Arc::clone(&release),
            calls: Arc::clone(&calls),
            panic_next: Arc::clone(&panic_next),
        });
        (
            factory,
            BlockingHarness {
                entered,
                release,
                calls,
                panic_next,
            },
        )
    }

    async fn mem_registry() -> Arc<RegistryHandle> {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)))
    }

    async fn pool_over_blocking_factory(cap: usize) -> (Arc<Pool>, BlockingHarness) {
        let (factory, harness) = blocking_factory(mem_registry().await);
        let pool = Arc::new(Pool::with_factory(
            cap,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
            factory,
            crate::http::shutdown::ShutdownState::new(),
        ));
        (pool, harness)
    }

    /// The request that started an activation is the one that owns it. When that
    /// request disappears — a deadline expiring, a client disconnecting — the
    /// attempt must be released rather than left behind for a task that never
    /// runs again. Without the attempt guard this test hangs or reports a stale
    /// attempt count.
    #[tokio::test]
    async fn cancelled_activation_leader_allows_retry() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_cancel", "tns_cancel");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;
        leader.abort();
        let _ = leader.await;

        harness.release_one();
        let retry = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire_spec_with_limit(&tenant, 4),
        )
        .await
        .expect("a retry must terminate rather than wait on a dead producer");
        assert!(retry.is_ok(), "a cancelled attempt must be retryable");
        assert_eq!(harness.calls(), 2, "the retry must activate");
    }

    /// A caller waiting on someone else's activation must be told when that
    /// activation is abandoned. Otherwise it waits for a producer that no longer
    /// exists, which is the way a tenant becomes unreachable without a restart.
    #[tokio::test]
    async fn cancelled_activation_notifies_existing_followers() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_follow", "tns_follow");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;
        let follower = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        leader.abort();
        let _ = leader.await;

        let followed = tokio::time::timeout(Duration::from_secs(5), follower)
            .await
            .expect("the follower must terminate, not wait forever")
            .expect("follower task joins");
        assert!(followed.is_err(), "an abandoned attempt is not a success");
    }

    /// Cancelling a follower must not cancel the shared activation: another
    /// caller is waiting for that runtime.
    #[tokio::test]
    async fn cancelling_a_follower_does_not_cancel_activation() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_follower_cancel", "tns_follower_cancel");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;
        let follower = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        follower.abort();
        let _ = follower.await;
        harness.release_one();

        let led = tokio::time::timeout(Duration::from_secs(5), leader)
            .await
            .expect("the leader must still finish")
            .expect("leader task joins");
        assert!(led.is_ok(), "a follower's cancellation is not the leader's");
    }

    /// An abandoned attempt must not hold the pool hostage: the slot it left
    /// behind is reclaimable, so the next tenant can still be served.
    #[tokio::test]
    async fn cancelled_slots_do_not_exhaust_capacity() {
        let (pool, harness) = pool_over_blocking_factory(1).await;
        let abandoned = spec("ten_abandoned", "tns_abandoned");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = abandoned.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;
        leader.abort();
        let _ = leader.await;
        harness.release_one();

        let other = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire_or_wait(&ready_tenant("ten_next", "tns_next")),
        )
        .await
        .expect("capacity must recover from an abandoned attempt");
        assert!(other.is_ok(), "an abandoned slot must not hold the pool");
    }

    /// A factory that panics must leave the tenant retryable. The unwinding
    /// runs the attempt's guard, which is the entire point of putting cleanup in
    /// `Drop` rather than after the await.
    #[tokio::test]
    async fn factory_panic_does_not_poison_activation() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        harness.panic_next.store(true, Ordering::SeqCst);
        let tenant = spec("ten_panic", "tns_panic");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        let panicked = match leader.await {
            Ok(_) => panic!("a panicking factory must fail the leader task"),
            Err(error) => error,
        };
        assert!(panicked.is_panic(), "the leader must report the panic");

        harness.release_one();
        let retry = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire_spec_with_limit(&tenant, 4),
        )
        .await
        .expect("a retry after a panic must terminate");
        assert!(
            retry.is_ok(),
            "a panicking factory must not poison the tenant"
        );
        assert_eq!(harness.calls(), 2);
    }

    /// A runtime a caller is waiting to use must not be unloaded underneath it.
    #[tokio::test]
    async fn runtime_is_pinned_before_waiting_for_tenant_permit() {
        let pool = test_pool().await;
        let tenant = ready_tenant("ten_pin_wait", "tns_pin_wait");
        let held = pool.acquire_or_wait(&tenant).await.expect("first guard");

        // The second caller is parked on the tenant's concurrency limit, having
        // already chosen the runtime it will use.
        let waiting = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_or_wait(&tenant).await }
        });
        tokio::time::sleep(Duration::from_millis(50)).await;

        assert_eq!(
            pool.evict_idle().await,
            0,
            "a runtime a caller is about to use is not idle"
        );
        assert!(
            pool.contains_ready("ten_pin_wait").await,
            "the runtime must survive the eviction sweep"
        );

        drop(held);
        let second = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiter must acquire once the permit is free")
            .expect("task joins");
        assert!(second.is_ok());
    }

    /// A tenant id bound to different storage is refused, not silently served
    /// from the runtime that belongs to another namespace.
    #[tokio::test]
    async fn binding_mismatch_is_rejected_while_ready() {
        let pool = test_pool().await;
        let _guard = pool
            .acquire_or_wait(&ready_tenant("ten_bind", "tns_bind_a"))
            .await
            .expect("first binding");

        let foreign = pool
            .acquire_or_wait(&ready_tenant("ten_bind", "tns_bind_b"))
            .await;
        assert!(
            foreign.is_err(),
            "a tenant bound to another namespace must not be served"
        );
        assert!(pool.contains_ready("ten_bind").await);
    }

    #[tokio::test]
    async fn binding_mismatch_is_rejected_while_loading() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let resident = spec("ten_bind_loading", "tns_bind_loading");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = resident.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;

        let foreign = pool
            .acquire_spec_with_limit(&spec("ten_bind_loading", "tns_other"), 4)
            .await;
        assert!(
            foreign.is_err(),
            "a loading slot must not be joined by a different binding"
        );

        harness.release_one();
        let led = tokio::time::timeout(Duration::from_secs(5), leader)
            .await
            .expect("leader terminates")
            .expect("task joins");
        assert!(led.is_ok(), "the original binding still activates");
    }

    /// Plan and schema describe the runtime revision, not the binding. A change
    /// makes the resident runtime stale, so it is replaced rather than reused or
    /// refused.
    #[tokio::test]
    async fn plan_change_replaces_the_runtime() {
        let pool = test_pool().await;
        let tenant = ready_tenant("ten_revision", "tns_revision");
        drop(pool.acquire_or_wait(&tenant).await.expect("first"));

        let mut changed = spec("ten_revision", "tns_revision");
        changed.plan_version = 2;
        drop(
            pool.acquire_spec_with_limit(&changed, 4)
                .await
                .expect("replaced runtime"),
        );

        assert_eq!(
            pool.activation_count("ten_revision").await,
            2,
            "a changed revision must be rebuilt, not reused"
        );
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

    /// `evict_idle` drops only Ready, unpinned slots whose `last_used`
    /// is older than `idle_ttl`. Pinned runtimes must be retained so
    /// in-flight requests can finish.
    #[tokio::test]
    async fn evict_idle_drops_only_idle_unpinned_slots() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            4,
            // ttl > 0 so freshly-pinned slots are not eligible.
            Duration::from_millis(50),
            Duration::from_millis(20),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));
        // Slot A: idle past TTL — eligible for eviction.
        let guard_a = pool
            .acquire_or_wait(&ready_tenant("ten_a", "tns_a"))
            .await
            .expect("acquire A");
        drop(guard_a);
        // Wait past the idle TTL.
        tokio::time::sleep(Duration::from_millis(80)).await;
        // Slot B: held by a guard — pin count > 0, must NOT be evicted.
        let _guard_b = pool
            .acquire_or_wait(&ready_tenant("ten_b", "tns_b"))
            .await
            .expect("acquire B");
        // Slot C: freshly activated, last_used == now — not eligible.
        let guard_c = pool
            .acquire_or_wait(&ready_tenant("ten_c", "tns_c"))
            .await
            .expect("acquire C");
        drop(guard_c);

        let evicted = pool.evict_idle().await;
        assert_eq!(evicted, 1, "only ten_a is idle and unpinned");
        assert!(!pool.contains_ready("ten_a").await);
        assert!(pool.contains_ready("ten_b").await);
        assert!(pool.contains_ready("ten_c").await);
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
        // mark_draining_if_idle unloads only if the slot is unpinned.
        // With the guard held, pin_count is 1, so the call
        // must return None and the slot must remain Ready.
        let threshold = std::time::Instant::now() + std::time::Duration::from_secs(3600);
        let result = pool.mark_draining_if_idle("ten_pinned", threshold).await;
        assert!(!result, "pinned runtime must not be evicted");
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
        // Use a registry without a privileged engine so
        // `build_runtime` fails through the explicit missing-engine
        // error, without constructing a production placeholder.
        let registry = RegistryHandle::from_store(Arc::new(InMemoryStore::default()));
        let pool = Arc::new(Pool::with_defaults(Arc::new(registry)));
        let tenant = ready_tenant("ten_neg", "tns_neg");
        let r1 = pool.acquire_or_wait(&tenant).await;
        assert!(matches!(r1, Err(PoolError::ActivationFailed)));
        let r2 = pool.acquire_or_wait(&tenant).await;
        assert!(matches!(r2, Err(PoolError::ActivationFailed)));
        // activation_count stays at 1 because the negative
        // cache short-circuited the second call.
        assert_eq!(pool.activation_count("ten_neg").await, 1);
    }

    /// Activation is never refused while the pool has capacity.
    ///
    /// The pool owns an LRU of runtime slots and separately consults
    /// `Tenancy`, which keeps its own resident-binding map. Both apply
    /// a capacity check, so a binding `Tenancy` retained for a tenant
    /// the pool had already evicted would eventually reject an
    /// activation the pool had made room for — degrading a live server
    /// to `CapacityTimeout` with nothing to explain it. Cycling many
    /// tenants through a two-slot pool makes that show up as a
    /// refused activation, which the loop below asserts never happens.
    #[tokio::test]
    async fn repeated_activation_cycles_within_capacity() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            2,
            Duration::ZERO,
            Duration::from_millis(50),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));

        // Cycle well past the capacity. With one owner there is no second
        // cache to fall behind, so every activation here must succeed.
        for round in 0..6 {
            for name in ["a", "b", "c"] {
                let tenant_id = format!("ten_{name}_{round}");
                let guard = pool
                    .acquire_or_wait(&ready_tenant(&tenant_id, &format!("tns_{name}")))
                    .await
                    .expect("cycling tenants must never exhaust the pool's capacity");
                drop(guard);
            }
        }

        // The LRU keeps only the most recently used slots, each of them Ready.
        // There is no second cache left behind to reject an activation for a
        // tenant the pool had already made room for.
        assert!(
            pool.contains_ready("ten_c_5").await,
            "the most recently activated tenant must still be resident"
        );
        assert_eq!(pool.capacity(), 2, "the pool's bound is unchanged");
    }
}
