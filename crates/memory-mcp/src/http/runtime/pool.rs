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
    /// An old revision is still in use; wait for its pins to drain.
    Drain,
    /// Release a runtime after the state lock has been dropped, then retry.
    Retire(Arc<TenantRuntime>),
    /// Release a reclaimed slot after the state lock has been dropped.
    Reclaim(TenantRuntimeSlot),
    /// No slot free and nothing reclaimable yet.
    NoRoom { next_expiry: Option<Instant> },
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
    waiters: Arc<tokio::sync::Notify>,
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

enum AttemptCompletion {
    Published(Result<Arc<TenantRuntime>, MemoryError>),
    Shutdown,
}

impl ActivationAttempt {
    fn complete(&mut self, outcome: Result<Arc<TenantRuntime>, MemoryError>) -> AttemptCompletion {
        self.completed = true;
        let completion = {
            let mut map = self.pool.lock_state();
            let published = self.pool.shutdown.publish_while_running(|| {
                if let Some(slot) = map.get_mut(&self.tenant_id) {
                    slot.finish_attempt(self.generation, outcome.clone(), Instant::now())
                } else {
                    false
                }
            });
            match published {
                None => {
                    if let Some(slot) = map.get_mut(&self.tenant_id) {
                        slot.abandon(self.generation);
                    }
                    AttemptCompletion::Shutdown
                }
                Some(true) => AttemptCompletion::Published(outcome),
                Some(false) => AttemptCompletion::Published(Err(MemoryError::Unavailable(
                    "tenant runtime activation was superseded".into(),
                ))),
            }
        };
        self.pool.waiters.notify_waiters();
        completion
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
            waiters: Arc::new(tokio::sync::Notify::new()),
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
        Self::from_http_config_with_shutdown(
            config,
            registry,
            ShutdownState::new(),
            None,
            Arc::new(crate::embedding::providers::task_runner::BackgroundTaskRunner::new()),
        )
    }

