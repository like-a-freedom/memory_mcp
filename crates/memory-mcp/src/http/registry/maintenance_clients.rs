//! Bounded namespace-bound database sessions for process-level maintenance.

use std::{
    collections::HashMap,
    mem::size_of,
    sync::{
        Arc, Mutex, MutexGuard, Weak,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use crate::{error::MemoryError, storage::client::DbClient};

#[cfg(test)]
use crate::storage::client::SurrealDbClient;

const PRODUCTION_CAPACITY: usize = 32;
const PRODUCTION_METADATA_BYTES: usize = 1024 * 1024;
const PRODUCTION_IDLE_TTL: Duration = Duration::from_secs(300);
const SLOT_ACCOUNTING_OVERHEAD: usize = 192;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct MaintenanceClientKey {
    pub namespace: String,
    pub database: String,
    pub engine_generation: u64,
    pub migration_dimension: Option<usize>,
    /// Ready-tenant paths require the catalog/readiness gate. Paths that
    /// inspect suspended or deleting namespaces use a separate key and never
    /// upgrade a schema as a side effect of their read.
    pub requires_ready_schema: bool,
}

impl MaintenanceClientKey {
    fn accounted_bytes(&self) -> usize {
        size_of::<Self>()
            .saturating_add(self.namespace.capacity())
            .saturating_add(self.database.capacity())
            .saturating_add(SLOT_ACCOUNTING_OVERHEAD)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct MaintenanceClientPoolConfig {
    pub capacity: usize,
    pub max_metadata_bytes: usize,
    pub idle_ttl: Duration,
}

impl Default for MaintenanceClientPoolConfig {
    fn default() -> Self {
        Self {
            capacity: PRODUCTION_CAPACITY,
            max_metadata_bytes: PRODUCTION_METADATA_BYTES,
            idle_ttl: PRODUCTION_IDLE_TTL,
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait MaintenanceClientFactory: Send + Sync + 'static {
    async fn bind(&self, key: &MaintenanceClientKey) -> Result<Arc<dyn DbClient>, MemoryError>;
}

#[derive(Clone)]
pub struct MaintenanceClients {
    inner: Arc<Inner>,
}

pub struct MaintenanceClientLease {
    inner: Arc<Inner>,
    key: Arc<MaintenanceClientKey>,
    id: u64,
    client: Arc<dyn DbClient>,
}

/// Shared adapter that retires the exact pool generation when a database
/// operation proves its session is broken. All maintenance users receive this
/// adapter, so reconnect behavior cannot depend on each job remembering to
/// invalidate the pool.
struct MaintenanceDbClient {
    delegate: Arc<dyn DbClient>,
    inner: Weak<Inner>,
    key: Arc<MaintenanceClientKey>,
    id: u64,
}

impl MaintenanceDbClient {
    fn observe<T>(&self, result: Result<T, MemoryError>) -> Result<T, MemoryError> {
        if let Err(error) = &result
            && crate::platform::persistence::db_errors::is_db_session_failure(error)
            && let Some(inner) = self.inner.upgrade()
        {
            invalidate_slot(&inner, &self.key, Some(self.id));
        }
        result
    }
}

#[async_trait::async_trait]
impl DbClient for MaintenanceDbClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        self.observe(self.delegate.select_one(record_id, namespace).await)
    }

    async fn select_table(
        &self,
        table: crate::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        self.observe(self.delegate.select_table(table, namespace).await)
    }

    async fn create(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, MemoryError> {
        self.observe(
            self.delegate
                .create(record_id, content, namespace, temporal_fields)
                .await,
        )
    }

    async fn update(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, MemoryError> {
        self.observe(
            self.delegate
                .update(record_id, content, namespace, temporal_fields)
                .await,
        )
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
        namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        self.observe(self.delegate.query(sql, vars, namespace).await)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.observe(self.delegate.apply_migrations(namespace).await)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MaintenanceClientSnapshot {
    /// Bound sessions plus in-flight bindings. Both consume the configured
    /// slot budget until they are closed or published and evicted.
    pub resident_sessions: usize,
    pub creating_bindings: usize,
    pub active_leases: usize,
    /// Accounted key and slot metadata. The SDK client allocation is reported
    /// separately by the process memory snapshot.
    pub metadata_accounted_bytes: usize,
    pub bind_attempts: usize,
    pub reuse_hits: usize,
    pub evictions: usize,
}

struct Inner {
    factory: Arc<dyn MaintenanceClientFactory>,
    config: MaintenanceClientPoolConfig,
    state: Mutex<State>,
    changed: tokio::sync::watch::Sender<u64>,
    shutdown: tokio_util::sync::CancellationToken,
    bind_attempts: AtomicUsize,
    reuse_hits: AtomicUsize,
    evictions: AtomicUsize,
}

#[derive(Default)]
struct State {
    slots: HashMap<Arc<MaintenanceClientKey>, Slot>,
    metadata_bytes: usize,
    next_id: u64,
    shutting_down: bool,
}

struct Slot {
    id: u64,
    metadata_bytes: usize,
    state: SlotState,
}

enum SlotState {
    Creating {
        invalidated: bool,
    },
    Ready {
        client: Arc<dyn DbClient>,
        active_leases: usize,
        last_used: tokio::time::Instant,
        invalidated: bool,
    },
}

impl MaintenanceClientLease {
    /// Return an outer `Arc` clone. All clones point at the same permanently
    /// namespace-bound adapter; they cannot select another tenant.
    pub fn client(&self) -> Arc<dyn DbClient> {
        Arc::clone(&self.client)
    }
}

impl Drop for MaintenanceClientLease {
    fn drop(&mut self) {
        let mut state = lock_state(&self.inner.state);
        let shutting_down = state.shutting_down;
        let remove = match state.slots.get_mut(&self.key) {
            Some(slot) if slot.id == self.id => match &mut slot.state {
                SlotState::Ready {
                    active_leases,
                    last_used,
                    invalidated,
                    ..
                } => {
                    *active_leases = active_leases.saturating_sub(1);
                    if *active_leases == 0 {
                        *last_used = tokio::time::Instant::now();
                    }
                    *active_leases == 0 && (*invalidated || shutting_down)
                }
                SlotState::Creating { .. } => false,
            },
            _ => false,
        };
        if remove {
            remove_slot(&mut state, &self.key);
            self.inner.evictions.fetch_add(1, Ordering::Relaxed);
        }
        drop(state);
        signal_change(&self.inner);
    }
}

impl MaintenanceClients {
    fn new(
        factory: Arc<dyn MaintenanceClientFactory>,
        config: MaintenanceClientPoolConfig,
    ) -> Self {
        let (changed, _) = tokio::sync::watch::channel(0_u64);
        Self {
            inner: Arc::new(Inner {
                factory,
                config,
                state: Mutex::new(State::default()),
                changed,
                shutdown: tokio_util::sync::CancellationToken::new(),
                bind_attempts: AtomicUsize::new(0),
                reuse_hits: AtomicUsize::new(0),
                evictions: AtomicUsize::new(0),
            }),
        }
    }

    /// Process-owned maintenance pool for the selected tenant engine.
    pub(crate) fn for_engine(
        engine: super::PrivilegedEngine,
        generation: u64,
        dimension: Option<usize>,
        config: MaintenanceClientPoolConfig,
    ) -> Self {
        Self::new(
            Arc::new(EngineFactory {
                engine,
                generation,
                dimension,
            }),
            config,
        )
    }

    #[cfg(any(test, feature = "test-fixtures"))]
    pub fn with_engine_for_test(
        engine: super::PrivilegedEngine,
        config: MaintenanceClientPoolConfig,
    ) -> Self {
        Self::new(Arc::new(TestEngineFactory { engine }), config)
    }

    #[cfg(test)]
    fn with_factory_for_test(
        factory: Arc<dyn MaintenanceClientFactory>,
        config: MaintenanceClientPoolConfig,
    ) -> Self {
        Self::new(factory, config)
    }

    pub async fn acquire(
        &self,
        key: MaintenanceClientKey,
    ) -> Result<MaintenanceClientLease, MemoryError> {
        if key.migration_dimension == Some(0) {
            return Err(MemoryError::ConfigInvalid(
                "maintenance migration dimension must be greater than zero".into(),
            ));
        }
        let metadata_bytes = key.accounted_bytes();
        if metadata_bytes > self.inner.config.max_metadata_bytes {
            return Err(MemoryError::ConfigInvalid(
                "maintenance client key exceeds the metadata budget".into(),
            ));
        }
        // Keep one owned copy of namespace/database metadata per resident
        // slot. Every concurrent lease shares this identity instead of
        // cloning two strings into every operation handle.
        let key = Arc::new(key);

        loop {
            // Subscribe before checking the map so a state change between the
            // check and await cannot strand this caller.
            let mut changed = self.inner.changed.subscribe();
            let now = tokio::time::Instant::now();
            let mut retired = Vec::new();
            let action = {
                let mut state = lock_state(&self.inner.state);
                if state.shutting_down {
                    return Err(shutting_down_error());
                }

                let resident_before_expiry = state.slots.len();
                evict_expired(&mut state, now, self.inner.config.idle_ttl, &mut retired);
                let expired_count = resident_before_expiry.saturating_sub(state.slots.len());
                self.inner
                    .evictions
                    .fetch_add(expired_count, Ordering::Relaxed);
                if let Some(slot) = state.slots.get_mut(&key) {
                    match &mut slot.state {
                        SlotState::Ready {
                            client,
                            active_leases,
                            last_used,
                            invalidated: false,
                        } => {
                            *active_leases = active_leases.saturating_add(1);
                            *last_used = now;
                            self.inner.reuse_hits.fetch_add(1, Ordering::Relaxed);
                            AcquireAction::Lease(slot.id, Arc::clone(client))
                        }
                        SlotState::Creating { .. }
                        | SlotState::Ready {
                            invalidated: true, ..
                        } => AcquireAction::Wait,
                    }
                } else if state.slots.len() < self.inner.config.capacity
                    && state.metadata_bytes.saturating_add(metadata_bytes)
                        <= self.inner.config.max_metadata_bytes
                {
                    state.next_id = state.next_id.wrapping_add(1).max(1);
                    let id = state.next_id;
                    state.metadata_bytes = state.metadata_bytes.saturating_add(metadata_bytes);
                    state.slots.insert(
                        Arc::clone(&key),
                        Slot {
                            id,
                            metadata_bytes,
                            state: SlotState::Creating { invalidated: false },
                        },
                    );
                    self.inner.bind_attempts.fetch_add(1, Ordering::Relaxed);
                    AcquireAction::Create(id)
                } else if evict_oldest_idle(&mut state, &mut retired) {
                    self.inner.evictions.fetch_add(1, Ordering::Relaxed);
                    AcquireAction::Retry
                } else {
                    AcquireAction::Wait
                }
            };
            drop(retired);
            match action {
                AcquireAction::Lease(id, client) => {
                    return Ok(MaintenanceClientLease {
                        inner: Arc::clone(&self.inner),
                        key: Arc::clone(&key),
                        id,
                        client,
                    });
                }
                AcquireAction::Retry => {
                    signal_change(&self.inner);
                }
                AcquireAction::Wait => wait_for_change(&self.inner, &mut changed).await?,
                AcquireAction::Create(id) => {
                    let mut guard = CreationGuard {
                        inner: Arc::clone(&self.inner),
                        key: Arc::clone(&key),
                        id,
                        completed: false,
                    };
                    let bound = tokio::select! {
                        biased;
                        _ = self.inner.shutdown.cancelled() => Err(shutting_down_error()),
                        result = self.inner.factory.bind(&key) => result,
                    }?;
                    let bound: Arc<dyn DbClient> = Arc::new(MaintenanceDbClient {
                        delegate: bound,
                        inner: Arc::downgrade(&self.inner),
                        key: Arc::clone(&key),
                        id,
                    });

                    let published = {
                        let mut state = lock_state(&self.inner.state);
                        if state.shutting_down {
                            false
                        } else if let Some(slot) = state.slots.get_mut(&key) {
                            if slot.id == id {
                                let invalidated =
                                    matches!(slot.state, SlotState::Creating { invalidated: true });
                                if invalidated {
                                    false
                                } else {
                                    slot.state = SlotState::Ready {
                                        client: Arc::clone(&bound),
                                        active_leases: 1,
                                        last_used: tokio::time::Instant::now(),
                                        invalidated: false,
                                    };
                                    guard.completed = true;
                                    true
                                }
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    };
                    signal_change(&self.inner);
                    if published {
                        return Ok(MaintenanceClientLease {
                            inner: Arc::clone(&self.inner),
                            key: Arc::clone(&key),
                            id,
                            client: bound,
                        });
                    }
                    return Err(shutting_down_error());
                }
            }
        }
    }

    /// Stop admissions, cancel in-flight binds, drop idle sessions and wait
    /// until every active maintenance lease has been released.
    pub async fn shutdown(&self) {
        self.begin_shutdown();
        loop {
            let mut changed = self.inner.changed.subscribe();
            if lock_state(&self.inner.state).slots.is_empty() {
                return;
            }
            if changed.changed().await.is_err() {
                return;
            }
        }
    }

    /// Synchronously close admission and retire idle sessions before a
    /// caller begins its bounded asynchronous drain.
    pub fn begin_shutdown(&self) {
        let mut retired = Vec::new();
        {
            let mut state = lock_state(&self.inner.state);
            if state.shutting_down {
                return;
            }
            state.shutting_down = true;
            let keys = state.slots.keys().cloned().collect::<Vec<_>>();
            for key in keys {
                let remove = match state.slots.get_mut(&key) {
                    Some(slot) => match &mut slot.state {
                        SlotState::Creating { invalidated } => {
                            *invalidated = true;
                            false
                        }
                        SlotState::Ready {
                            active_leases,
                            invalidated,
                            ..
                        } => {
                            *invalidated = true;
                            *active_leases == 0
                        }
                    },
                    None => false,
                };
                if remove && let Some(slot) = remove_slot(&mut state, &key) {
                    retired.push(slot);
                    self.inner.evictions.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        self.inner.shutdown.cancel();
        signal_change(&self.inner);
        drop(retired);
    }

    pub fn snapshot(&self) -> MaintenanceClientSnapshot {
        let state = lock_state(&self.inner.state);
        let mut creating_bindings = 0;
        let mut active_leases = 0;
        for slot in state.slots.values() {
            match &slot.state {
                SlotState::Creating { .. } => creating_bindings += 1,
                SlotState::Ready {
                    active_leases: active,
                    ..
                } => active_leases += active,
            }
        }
        MaintenanceClientSnapshot {
            resident_sessions: state.slots.len(),
            creating_bindings,
            active_leases,
            metadata_accounted_bytes: state.metadata_bytes,
            bind_attempts: self.inner.bind_attempts.load(Ordering::Relaxed),
            reuse_hits: self.inner.reuse_hits.load(Ordering::Relaxed),
            evictions: self.inner.evictions.load(Ordering::Relaxed),
        }
    }
}

enum AcquireAction {
    Lease(u64, Arc<dyn DbClient>),
    Create(u64),
    Retry,
    Wait,
}

struct CreationGuard {
    inner: Arc<Inner>,
    key: Arc<MaintenanceClientKey>,
    id: u64,
    completed: bool,
}

impl Drop for CreationGuard {
    fn drop(&mut self) {
        if self.completed {
            return;
        }
        let mut state = lock_state(&self.inner.state);
        if state.slots.get(&self.key).is_some_and(|slot| {
            slot.id == self.id && matches!(&slot.state, SlotState::Creating { .. })
        }) {
            remove_slot(&mut state, &self.key);
        }
        drop(state);
        signal_change(&self.inner);
    }
}

struct EngineFactory {
    engine: super::PrivilegedEngine,
    generation: u64,
    dimension: Option<usize>,
}

#[async_trait::async_trait]
impl MaintenanceClientFactory for EngineFactory {
    async fn bind(&self, key: &MaintenanceClientKey) -> Result<Arc<dyn DbClient>, MemoryError> {
        if key.engine_generation != self.generation || key.migration_dimension != self.dimension {
            return Err(MemoryError::Conflict(
                "maintenance client key does not match the selected engine generation or embedding dimension".into(),
            ));
        }
        if key.requires_ready_schema {
            let binding = crate::models::registry::NamespaceBinding {
                namespace: key.namespace.clone(),
                database: key.database.clone(),
            };
            let migrations =
                crate::http::leases::migration::SurrealTenantMigrations::new(self.engine.clone());
            crate::provisioning::api::ensure_ready_schema(
                &migrations,
                &binding,
                key.migration_dimension,
            )
            .await?;
        }
        let client = self
            .engine
            .bind_namespace_with_dimension(&key.namespace, &key.database, key.migration_dimension)
            .await?;
        Ok(client)
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
struct TestEngineFactory {
    engine: super::PrivilegedEngine,
}

#[cfg(any(test, feature = "test-fixtures"))]
#[async_trait::async_trait]
impl MaintenanceClientFactory for TestEngineFactory {
    async fn bind(&self, key: &MaintenanceClientKey) -> Result<Arc<dyn DbClient>, MemoryError> {
        let client = self
            .engine
            .bind_namespace_with_dimension(&key.namespace, &key.database, key.migration_dimension)
            .await?;
        Ok(client)
    }
}

fn evict_expired(
    state: &mut State,
    now: tokio::time::Instant,
    idle_ttl: Duration,
    retired: &mut Vec<Slot>,
) {
    let keys = state
        .slots
        .iter()
        .filter_map(|(key, slot)| match &slot.state {
            SlotState::Ready {
                active_leases: 0,
                last_used,
                invalidated,
                ..
            } if *invalidated || now.saturating_duration_since(*last_used) >= idle_ttl => {
                Some(key.clone())
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    for key in keys {
        if let Some(slot) = remove_slot(state, &key) {
            retired.push(slot);
        }
    }
}

fn evict_oldest_idle(state: &mut State, retired: &mut Vec<Slot>) -> bool {
    let oldest = state
        .slots
        .iter()
        .filter_map(|(key, slot)| match &slot.state {
            SlotState::Ready {
                active_leases: 0,
                last_used,
                ..
            } => Some((key.clone(), *last_used)),
            SlotState::Creating { .. } | SlotState::Ready { .. } => None,
        })
        .min_by_key(|(_, last_used)| *last_used)
        .map(|(key, _)| key);
    if let Some(key) = oldest
        && let Some(slot) = remove_slot(state, &key)
    {
        retired.push(slot);
        return true;
    }
    false
}

fn remove_slot(state: &mut State, key: &MaintenanceClientKey) -> Option<Slot> {
    let slot = state.slots.remove(key)?;
    state.metadata_bytes = state.metadata_bytes.saturating_sub(slot.metadata_bytes);
    Some(slot)
}

async fn wait_for_change(
    inner: &Inner,
    changed: &mut tokio::sync::watch::Receiver<u64>,
) -> Result<(), MemoryError> {
    tokio::select! {
        biased;
        _ = inner.shutdown.cancelled() => Err(shutting_down_error()),
        result = changed.changed() => result.map_err(|_| shutting_down_error()),
    }
}

fn signal_change(inner: &Inner) {
    inner
        .changed
        .send_modify(|revision| *revision = revision.wrapping_add(1));
}

fn invalidate_slot(inner: &Arc<Inner>, key: &MaintenanceClientKey, expected_id: Option<u64>) {
    let mut retired = Vec::new();
    let mut state = lock_state(&inner.state);
    let remove = match state.slots.get_mut(key) {
        Some(slot) if expected_id.is_none_or(|id| slot.id == id) => match &mut slot.state {
            SlotState::Creating { invalidated } => {
                *invalidated = true;
                false
            }
            SlotState::Ready {
                active_leases,
                invalidated,
                ..
            } => {
                *invalidated = true;
                *active_leases == 0
            }
        },
        _ => false,
    };
    if remove && let Some(slot) = remove_slot(&mut state, key) {
        retired.push(slot);
        inner.evictions.fetch_add(1, Ordering::Relaxed);
    }
    drop(state);
    drop(retired);
    signal_change(inner);
}

fn shutting_down_error() -> MemoryError {
    MemoryError::Conflict("maintenance client pool is shutting down".into())
}

fn lock_state(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        future::Future,
        pin::Pin,
        sync::{
            Arc, Mutex, Weak,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        task::{Context, Poll, Waker},
        time::Duration,
    };

    use super::*;
    use crate::http::registry::PrivilegedEngine;
    use surrealdb::{Surreal, engine::local::Mem};
    use tokio::sync::Semaphore;

    struct ControlledFactory {
        engine: PrivilegedEngine,
        bind_count: AtomicUsize,
        starts: Semaphore,
        release: Semaphore,
        block_binds: bool,
        fail_once: AtomicBool,
        last_client: Mutex<Option<Weak<SurrealDbClient>>>,
    }

    impl ControlledFactory {
        fn new(engine: PrivilegedEngine) -> Self {
            Self {
                engine,
                bind_count: AtomicUsize::new(0),
                starts: Semaphore::new(0),
                release: Semaphore::new(0),
                block_binds: false,
                fail_once: AtomicBool::new(false),
                last_client: Mutex::new(None),
            }
        }

        async fn wait_until_started(&self) {
            self.starts
                .acquire()
                .await
                .expect("test factory remains open")
                .forget();
        }

        fn release_bind(&self) {
            self.release.add_permits(1);
        }

        fn last_client(&self) -> Option<Weak<SurrealDbClient>> {
            self.last_client
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait::async_trait]
    impl MaintenanceClientFactory for ControlledFactory {
        async fn bind(&self, key: &MaintenanceClientKey) -> Result<Arc<dyn DbClient>, MemoryError> {
            self.bind_count.fetch_add(1, Ordering::Relaxed);
            if self.block_binds {
                self.starts.add_permits(1);
                self.release
                    .acquire()
                    .await
                    .map_err(|error| MemoryError::Storage(error.to_string()))?
                    .forget();
            }
            if self.fail_once.swap(false, Ordering::AcqRel) {
                return Err(MemoryError::Storage("controlled bind failure".into()));
            }
            let client = self
                .engine
                .bind_namespace_with_dimension(
                    &key.namespace,
                    &key.database,
                    key.migration_dimension,
                )
                .await?;
            *self
                .last_client
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::downgrade(&client));
            Ok(client)
        }
    }

    struct BrokenOnceFactory {
        binds: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl MaintenanceClientFactory for BrokenOnceFactory {
        async fn bind(
            &self,
            _key: &MaintenanceClientKey,
        ) -> Result<Arc<dyn DbClient>, MemoryError> {
            let attempt = self.binds.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new(ScriptedSessionClient {
                fail_connection: attempt == 0,
            }))
        }
    }

    struct ScriptedSessionClient {
        fail_connection: bool,
    }

    #[async_trait::async_trait]
    impl DbClient for ScriptedSessionClient {
        async fn select_one(
            &self,
            _record_id: &str,
            _namespace: &str,
        ) -> Result<Option<serde_json::Value>, MemoryError> {
            Ok(None)
        }

        async fn select_table(
            &self,
            _table: crate::storage::table_scope::OwnedTable,
            _namespace: &str,
        ) -> Result<Vec<serde_json::Value>, MemoryError> {
            Ok(Vec::new())
        }

        async fn create(
            &self,
            _record_id: &str,
            _content: serde_json::Value,
            _namespace: &str,
            _temporal_fields: &[&str],
        ) -> Result<serde_json::Value, MemoryError> {
            Ok(serde_json::Value::Null)
        }

        async fn update(
            &self,
            _record_id: &str,
            _content: serde_json::Value,
            _namespace: &str,
            _temporal_fields: &[&str],
        ) -> Result<serde_json::Value, MemoryError> {
            Ok(serde_json::Value::Null)
        }

        async fn query(
            &self,
            _sql: &str,
            _vars: Option<serde_json::Value>,
            _namespace: &str,
        ) -> Result<serde_json::Value, MemoryError> {
            if self.fail_connection {
                Err(MemoryError::Storage("WebSocket connection closed".into()))
            } else {
                Ok(serde_json::json!([]))
            }
        }

        async fn apply_migrations(&self, _namespace: &str) -> Result<(), MemoryError> {
            Ok(())
        }
    }

    fn poll_once<T>(future: Pin<&mut impl Future<Output = T>>) -> Poll<T> {
        let mut context = Context::from_waker(Waker::noop());
        future.poll(&mut context)
    }

    async fn test_engine() -> PrivilegedEngine {
        let db = Surreal::new::<Mem>(())
            .await
            .expect("embedded SurrealDB starts");
        PrivilegedEngine::LocalMem(Arc::new(db))
    }

    fn key(namespace: &str, database: &str) -> MaintenanceClientKey {
        MaintenanceClientKey {
            namespace: namespace.to_owned(),
            database: database.to_owned(),
            engine_generation: 1,
            migration_dimension: None,
            requires_ready_schema: false,
        }
    }

    fn config(
        capacity: usize,
        metadata_bytes: usize,
        idle_ttl: Duration,
    ) -> MaintenanceClientPoolConfig {
        MaintenanceClientPoolConfig {
            capacity,
            max_metadata_bytes: metadata_bytes,
            idle_ttl,
        }
    }

    #[tokio::test]
    async fn cancelled_create_releases_the_reserved_slot_and_metadata() {
        let mut factory = ControlledFactory::new(test_engine().await);
        factory.block_binds = true;
        let factory = Arc::new(factory);
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(1, 4096, Duration::from_secs(300)),
        );
        let acquire = tokio::spawn({
            let pool = pool.clone();
            async move { pool.acquire(key("tns_cancel_create", "memory")).await }
        });
        factory.wait_until_started().await;
        acquire.abort();
        let _ = acquire.await;

        let snapshot = pool.snapshot();
        assert_eq!(snapshot.resident_sessions, 0);
        assert_eq!(snapshot.creating_bindings, 0);
        assert_eq!(snapshot.metadata_accounted_bytes, 0);

        factory.release_bind();
        let recovered = pool
            .acquire(key("tns_cancel_create", "memory"))
            .await
            .expect("a cancelled creator does not strand capacity");
        assert_eq!(factory.bind_count.load(Ordering::Relaxed), 2);
        drop(recovered);
    }

    #[tokio::test]
    async fn a_failed_bind_releases_its_reservation_for_retry() {
        let factory = ControlledFactory::new(test_engine().await);
        factory.fail_once.store(true, Ordering::Release);
        let factory = Arc::new(factory);
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(1, 4096, Duration::from_secs(300)),
        );

        assert!(
            pool.acquire(key("tns_failed_bind", "memory"))
                .await
                .is_err()
        );
        let failed = pool.snapshot();
        assert_eq!(failed.resident_sessions, 0);
        assert_eq!(failed.metadata_accounted_bytes, 0);

        let lease = pool
            .acquire(key("tns_failed_bind", "memory"))
            .await
            .expect("failed creation is retryable");
        assert_eq!(factory.bind_count.load(Ordering::Relaxed), 2);
        drop(lease);
    }

    #[tokio::test]
    async fn active_leases_pin_sessions_when_capacity_is_full() {
        let pool = MaintenanceClients::with_engine_for_test(
            test_engine().await,
            config(1, 4096, Duration::from_secs(300)),
        );
        let first = pool
            .acquire(key("tns_pinned", "memory"))
            .await
            .expect("first tenant binds");
        let mut pending = Box::pin(pool.acquire(key("tns_waiting", "memory")));
        assert!(poll_once(pending.as_mut()).is_pending());
        assert_eq!(pool.snapshot().bind_attempts, 1);
        assert_eq!(pool.snapshot().active_leases, 1);

        drop(first);
        let second = pending
            .await
            .expect("capacity is released when the active lease drops");
        assert_eq!(pool.snapshot().bind_attempts, 2);
        drop(second);
    }

    #[tokio::test]
    async fn eviction_releases_the_sdk_client_and_counts_the_eviction() {
        let factory = Arc::new(ControlledFactory::new(test_engine().await));
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(1, 4096, Duration::from_secs(300)),
        );
        let first = pool
            .acquire(key("tns_evict_first", "memory"))
            .await
            .expect("first tenant binds");
        drop(first);
        let first_client = factory.last_client().expect("factory saw first adapter");

        let second = pool
            .acquire(key("tns_evict_second", "memory"))
            .await
            .expect("capacity eviction admits the next tenant");

        assert!(first_client.upgrade().is_none());
        assert_eq!(pool.snapshot().evictions, 1);
        drop(second);
    }

    #[tokio::test]
    async fn idle_ttl_eviction_releases_session_without_waiting_for_capacity_pressure() {
        let factory = Arc::new(ControlledFactory::new(test_engine().await));
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(4, 4096, Duration::ZERO),
        );
        let lease = pool
            .acquire(key("tns_ttl", "memory"))
            .await
            .expect("tenant binds");
        drop(lease);
        let expired_client = factory.last_client().expect("factory saw client");

        let next = pool
            .acquire(key("tns_after_ttl", "memory"))
            .await
            .expect("expired binding is replaced");

        assert!(expired_client.upgrade().is_none());
        assert_eq!(pool.snapshot().evictions, 1);
        drop(next);
    }

    #[tokio::test]
    async fn a_session_failure_retires_the_binding_and_reconnects_once() {
        let factory = Arc::new(BrokenOnceFactory {
            binds: AtomicUsize::new(0),
        });
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(2, 4096, Duration::from_secs(300)),
        );
        let identity = key("tns_session_failure", "memory");
        let first = pool
            .acquire(identity.clone())
            .await
            .expect("initial maintenance session binds");

        assert!(matches!(
            first.client().query("SELECT * FROM fact", None, "tns_session_failure").await,
            Err(MemoryError::Storage(message)) if message.contains("WebSocket connection closed")
        ));
        let mut reconnect = Box::pin(pool.acquire(identity));
        assert!(poll_once(reconnect.as_mut()).is_pending());
        assert_eq!(factory.binds.load(Ordering::SeqCst), 1);

        drop(first);
        let next = reconnect.await.expect("broken session is rebound");
        assert_eq!(factory.binds.load(Ordering::SeqCst), 2);
        assert_eq!(pool.snapshot().evictions, 1);
        assert_eq!(
            next.client()
                .query("SELECT * FROM fact", None, "tns_session_failure")
                .await
                .expect("new session answers"),
            serde_json::json!([])
        );
        drop(next);
    }

    #[tokio::test]
    async fn a_stale_session_failure_cannot_invalidate_its_replacement() {
        let factory = Arc::new(BrokenOnceFactory {
            binds: AtomicUsize::new(0),
        });
        let pool = MaintenanceClients::with_factory_for_test(
            factory.clone(),
            config(2, 4096, Duration::from_secs(300)),
        );
        let identity = key("tns_stale_session", "memory");
        let first = pool
            .acquire(identity.clone())
            .await
            .expect("initial maintenance session binds");
        let stale_client = first.client();
        assert!(
            stale_client
                .query("SELECT * FROM fact", None, "tns_stale_session")
                .await
                .is_err()
        );
        drop(first);

        let replacement = pool
            .acquire(identity.clone())
            .await
            .expect("broken binding is replaced");
        assert_eq!(factory.binds.load(Ordering::SeqCst), 2);
        assert_eq!(pool.snapshot().evictions, 1);

        assert!(
            stale_client
                .query("SELECT * FROM fact", None, "tns_stale_session")
                .await
                .is_err()
        );
        assert_eq!(pool.snapshot().evictions, 1);

        let mut reuse = Box::pin(pool.acquire(identity));
        let Poll::Ready(Ok(reused)) = poll_once(reuse.as_mut()) else {
            panic!("stale binding must not retire its replacement");
        };
        assert_eq!(factory.binds.load(Ordering::SeqCst), 2);
        assert_eq!(pool.snapshot().evictions, 1);
        drop((replacement, reused, stale_client));
    }

    #[tokio::test]
    async fn shutdown_closes_admission_drops_idle_and_drains_active_leases() {
        let pool = MaintenanceClients::with_engine_for_test(
            test_engine().await,
            config(2, 4096, Duration::from_secs(300)),
        );
        let active = pool
            .acquire(key("tns_shutdown_active", "memory"))
            .await
            .expect("active tenant binds");
        let idle = pool
            .acquire(key("tns_shutdown_idle", "memory"))
            .await
            .expect("idle tenant binds");
        drop(idle);

        let mut shutdown = Box::pin(pool.shutdown());
        assert!(poll_once(shutdown.as_mut()).is_pending());
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.resident_sessions, 1);
        assert_eq!(snapshot.active_leases, 1);
        assert!(
            pool.acquire(key("tns_shutdown_late", "memory"))
                .await
                .is_err()
        );

        drop(active);
        shutdown.await;
        assert_eq!(pool.snapshot().resident_sessions, 0);
        assert_eq!(pool.snapshot().active_leases, 0);
    }

    #[tokio::test]
    async fn metadata_budget_refuses_a_binding_before_reserving_a_slot() {
        let pool = MaintenanceClients::with_engine_for_test(
            test_engine().await,
            config(32, 1, Duration::from_secs(300)),
        );

        let observed = pool.acquire(key("tns_metadata_budget", "memory")).await;

        assert!(observed.is_err());
        let snapshot = pool.snapshot();
        assert_eq!(snapshot.resident_sessions, 0);
        assert_eq!(snapshot.metadata_accounted_bytes, 0);
        assert_eq!(snapshot.bind_attempts, 0);
    }
}