    /// As [`Pool::from_http_config`], sharing the instance's shutdown signal
    /// and carrying the deployment-level policy into every tenant runtime this
    /// pool activates.
    pub(crate) fn from_http_config_with_shutdown(
        config: &crate::http::config::HttpConfig,
        registry: Arc<crate::http::registry::RegistryHandle>,
        shutdown: ShutdownState,
        deployment_policy: Option<crate::http::runtime::bootstrap::DeploymentPolicy>,
        background_task_runner: Arc<crate::embedding::providers::task_runner::BackgroundTaskRunner>,
    ) -> Self {
        let per_tenant_concurrency = config
            .signup_plan_limits
            .as_ref()
            .map_or(DEFAULT_PER_TENANT_CONCURRENCY, |limits| {
                limits.per_tenant_request_concurrency
            });
        let mut options = crate::http::runtime::storage::RuntimeOptions::from_http_config(config)
            .with_background_task_runner(background_task_runner);
        if let Some(policy) = deployment_policy {
            options = options.with_cache_limits(policy.cache_limits);
            options = options.with_lifecycle_config(policy.lifecycle);
            options = options.with_query_logging(
                policy.query_logging_enabled,
                policy.query_log_retention_days,
            );
            options.embedding_similarity_threshold = policy.embedding_similarity_threshold;
            options.claim_config = policy.claim_config;
            if let Some(extractor) = policy.entity_extractor {
                options = options.with_entity_extractor(extractor);
            }
            if let Some(embedding) = policy.embedding {
                options = options.with_embedding_policy(embedding);
            }
        }
        Self::with_factory(
            config.pool_cap,
            config.runtime_idle_ttl,
            config.runtime_capacity_wait,
            config.runtime_activation_timeout,
            per_tenant_concurrency,
            Arc::new(
                crate::bootstrap::integration::tenancy_runtime::RegistryTenantRuntimeFactory::new(
                    registry, options,
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

    /// Clone the bounded set of currently resident runtimes.
    ///
    /// Loading and failed slots do not own runtimes. The pool lock is held only
    /// while these at-most-`cap` handles are cloned; callers may await owner
    /// snapshots after this method returns without blocking eviction.
    pub(crate) fn resident_runtime_handles(&self) -> Vec<Arc<TenantRuntime>> {
        self.lock_state()
            .iter()
            .filter_map(|(_, slot)| match &slot.state {
                SlotState::Ready { runtime } | SlotState::Draining { runtime } => {
                    Some(Arc::clone(runtime))
                }
                SlotState::Absent | SlotState::Loading | SlotState::Failed { .. } => None,
            })
            .collect()
    }

    /// Take the slot bookkeeping lock.  Poisoning is ignored rather than propagated: every critical section here is a few field writes with no fallible step, so a panic elsewhere cannot leave the map inconsistent, and refusing every later request because an unrelated task panicked would turn one failure into an outage.
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

    /// The next time an unpinned slot may become reclaimable without an
    /// external notification.
    fn next_reclaimable_at(
        &self,
        map: &LruCache<String, TenantRuntimeSlot>,
        now: Instant,
    ) -> Option<Instant> {
        map.iter()
            .filter(|(_, slot)| slot.pins.load(Ordering::SeqCst) == 0)
            .filter_map(|(_, slot)| match &slot.state {
                SlotState::Ready { .. } => slot
                    .last_used
                    .checked_add(self.idle_ttl)
                    .filter(|expiry| *expiry > now),
                SlotState::Failed { retry_at } if *retry_at > now => Some(*retry_at),
                _ => None,
            })
            .min()
    }

    fn no_room(&self, map: &LruCache<String, TenantRuntimeSlot>, now: Instant) -> Decision {
        Decision::NoRoom {
            next_expiry: self.next_reclaimable_at(map, now),
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
            if matches!(slot.state, SlotState::Loading) {
                if let Some(receiver) = slot.subscribe() {
                    return Decision::Follow(receiver);
                }
                return self.no_room(map, now);
            }
            if slot.revision == revision && slot.in_negative_backoff(now) {
                return Decision::Backoff;
            }
            match std::mem::replace(&mut slot.state, SlotState::Absent) {
                SlotState::Ready { runtime } => {
                    if slot.revision == revision {
                        slot.state = SlotState::Ready {
                            runtime: Arc::clone(&runtime),
                        };
                        let semaphore = Arc::clone(&slot.concurrency);
                        let reservation =
                            SlotReservation::acquire(&slot.pins, Arc::clone(&self.waiters));
                        slot.last_used = now;
                        return Decision::Serve {
                            runtime,
                            semaphore,
                            reservation,
                        };
                    }
                    // Stop admitting work to the old revision. Existing guards
                    // keep their runtime alive, while this slot blocks new
                    // activation until all those guards have released their pins.
                    slot.state = SlotState::Draining { runtime };
                    slot.revision = revision;
                    return if slot.pins.load(Ordering::SeqCst) == 0 {
                        self.retire_draining(slot)
                    } else {
                        Decision::Drain
                    };
                }
                SlotState::Draining { runtime } => {
                    slot.state = SlotState::Draining { runtime };
                    slot.revision = revision;
                    return if slot.pins.load(Ordering::SeqCst) == 0 {
                        self.retire_draining(slot)
                    } else {
                        Decision::Drain
                    };
                }
                SlotState::Failed { retry_at } if retry_at > now && slot.revision == revision => {
                    slot.state = SlotState::Failed { retry_at };
                    return Decision::Backoff;
                }
                SlotState::Absent | SlotState::Failed { .. } => {}
                SlotState::Loading => {
                    slot.state = SlotState::Loading;
                    return Decision::NoRoom { next_expiry: None };
                }
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
                    let Some(retired) = map.pop(&victim) else {
                        return self.no_room(map, now);
                    };
                    return Decision::Reclaim(retired);
                }
                None => return self.no_room(map, now),
            }
        }
        map.put(
            identity.tenant_id.clone(),
            TenantRuntimeSlot::new(identity.clone(), revision),
        );
        let Some(slot) = map.get_mut(&identity.tenant_id) else {
            // Unreachable: the entry was inserted immediately above.
            return self.no_room(map, now);
        };
        let (generation, _receiver) = slot.begin_loading();
        Decision::Lead { generation }
    }

    /// Move a drained runtime out of its slot and return it to the caller for
    /// destruction after the pool state lock is released.
    fn retire_draining(&self, slot: &mut TenantRuntimeSlot) -> Decision {
        match std::mem::replace(&mut slot.state, SlotState::Absent) {
            SlotState::Draining { runtime } => {
                slot.concurrency =
                    Arc::new(Semaphore::new(slot.revision.concurrency.max(1) as usize));
                Decision::Retire(runtime)
            }
            other => {
                slot.state = other;
                Decision::NoRoom { next_expiry: None }
            }
        }
    }

    /// Remove idle, unpinned Ready runtimes. Dropping the last runtime
    /// handle closes its tenant-bound stores; no data or Registry row is removed.
    pub async fn evict_idle(&self) -> usize {
        let now = Instant::now();
        let mut unloaded = Vec::new();
        let mut removed_any = false;
        {
            let mut map = self.lock_state();
            let candidates: Vec<String> = map
                .iter()
                .filter(|(_, slot)| slot.is_reclaimable(now, self.idle_ttl))
                .map(|(tenant_id, _)| tenant_id.clone())
                .collect();
            for tenant_id in candidates {
                if let Some(slot) = map.pop(&tenant_id) {
                    removed_any = true;
                    if let SlotState::Ready { runtime } = slot.state {
                        unloaded.push(runtime);
                    }
                }
            }
        }
        // Runtimes are dropped here, outside the lock: closing a tenant's stores
        // can block, and nothing else may be waiting on bookkeeping.
        let evicted = unloaded.len();
        drop(unloaded);
        if removed_any {
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
            biased;
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
        let capacity_deadline = Instant::now() + self.capacity_wait;

        loop {
            if self.shutdown.is_shutting_down() {
                return Err(PoolError::ShuttingDown);
            }
            // Registered before the decision, so a release that lands between
            // the check and the wait still wakes this caller.
            let notified = self.waiters.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
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
                    let permit = self
                        .acquire_tenant_permit(semaphore, Instant::now() + self.capacity_wait)
                        .await?;
                    // Acquiring a permit is not the final handoff: shutdown
                    // may begin after that await. Publish the lease through the
                    // same gate that fences runtime activation.
                    return self
                        .shutdown
                        .publish_while_running(|| reservation.into_guard(runtime, permit))
                        .ok_or(PoolError::ShuttingDown);
                }
                Decision::Backoff => return Err(PoolError::ActivationFailed),
                Decision::Drain => {
                    if Instant::now() >= capacity_deadline {
                        return Err(PoolError::CapacityTimeout);
                    }
                    let shutdown = self.shutdown.token();
                    tokio::select! {
                        _ = notified => {}
                        _ = shutdown.cancelled() => return Err(PoolError::ShuttingDown),
                        _ = tokio::time::sleep_until(capacity_deadline.into()) => {
                            return Err(PoolError::CapacityTimeout);
                        }
                    }
                }
                Decision::Retire(runtime) => {
                    // Runtime teardown closes tenant stores and may be slow.
                    // This branch runs after `decide` released the pool lock.
                    drop(runtime);
                    self.waiters.notify_waiters();
                }
                Decision::Reclaim(slot) => {
                    // The last runtime reference is dropped outside the global
                    // bookkeeping lock; an eviction never blocks other tenants.
                    drop(slot);
                    self.waiters.notify_waiters();
                }
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
                        biased;
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
                        AttemptCompletion::Published(Ok(_)) => continue,
                        AttemptCompletion::Shutdown => return Err(PoolError::ShuttingDown),
                        AttemptCompletion::Published(Err(error)) => {
                            crate::http::logging::log_warn(
                                "http.runtime.activation_failed",
                                &format!("tenant {}: {error}", identity.tenant_id),
                            );
                            return Err(PoolError::ActivationFailed);
                        }
                    }
                }
                Decision::NoRoom { next_expiry } => {
                    if Instant::now() >= capacity_deadline {
                        return Err(PoolError::CapacityTimeout);
                    }
                    let shutdown = self.shutdown.token();
                    let wake_at = next_expiry
                        .map_or(capacity_deadline, |expiry| expiry.min(capacity_deadline));
                    tokio::select! {
                        _ = notified => {}
                        _ = shutdown.cancelled() => return Err(PoolError::ShuttingDown),
                        _ = tokio::time::sleep_until(wake_at.into()) => {
                            if wake_at >= capacity_deadline {
                                return Err(PoolError::CapacityTimeout);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    // These pool scenarios use a real SurrealDB Mem registry and the production
    // runtime factory, with explicit gates around activation. Timer/deadline
    // cases are integration evidence even though Cargo places them in lib tests.
    use super::*;
    use crate::http::registry::RegistryHandle;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use chrono::Utc;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::Poll;
    use surrealdb::Surreal;
    use surrealdb::engine::local::Mem;
    use tower_service::Service;

    async fn acquire_for_deadline_test(
        axum::extract::State(state): axum::extract::State<Arc<crate::http::HttpState>>,
    ) -> axum::http::StatusCode {
        match state
            .pool
            .acquire_spec_with_limit(&spec("ten_deadline", "tns_deadline"), 4)
            .await
        {
            Ok(_guard) => axum::http::StatusCode::OK,
            Err(_) => axum::http::StatusCode::SERVICE_UNAVAILABLE,
        }
    }

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

    async fn assert_pending_once<F: Future>(mut future: Pin<&mut F>) {
        let observed = std::future::poll_fn(|cx| Poll::Ready(future.as_mut().poll(cx))).await;
        assert!(observed.is_pending(), "the acquisition must be waiting");
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
        fail_next: Arc<std::sync::atomic::AtomicBool>,
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
            let should_panic = self.panic_next.swap(false, Ordering::SeqCst);
            if self.fail_next.swap(false, Ordering::SeqCst) {
                return Err(crate::tenancy::api::RuntimeFactoryError::Storage(
                    crate::error::MemoryError::Unavailable("factory failed on purpose".into()),
                ));
            }
            let permit = self.release.acquire().await.map_err(|_| {
                crate::tenancy::api::RuntimeFactoryError::Storage(
                    crate::error::MemoryError::Unavailable("factory released".into()),
                )
            })?;
            permit.forget();
            if should_panic {
                panic!("factory panicked on purpose");
            }
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
        fail_next: Arc<std::sync::atomic::AtomicBool>,
        shutdown: crate::http::shutdown::ShutdownState,
    }

    impl BlockingHarness {
        /// Wait until one factory call has started.
        async fn wait_until_entered(&self) {
            self.wait_until_entered_within(Duration::from_secs(5)).await;
        }

        async fn wait_until_entered_within(&self, deadline: Duration) {
            tokio::time::timeout(deadline, self.entered.acquire())
                .await
                .expect("activation must enter within the test deadline")
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
        let fail_next = Arc::new(std::sync::atomic::AtomicBool::new(false));
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
            fail_next: Arc::clone(&fail_next),
        });
        (
            factory,
            BlockingHarness {
                entered,
                release,
                calls,
                panic_next,
                fail_next,
                shutdown: crate::http::shutdown::ShutdownState::new(),
            },
        )
    }

    async fn mem_registry() -> Arc<RegistryHandle> {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)))
    }

    async fn pool_over_blocking_factory(cap: usize) -> (Arc<Pool>, BlockingHarness) {
        pool_with_blocking_factory(
            cap,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await
    }

    async fn pool_with_blocking_factory(
        cap: usize,
        idle_ttl: Duration,
        capacity_wait: Duration,
        activation_timeout: Duration,
        per_tenant_concurrency: u32,
    ) -> (Arc<Pool>, BlockingHarness) {
        let (factory, harness) = blocking_factory(mem_registry().await);
        let shutdown = crate::http::shutdown::ShutdownState::new();
        let pool = Arc::new(Pool::with_factory(
            cap,
            idle_ttl,
            capacity_wait,
            activation_timeout,
            per_tenant_concurrency,
            factory,
            shutdown.clone(),
        ));
        (
            pool,
            BlockingHarness {
                shutdown,
                ..harness
            },
        )
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
        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        leader.abort();
        let _ = leader.await;

        let followed = tokio::time::timeout(Duration::from_secs(5), follower)
            .await
            .expect("the follower must terminate, not wait forever");
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
        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        drop(follower);
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

    #[tokio::test]
    async fn shutdown_wins_over_a_released_activation() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_shutdown", "tns_shutdown");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;

        // Release the factory and signal shutdown before polling the leader
        // again; a request must receive the shutdown refusal, not a guard.
        harness.release_one();
        harness.shutdown.begin();
        let result = tokio::time::timeout(Duration::from_secs(5), leader)
            .await
            .expect("shutdown cancels the pending activation")
            .expect("leader task joins");

        assert!(matches!(result, Err(PoolError::ShuttingDown)));
    }

    #[tokio::test]
    async fn shutdown_terminates_a_blocked_activation_producer() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_shutdown_producer", "tns_shutdown_producer");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        tokio::time::timeout(Duration::from_secs(1), harness.wait_until_entered())
            .await
            .expect("activation producer must enter the factory");

        harness.shutdown.begin();

        let result = tokio::time::timeout(Duration::from_secs(1), leader)
            .await
            .expect("shutdown must cancel a producer blocked in the factory")
            .expect("producer task joins");
        assert!(matches!(result, Err(PoolError::ShuttingDown)));
    }

    #[tokio::test]
    async fn shutdown_terminates_a_blocked_activation_follower() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        let tenant = spec("ten_shutdown_follower", "tns_shutdown_follower");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        tokio::time::timeout(Duration::from_secs(1), harness.wait_until_entered())
            .await
            .expect("activation producer must enter the factory");

        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        harness.shutdown.begin();

        let result = tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("shutdown must cancel a follower waiting on activation");
        assert!(matches!(result, Err(PoolError::ShuttingDown)));
        let leader_result = tokio::time::timeout(Duration::from_secs(1), leader)
            .await
            .expect("shutdown must also cancel the activation producer")
            .expect("producer task joins");
        assert!(matches!(leader_result, Err(PoolError::ShuttingDown)));
    }

    #[tokio::test]
    async fn shutdown_terminates_a_capacity_waiter() {
        let (pool, harness) = pool_over_blocking_factory(1).await;
        harness.release_one();
        let held = pool
            .acquire_spec_with_limit(&spec("ten_shutdown_full", "tns_shutdown_full"), 4)
            .await
            .expect("first runtime acquires and remains pinned");
        let waiting_tenant = spec("ten_shutdown_wait", "tns_shutdown_wait");
        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&waiting_tenant, 4));
        assert_pending_once(waiter.as_mut()).await;

        harness.shutdown.begin();

        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("shutdown must cancel a capacity wait");
        assert!(matches!(result, Err(PoolError::ShuttingDown)));
        drop(held);
    }

    #[tokio::test]
    async fn shutdown_terminates_a_tenant_permit_waiter_and_releases_its_pin() {
        let (pool, harness) = pool_with_blocking_factory(
            4,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            1,
        )
        .await;
        harness.release_one();
        let tenant = spec("ten_shutdown_permit", "tns_shutdown_permit");
        let held = pool
            .acquire_spec_with_limit(&tenant, 1)
            .await
            .expect("first request acquires the tenant permit");
        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&tenant, 1));
        assert_pending_once(waiter.as_mut()).await;

        harness.shutdown.begin();

        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("shutdown must cancel a tenant-permit wait");
        assert!(matches!(result, Err(PoolError::ShuttingDown)));
        drop(held);
        assert_eq!(
            pool.evict_idle().await,
            1,
            "the cancelled waiter must release its reserved runtime pin"
        );
    }

    #[tokio::test]
    async fn shutdown_wins_when_a_warm_acquisition_becomes_ready() {
        let (pool, harness) = pool_with_blocking_factory(
            4,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            1,
        )
        .await;
        harness.release_one();
        let tenant = spec("ten_shutdown_handoff", "tns_shutdown_handoff");
        let held = pool
            .acquire_spec_with_limit(&tenant, 1)
            .await
            .expect("first request holds the permit");
        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&tenant, 1));
        assert_pending_once(waiter.as_mut()).await;

        harness.shutdown.begin();
        drop(held);

        let result = tokio::time::timeout(Duration::from_secs(1), waiter)
            .await
            .expect("a ready permit cannot keep the acquisition pending");
        assert!(
            matches!(result, Err(PoolError::ShuttingDown)),
            "a warm runtime must not be handed out after shutdown"
        );
        assert_eq!(
            pool.evict_idle().await,
            1,
            "shutdown cancellation releases the waiter's pin"
        );
    }

    #[tokio::test]
    async fn http_deadline_cancels_activation_and_the_next_request_recovers() {
        let mut state = crate::http::HttpState::default_for_test().await;
        let registry = Arc::new(state.registry.clone());
        let (factory, harness) = blocking_factory(registry);
        let state_mut = Arc::get_mut(&mut state).expect("test state has one owner");
        state_mut.config.request_deadline = Duration::from_millis(30);
        state_mut.pool = Arc::new(Pool::with_factory(
            4,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(10),
            4,
            factory,
            state_mut.shutdown.clone(),
        ));

        let mut service = axum::Router::new()
            .route("/", axum::routing::get(acquire_for_deadline_test))
            .layer(axum::middleware::from_fn_with_state(
                Arc::clone(&state),
                crate::http::middleware::request_deadline,
            ))
            .with_state(state);
        let request = || {
            axum::http::Request::builder()
                .uri("/")
                .body(axum::body::Body::empty())
                .expect("request")
        };

        let first = service.call(request()).await.expect("first response");
        assert_eq!(
            first.status(),
            axum::http::StatusCode::REQUEST_TIMEOUT,
            "the actual request deadline must cancel the in-flight acquisition"
        );
        harness.wait_until_entered().await;
        harness.release_one();

        let second = tokio::time::timeout(Duration::from_secs(5), service.call(request()))
            .await
            .expect("the retry after request cancellation must terminate")
            .expect("second response");
        assert_eq!(
            second.status(),
            axum::http::StatusCode::OK,
            "the request after deadline cancellation must activate the tenant"
        );
    }

    #[tokio::test]
    async fn timed_out_activation_retries_after_backoff() {
        let (pool, harness) = pool_with_blocking_factory(
            4,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_millis(100),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        let tenant = spec("ten_timeout_backoff", "tns_timeout_backoff");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        tokio::time::timeout(Duration::from_secs(1), harness.wait_until_entered())
            .await
            .expect("activation producer must enter the factory");
        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        let timed_out = tokio::time::timeout(Duration::from_secs(1), leader)
            .await
            .expect("activation timeout must finish the producer")
            .expect("producer task joins");
        assert!(matches!(timed_out, Err(PoolError::ActivationFailed)));
        let followed = tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("activation timeout must also terminate followers");
        assert!(matches!(followed, Err(PoolError::ActivationFailed)));
        assert_eq!(harness.calls(), 1);

        harness.release_one();
        assert!(matches!(
            pool.acquire_spec_with_limit(&tenant, 4).await,
            Err(PoolError::ActivationFailed)
        ));
        assert_eq!(
            harness.calls(),
            1,
            "a retry inside negative backoff must not re-enter the factory"
        );

        let retry_at = tokio::time::Instant::from_std(
            Instant::now()
                + crate::http::runtime::lifecycle::ACTIVATION_BACKOFF
                + Duration::from_millis(20),
        );
        tokio::time::timeout(Duration::from_secs(6), tokio::time::sleep_until(retry_at))
            .await
            .expect("the activation backoff deadline must be finite");
        let recovered = tokio::time::timeout(
            Duration::from_secs(2),
            pool.acquire_spec_with_limit(&tenant, 4),
        )
        .await
        .expect("activation must be retryable after the backoff expires");
        assert!(recovered.is_ok());
        assert_eq!(harness.calls(), 2);
    }

    #[tokio::test]
    async fn revision_replacement_waits_until_old_guards_drain() {
        let (pool, harness) = pool_over_blocking_factory(1).await;
        harness.release_one();
        let mut original = spec("ten_revision_drain", "tns_revision_drain");
        let old_guard = pool
            .acquire_spec_with_limit(&original, 4)
            .await
            .expect("old revision");
        harness.wait_until_entered().await;
        let old_runtime = Arc::clone(old_guard.runtime());
        original.plan_version = 2;
        let replacement = original;

        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&replacement, 4));
        assert_pending_once(waiter.as_mut()).await;
        assert_eq!(harness.calls(), 1);

        drop(old_guard);
        assert_pending_once(waiter.as_mut()).await;
        harness.wait_until_entered().await;
        assert_eq!(harness.calls(), 2);
        harness.release_one();
        let new_guard = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("the pin release wakes the replacement")
            .expect("replacement activates");
        assert_eq!(new_guard.runtime().tenant_id, "ten_revision_drain");
        assert!(
            !Arc::ptr_eq(&old_runtime, new_guard.runtime()),
            "a revised tenant receives a newly activated runtime"
        );
        drop(new_guard);
    }

    #[tokio::test]
    async fn capacity_wait_retries_when_a_pin_is_released() {
        let registry = mem_registry().await;
        let pool = Arc::new(Pool::new(
            1,
            Duration::ZERO,
            Duration::from_secs(2),
            DEFAULT_ACTIVATION_TIMEOUT,
            DEFAULT_PER_TENANT_CONCURRENCY,
            registry,
        ));
        let held = pool
            .acquire_or_wait(&ready_tenant("ten_capacity_held", "tns_capacity_held"))
            .await
            .expect("first runtime");
        let waiting_tenant = ready_tenant("ten_capacity_waiter", "tns_capacity_waiter");
        let mut waiter = Box::pin(pool.acquire_or_wait(&waiting_tenant));
        assert_pending_once(waiter.as_mut()).await;
        drop(held);

        let acquired = tokio::time::timeout(Duration::from_secs(5), waiter)
            .await
            .expect("releasing a pin wakes the capacity waiter");
        assert!(acquired.is_ok());
    }

    #[tokio::test]
    async fn idle_ttl_expiry_wakes_capacity_waiter() {
        let idle_ttl = Duration::from_millis(300);
        let (pool, harness) = pool_with_blocking_factory(
            1,
            idle_ttl,
            Duration::from_secs(2),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.release_one();
        harness.release_one();
        let held = pool
            .acquire_spec_with_limit(&spec("ten_ttl_held", "tns_ttl_held"), 4)
            .await
            .expect("first runtime acquires");
        drop(held);

        let waiting_tenant = spec("ten_ttl_waiter", "tns_ttl_waiter");
        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&waiting_tenant, 4));
        assert_pending_once(waiter.as_mut()).await;
        let acquired = tokio::time::timeout(Duration::from_secs(2), waiter)
            .await
            .expect("the idle-TTL expiry must wake the capacity waiter");
        let acquired = acquired.expect("idle tenant was evicted");
        assert_eq!(acquired.runtime().tenant_id, "ten_ttl_waiter");
        assert_eq!(harness.calls(), 2);
    }

    #[tokio::test]
    async fn expired_failure_backoff_releases_capacity_for_another_tenant() {
        let (pool, harness) = pool_with_blocking_factory(
            1,
            Duration::ZERO,
            Duration::from_secs(7),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.fail_next.store(true, Ordering::SeqCst);
        let failed_tenant = spec("ten_failed_capacity", "tns_failed_capacity");
        let failed = pool.acquire_spec_with_limit(&failed_tenant, 4).await;
        assert!(matches!(failed, Err(PoolError::ActivationFailed)));
        harness.wait_until_entered().await;
        assert_eq!(harness.calls(), 1);

        let waiting = tokio::spawn({
            let pool = Arc::clone(&pool);
            async move {
                pool.acquire_spec_with_limit(&spec("ten_after_failed", "tns_after_failed"), 4)
                    .await
            }
        });
        let activation_deadline =
            crate::http::runtime::lifecycle::ACTIVATION_BACKOFF + Duration::from_secs(2);
        harness.wait_until_entered_within(activation_deadline).await;
        assert_eq!(harness.calls(), 2);
        harness.release_one();
        let acquired = tokio::time::timeout(Duration::from_secs(2), waiting)
            .await
            .expect("the waiting tenant completes after activation")
            .expect("waiter task joins")
            .expect("backoff expiry releases capacity");
        assert_eq!(acquired.runtime().tenant_id, "ten_after_failed");
    }

    /// Real-timer integration: renew through public acquisition, then observe
    /// the waiter between the original and renewed idle deadlines.
    #[tokio::test]
    async fn warm_reacquisition_preserves_capacity_until_the_renewed_idle_deadline() {
        let ttl = Duration::from_secs(1);
        let (pool, harness) = pool_with_blocking_factory(
            1,
            ttl,
            Duration::from_secs(4),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.release_one();
        let tenant = spec("ten_idle_renewal", "tns_idle_renewal");
        let first = pool
            .acquire_spec_with_limit(&tenant, 4)
            .await
            .expect("first runtime");
        harness.wait_until_entered().await;
        drop(first);
        let released_at = Instant::now();
        let mut waiter = tokio::spawn({
            let pool = Arc::clone(&pool);
            async move {
                pool.acquire_spec_with_limit(&spec("ten_idle_waiter", "tns_idle_waiter"), 4)
                    .await
            }
        });
        tokio::time::sleep_until(tokio::time::Instant::from_std(
            released_at + Duration::from_millis(500),
        ))
        .await;
        let refreshed = pool
            .acquire_spec_with_limit(&tenant, 4)
            .await
            .expect("warm runtime");
        drop(refreshed);
        let renewed_at = Instant::now();
        tokio::time::sleep_until(tokio::time::Instant::from_std(
            released_at + Duration::from_millis(1100),
        ))
        .await;
        assert!(
            Instant::now() < renewed_at + ttl,
            "observation precedes renewed expiry"
        );
        assert!(
            !waiter.is_finished(),
            "old expiry must not release renewed runtime"
        );
        assert_eq!(
            harness.calls(),
            1,
            "no second activation at the old deadline"
        );
        harness
            .wait_until_entered_within(Duration::from_secs(2))
            .await;
        harness.release_one();
        let acquired = tokio::time::timeout(Duration::from_secs(3), &mut waiter)
            .await
            .expect("bounded idle expiry")
            .expect("waiter joins")
            .expect("new tenant acquires");
        assert_eq!(acquired.runtime().tenant_id, "ten_idle_waiter");
    }

    #[tokio::test]
    async fn capacity_wakeups_do_not_extend_the_deadline() {
        let capacity_wait = Duration::from_millis(200);
        let (pool, harness) = pool_with_blocking_factory(
            1,
            Duration::ZERO,
            capacity_wait,
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.release_one();
        let keeper_tenant = spec("ten_deadline_held", "tns_deadline_held");
        let held = pool
            .acquire_spec_with_limit(&keeper_tenant, 4)
            .await
            .expect("first runtime acquires");
        let waiting_tenant = spec("ten_deadline_wait", "tns_deadline_wait");
        let mut waiter = Box::pin(pool.acquire_spec_with_limit(&waiting_tenant, 4));
        assert_pending_once(waiter.as_mut()).await;

        // Observe the caller's actual waker, not the pool's private notifier.
        struct CallerWaker(std::sync::atomic::AtomicU64);
        impl std::task::Wake for CallerWaker {
            fn wake(self: Arc<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let wakes = Arc::new(CallerWaker(std::sync::atomic::AtomicU64::new(0)));
        let waker = std::task::Waker::from(Arc::clone(&wakes));
        let started_at = Instant::now();
        {
            let mut observe = || {
                let mut context = std::task::Context::from_waker(&waker);
                waiter.as_mut().poll(&mut context)
            };
            assert!(observe().is_pending());
            let pulse = pool
                .acquire_spec_with_limit(&keeper_tenant, 4)
                .await
                .expect("warm pulse");
            wakes.0.store(0, Ordering::SeqCst);
            drop(pulse);
            assert!(
                wakes.0.load(Ordering::SeqCst) > 0,
                "public release wakes the caller"
            );
            assert!(observe().is_pending(), "caller consumes the first wakeup");
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                started_at + Duration::from_millis(80),
            ))
            .await;
            let pulse = pool
                .acquire_spec_with_limit(&keeper_tenant, 4)
                .await
                .expect("warm pulse");
            wakes.0.store(0, Ordering::SeqCst);
            drop(pulse);
            assert!(
                wakes.0.load(Ordering::SeqCst) > 0,
                "second public release wakes the caller"
            );
            assert!(observe().is_pending(), "caller consumes the second wakeup");
            tokio::time::sleep_until(tokio::time::Instant::from_std(
                started_at + Duration::from_millis(160),
            ))
            .await;
            let pulse = pool
                .acquire_spec_with_limit(&keeper_tenant, 4)
                .await
                .expect("warm pulse");
            wakes.0.store(0, Ordering::SeqCst);
            drop(pulse);
            assert!(
                wakes.0.load(Ordering::SeqCst) > 0,
                "third public release wakes the caller"
            );
            assert!(observe().is_pending(), "caller consumes the third wakeup");
        }
        let result = tokio::time::timeout(Duration::from_secs(1), &mut waiter)
            .await
            .expect("public acquisitions must not extend the capacity deadline");
        let elapsed = started_at.elapsed();
        assert!(matches!(result, Err(PoolError::CapacityTimeout)));
        assert!(
            elapsed < Duration::from_millis(300),
            "the waiter must finish near its original deadline despite repeated releases"
        );
        drop(held);
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
        harness.wait_until_entered().await;
        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        harness.release_one();
        let panicked = match leader.await {
            Ok(_) => panic!("a panicking factory must fail the leader task"),
            Err(error) => error,
        };
        assert!(panicked.is_panic(), "the leader must report the panic");
        let followed = tokio::time::timeout(Duration::from_secs(1), follower)
            .await
            .expect("factory panic must terminate existing followers");
        assert!(matches!(followed, Err(PoolError::ActivationFailed)));

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
        let (pool, harness) = pool_with_blocking_factory(
            1,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            1,
        )
        .await;
        harness.release_one();
        let tenant = spec("ten_pin_wait", "tns_pin_wait");
        let held = pool
            .acquire_spec_with_limit(&tenant, 1)
            .await
            .expect("first guard");

        // The second caller is parked on the tenant's concurrency limit, having
        // already chosen the runtime it will use.
        let mut waiting = Box::pin(pool.acquire_spec_with_limit(&tenant, 1));
        assert_pending_once(waiting.as_mut()).await;
        drop(held);

        assert_eq!(
            pool.evict_idle().await,
            0,
            "a runtime a caller is about to use is not idle"
        );

        let second = tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("the waiter must acquire once the permit is free");
        let second = second.expect("the public acquisition succeeds");
        assert_eq!(second.runtime().tenant_id, "ten_pin_wait");
        drop(second);
        assert_eq!(
            pool.evict_idle().await,
            1,
            "an unused runtime can now unload"
        );
    }

    /// A tenant id bound to different storage is refused, not silently served
    /// from the runtime that belongs to another namespace.
    #[tokio::test]
    async fn binding_mismatch_is_rejected_while_ready() {
        let pool = test_pool().await;
        let resident = pool
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
        let original = pool
            .acquire_or_wait(&ready_tenant("ten_bind", "tns_bind_a"))
            .await
            .expect("the original binding remains usable");
        assert_eq!(original.runtime().namespace, "tns_bind_a");
        assert!(
            Arc::ptr_eq(resident.runtime(), original.runtime()),
            "a refused binding cannot replace the resident runtime"
        );
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

    async fn test_pool() -> Arc<Pool> {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        Arc::new(Pool::with_defaults(registry))
    }

    #[tokio::test]
    async fn releasing_a_pin_wakes_drain_and_capacity_waiters() {
        let (pool, harness) = pool_with_blocking_factory(
            1,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.release_one();
        let mut original = spec("ten_notify", "tns_notify");
        let old_guard = pool
            .acquire_spec_with_limit(&original, 4)
            .await
            .expect("acquire runtime");
        harness.wait_until_entered().await;
        original.plan_version = 2;
        let mut drain_waiter = Box::pin(pool.acquire_spec_with_limit(&original, 4));
        assert_pending_once(drain_waiter.as_mut()).await;
        let capacity_tenant = spec("ten_capacity", "tns_capacity");
        let mut capacity_waiter = Box::pin(pool.acquire_spec_with_limit(&capacity_tenant, 4));
        assert_pending_once(capacity_waiter.as_mut()).await;

        drop(old_guard);
        assert_pending_once(drain_waiter.as_mut()).await;
        assert_pending_once(capacity_waiter.as_mut()).await;
        harness.wait_until_entered().await;
        assert_eq!(harness.calls(), 2);
        harness.release_one();

        let replacement = tokio::time::timeout(Duration::from_secs(5), &mut drain_waiter)
            .await
            .expect("the drain waiter activates the new revision")
            .expect("the new revision acquires");
        assert_eq!(replacement.runtime().tenant_id, "ten_notify");
        assert_pending_once(capacity_waiter.as_mut()).await;

        drop(replacement);
        assert_pending_once(capacity_waiter.as_mut()).await;
        harness.wait_until_entered().await;
        assert_eq!(harness.calls(), 3);
        harness.release_one();
        let acquired = tokio::time::timeout(Duration::from_secs(5), capacity_waiter)
            .await
            .expect("the capacity waiter recovers after the pin is released")
            .expect("the waiting tenant acquires");
        assert_eq!(acquired.runtime().tenant_id, "ten_capacity");
    }

    #[tokio::test]
    async fn resident_runtime_handles_exclude_loading_and_include_ready() {
        let (pool, harness) = pool_over_blocking_factory(1).await;
        let acquisition_pool = Arc::clone(&pool);
        let runtime_spec = spec("ten_snapshot_loading", "tns_snapshot_loading");
        let acquisition = tokio::spawn(async move {
            acquisition_pool
                .acquire_spec_with_limit(&runtime_spec, 4)
                .await
        });

        harness.wait_until_entered().await;
        assert!(
            pool.resident_runtime_handles().is_empty(),
            "a Loading slot does not yet retain a runtime"
        );

        harness.release_one();
        let guard = acquisition
            .await
            .expect("activation task joins")
            .expect("runtime activates");
        drop(guard);
        assert_eq!(pool.resident_runtime_handles().len(), 1);
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
        assert_eq!(second.runtime().tenant_id, "ten_y");
        drop(second);
    }

    #[tokio::test]
    async fn idle_eviction_removes_only_unpinned_runtimes() {
        let (pool, harness) = pool_with_blocking_factory(
            3,
            Duration::ZERO,
            Duration::from_secs(2),
            Duration::from_secs(30),
            DEFAULT_PER_TENANT_CONCURRENCY,
        )
        .await;
        harness.release_one();
        harness.release_one();
        harness.release_one();
        harness.release_one();
        harness.release_one();

        let idle_a = pool
            .acquire_spec_with_limit(&spec("ten_a", "tns_a"), 4)
            .await
            .expect("tenant A acquires");
        let runtime_a = Arc::clone(idle_a.runtime());
        drop(idle_a);
        let pinned_b = pool
            .acquire_spec_with_limit(&spec("ten_b", "tns_b"), 4)
            .await
            .expect("tenant B acquires");
        let idle_c = pool
            .acquire_spec_with_limit(&spec("ten_c", "tns_c"), 4)
            .await
            .expect("tenant C acquires");
        let runtime_c = Arc::clone(idle_c.runtime());
        drop(idle_c);

        assert_eq!(pool.evict_idle().await, 2);

        let same_b = pool
            .acquire_spec_with_limit(&spec("ten_b", "tns_b"), 4)
            .await
            .expect("a pinned runtime remains reusable");
        assert!(
            Arc::ptr_eq(pinned_b.runtime(), same_b.runtime()),
            "eviction retains the runtime held by an operation"
        );
        drop(same_b);
        drop(pinned_b);

        let new_a = pool
            .acquire_spec_with_limit(&spec("ten_a", "tns_a"), 4)
            .await
            .expect("an evicted tenant can activate again");
        let new_c = pool
            .acquire_spec_with_limit(&spec("ten_c", "tns_c"), 4)
            .await
            .expect("a second evicted tenant can activate again");
        assert!(!Arc::ptr_eq(&runtime_a, new_a.runtime()));
        assert!(!Arc::ptr_eq(&runtime_c, new_c.runtime()));
        assert_eq!(harness.calls(), 5);
    }

    // Integration scenarios below use a real SurrealDB Mem registry and the
    // production runtime factory. Their gates hold the real activation path
    // open; these are not isolated pool policy units.

    /// A cold leader remains in the factory until a public follower is pending.
    #[tokio::test]
    async fn single_flight_activation_runs_once() {
        let (pool, harness) = pool_over_blocking_factory(1).await;
        let tenant = spec("ten_single", "tns_single");
        let leader = tokio::spawn({
            let pool = Arc::clone(&pool);
            let tenant = tenant.clone();
            async move { pool.acquire_spec_with_limit(&tenant, 4).await }
        });
        harness.wait_until_entered().await;

        let mut follower = Box::pin(pool.acquire_spec_with_limit(&tenant, 4));
        assert_pending_once(follower.as_mut()).await;
        assert_eq!(
            harness.calls(),
            1,
            "the blocked cold path has one factory call"
        );
        harness.release_one();

        let leader_guard = tokio::time::timeout(Duration::from_secs(5), leader)
            .await
            .expect("cold leader completes")
            .expect("leader task joins")
            .expect("leader acquires");
        let follower_guard = tokio::time::timeout(Duration::from_secs(5), follower)
            .await
            .expect("follower acquires the published runtime")
            .expect("follower acquisition succeeds");

        assert_eq!(leader_guard.runtime().tenant_id, "ten_single");
        assert_eq!(follower_guard.runtime().tenant_id, "ten_single");
        assert!(
            Arc::ptr_eq(leader_guard.runtime(), follower_guard.runtime()),
            "overlapping cold acquisitions return the same runtime"
        );

        drop(leader_guard);
        drop(follower_guard);
        harness.release_one();
        let recovered = tokio::time::timeout(
            Duration::from_secs(5),
            pool.acquire_spec_with_limit(&spec("ten_recovered", "tns_recovered"), 4),
        )
        .await
        .expect("dropping both guards returns pool capacity")
        .expect("the next tenant acquires");
        assert_eq!(recovered.runtime().tenant_id, "ten_recovered");
        assert_eq!(harness.calls(), 2);
    }

    /// Component integration: constructed lease releases its actual guard.
    #[tokio::test]
    async fn dropping_an_unconsumed_response_body_releases_runtime_and_admission() {
        use crate::http::runtime::guard::{LeasedBody, ResponseLease};

        let (pool, harness) = pool_over_blocking_factory(1).await;
        harness.release_one();
        let held_runtime = pool
            .acquire_spec_with_limit(&spec("ten_response", "tns_response"), 4)
            .await
            .expect("runtime acquires");
        harness.wait_until_entered().await;
        let gate = Arc::new(AdmissionGate::new(1));
        let permit = gate.try_acquire().expect("first permit");
        let body = LeasedBody::new(
            axum::body::Body::from("hello"),
            ResponseLease::new(Some(Arc::new(held_runtime)), Arc::new(permit)),
        );
        let next_tenant = spec("ten_after_response", "tns_after");
        let mut capacity_waiter = Box::pin(pool.acquire_spec_with_limit(&next_tenant, 4));
        assert_pending_once(capacity_waiter.as_mut()).await;
        assert!(gate.try_acquire().is_err(), "the body retains admission");

        drop(body);
        assert!(gate.try_acquire().is_ok(), "body drop returns admission");
        assert_pending_once(capacity_waiter.as_mut()).await;
        harness.wait_until_entered().await;
        harness.release_one();
        let acquired = tokio::time::timeout(Duration::from_secs(5), capacity_waiter)
            .await
            .expect("body drop releases the runtime pin")
            .expect("the next tenant acquires");
        assert_eq!(acquired.runtime().tenant_id, "ten_after_response");
    }

    /// Full router integration: lease attachment must retain the real gate.
    #[cfg(feature = "test-fixtures")]
    #[tokio::test]
    async fn actual_http_response_retains_admission_until_body_drop() {
        use crate::http::config::HttpConfig;
        use crate::http::test_state::HttpStateTestBuilder;
        use axum::body::Body;
        use axum::http::{Request, StatusCode};

        const API_KEY: &str =
            "mem_sk_ak_aaaa0000-0000-4000-8000-000000000000_isolationtest0000000000000000000";
        let mut config = HttpConfig::default_for_test();
        config.global_request_limit = 1;
        let state = HttpStateTestBuilder::new()
            .await
            .with_config(config)
            .build()
            .await
            .expect("HTTP state");
        crate::http::test_bootstrap::apply_bootstrap_entries(
            &state,
            &format!("lease_test={API_KEY}"),
        )
        .await
        .expect("ready tenant");
        let mut router = crate::http::router::build_router(state, None).expect("real router");
        let request = || {
            Request::builder()
                .method("POST")
                .uri("/mcp")
                .header("host", "localhost")
                .header("content-type", "application/json")
                .header("accept", "application/json, text/event-stream")
                .header("authorization", format!("Bearer {API_KEY}"))
                .header("mcp-protocol-version", "2026-07-28")
                .header("mcp-method", "tools/list")
                .body(Body::from(serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": "tools/list",
                    "params": {"_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientInfo": {"name": "lease-test", "version": "0"},
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }}
                }).to_string()))
                .expect("MCP request")
        };
        let held = tokio::time::timeout(Duration::from_secs(10), router.call(request()))
            .await
            .expect("bounded first response")
            .expect("first response");
        assert_eq!(held.status(), StatusCode::OK);
        let refused = tokio::time::timeout(Duration::from_secs(5), router.call(request()))
            .await
            .expect("bounded refusal")
            .expect("refused response");
        assert_eq!(refused.status(), StatusCode::SERVICE_UNAVAILABLE);
        drop(held);
        let recovered = tokio::time::timeout(Duration::from_secs(10), router.call(request()))
            .await
            .expect("bounded recovery")
            .expect("recovered response");
        assert_eq!(recovered.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn a_pinned_runtime_causes_capacity_timeout_for_a_distinct_tenant() {
        let db = surrealdb::Surreal::new::<surrealdb::engine::local::Mem>(())
            .await
            .unwrap();
        db.use_ns("control").use_db("control").await.unwrap();
        let registry = Arc::new(RegistryHandle::in_memory_with_mem_engine(Arc::new(db)));
        let pool = Arc::new(Pool::new(
            1,
            DEFAULT_IDLE_TTL,
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

    #[tokio::test]
    async fn negative_cache_swallows_repeated_failures() {
        let (pool, harness) = pool_over_blocking_factory(4).await;
        harness.fail_next.store(true, Ordering::SeqCst);
        let tenant = spec("ten_neg", "tns_neg");

        let first = pool.acquire_spec_with_limit(&tenant, 4).await;
        assert!(matches!(first, Err(PoolError::ActivationFailed)));
        assert_eq!(harness.calls(), 1);

        let backoff = pool.acquire_spec_with_limit(&tenant, 4).await;
        assert!(matches!(backoff, Err(PoolError::ActivationFailed)));
        assert_eq!(
            harness.calls(),
            1,
            "a request inside negative backoff does not invoke the factory again"
        );
    }

    #[tokio::test]
    async fn a_reclaimed_slot_can_serve_a_later_tenant() {
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

        let tenant_a = pool
            .acquire_or_wait(&ready_tenant("ten_a", "tns_a"))
            .await
            .expect("first tenant activates");
        drop(tenant_a);
        let tenant_b = pool
            .acquire_or_wait(&ready_tenant("ten_b", "tns_b"))
            .await
            .expect("second tenant activates");
        drop(tenant_b);
        let tenant_c = pool
            .acquire_or_wait(&ready_tenant("ten_c", "tns_c"))
            .await
            .expect("a reclaimed slot activates for the next tenant");
        assert_eq!(tenant_c.runtime().tenant_id, "ten_c");
    }
}

#[cfg(test)]
mod admission_gate_tests {
    use super::AdmissionGate;
    use std::sync::Arc;

    #[test]
    fn request_budget_refuses_acquisitions_above_its_limit() {
        let gate = Arc::new(AdmissionGate::new(1));
        let _first = gate.try_acquire().expect("first request permit");

        assert!(gate.try_acquire().is_err());
    }

    #[test]
    fn subscriptions_use_a_separate_budget_from_requests() {
        let gate = Arc::new(AdmissionGate::new(1));
        let request = gate.try_acquire().expect("request permit");
        let subscription = gate
            .try_acquire_for(true)
            .expect("subscription has its own budget");

        assert!(gate.try_acquire().is_err());
        drop(request);
        drop(subscription);
        assert!(gate.try_acquire().is_ok());
    }
}
