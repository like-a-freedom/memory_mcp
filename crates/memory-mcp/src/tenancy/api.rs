use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::MemoryError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRuntimeSpec {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
    pub plan_version: u32,
    pub schema_version: u32,
    /// The tenant's lifecycle status at the moment the spec was resolved.
    ///
    /// Carried so a runtime built for a tenant being deleted is assembled
    /// from that tenant's real state rather than an assumed `Ready`. A
    /// factory that cannot represent the status must not silently widen it.
    pub status: TenantLifecycleStatus,
}

/// A tenant's lifecycle status, as far as runtime construction is concerned.
///
/// Deliberately narrow: these are the three states in which a tenant has a
/// namespace that still holds data worth reaching. `Suspended`, `Failed` and
/// the provisioning states are request-path refusals and have no maintenance
/// reading; see [`ResolveTenantPort::resolve_tenant_for_maintenance`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TenantLifecycleStatus {
    #[default]
    Ready,
    Deleting,
    Purged,
}

#[derive(Debug)]
pub enum RuntimeFactoryError {
    Storage(MemoryError),
}

impl From<MemoryError> for RuntimeFactoryError {
    fn from(error: MemoryError) -> Self {
        Self::Storage(error)
    }
}

#[async_trait::async_trait]
pub trait TenantRuntimeFactory: Send + Sync {
    type Runtime: Send + Sync + 'static;

    async fn activate(&self, spec: TenantRuntimeSpec)
    -> Result<Self::Runtime, RuntimeFactoryError>;
}

pub struct RuntimeLease<R> {
    runtime: Arc<R>,
}

impl<R> RuntimeLease<R> {
    pub fn runtime(&self) -> &R {
        self.runtime.as_ref()
    }

    pub fn into_runtime(self) -> Arc<R> {
        self.runtime
    }
}

impl<R> Clone for RuntimeLease<R> {
    fn clone(&self) -> Self {
        Self {
            runtime: Arc::clone(&self.runtime),
        }
    }
}

struct Entry<R> {
    spec: TenantRuntimeSpec,
    runtime: Arc<R>,
    last_used: std::time::Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenantResolutionStatus {
    Reserved,
    NamespaceCreating,
    Migrating,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TenantResolution {
    Ready(TenantRuntimeSpec),
    Provisioning(TenantResolutionStatus),
    Suspended,
    Failed(String),
    NotFound,
}

/// A tenant that is bound and reachable, whatever its lifecycle status.
///
/// Distinct from [`TenantResolution`] on purpose. That type answers "may this
/// authenticated caller serve a request for this tenant?", and every arm but
/// [`TenantResolution::Ready`] is a refusal. A maintenance caller — the
/// deletion-recovery worker, the app-session sweeper — has a different question:
/// "which namespace is this tenant's data in, and is it still there?" A tenant
/// being deleted must answer that even though it must never serve a request.
///
/// Keeping the two apart is what stops the maintenance path from becoming a
/// way around the request-path refusals: [`resolve_tenant_runtime`] is
/// unchanged, so an HTTP request for a deleting tenant still fails.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantBinding {
    pub spec: TenantRuntimeSpec,
    /// The status the tenant was in when the binding was read. Carried for
    /// the caller's own policy, never used to grant access. Mirrors
    /// `spec.status`; kept as a field so a caller can read the status
    /// without reaching through the spec.
    pub status: TenantLifecycleStatus,
}

#[async_trait::async_trait]
pub trait ResolveTenantPort: Send + Sync {
    async fn resolve_tenant(&self, account_id: &str) -> Result<TenantResolution, MemoryError>;

    /// Bind a tenant by its own id for maintenance work, without the
    /// request-path status gate.
    ///
    /// A default of `Unsupported` keeps the trait implementable without this
    /// method: a resolver that can only speak the account-keyed request path
    /// says so rather than silently resolving a tenant it should not.
    async fn resolve_tenant_for_maintenance(
        &self,
        _tenant_id: &str,
    ) -> Result<TenantBinding, MemoryError> {
        Err(MemoryError::Unavailable(
            "tenant maintenance resolution is not supported by this resolver".into(),
        ))
    }
}

pub async fn resolve_tenant_runtime(
    resolver: &(impl ResolveTenantPort + ?Sized),
    account_id: &str,
) -> Result<TenantRuntimeSpec, MemoryError> {
    match resolver.resolve_tenant(account_id).await? {
        TenantResolution::Ready(spec) => Ok(spec),
        TenantResolution::NotFound => Err(MemoryError::NotFound("tenant not found".into())),
        TenantResolution::Suspended => Err(MemoryError::Auth("tenant suspended".into())),
        TenantResolution::Failed(tenant_id) => Err(MemoryError::Unavailable(format!(
            "tenant {tenant_id} failed"
        ))),
        TenantResolution::Provisioning(status) => Err(MemoryError::Unavailable(format!(
            "tenant provisioning: {status:?}"
        ))),
    }
}

type ActivationResult<R> = Result<Arc<R>, MemoryError>;
type ActivationSender<R> = tokio::sync::broadcast::Sender<ActivationResult<R>>;

/// Trusted tenant resolution and Tenant Runtime lifecycle.
///
/// `Tenancy` owns three guarantees, not a runtime cache:
///
/// 1. **Single-flight activation** — concurrent activations for one
///    tenant share one factory call (`activating`).
/// 2. **Immutable binding** — once a tenant's `TenantRuntimeSpec` is
///    resolved, a different spec for the same tenant is a conflict.
/// 3. **Resolved-spec bookkeeping** — `entries` remembers the spec and
///    keeps the runtime alive for the lifetime of a lease.
///
/// The runtime *pool* (`http::runtime::pool`) remains the single
/// authority for capacity and for serving warm requests. The pool must
/// call `evict_tenant` on every path that drops a slot, which keeps
/// `entries` a subset of the pool's LRU key set; the capacity check in
/// `activate` is therefore a backstop that cannot reject an activation
/// the pool had already made room for. That is what makes the pool's
/// `repeated_activation_cycles_within_capacity` test meaningful: a
/// stale entry left behind here would surface as an `ActivationFailed`
/// on a tenant the pool had just made room for.
pub struct Tenancy<F: TenantRuntimeFactory> {
    factory: Arc<F>,
    capacity: usize,
    idle_ttl: Duration,
    activation_timeout: Duration,
    entries: Mutex<HashMap<String, Entry<F::Runtime>>>,
    activating: Mutex<HashMap<String, ActivationSender<F::Runtime>>>,
}

impl<F: TenantRuntimeFactory> Tenancy<F> {
    pub fn new(
        factory: Arc<F>,
        capacity: usize,
        idle_ttl: Duration,
        activation_timeout: Duration,
    ) -> Self {
        Self {
            factory,
            capacity: capacity.max(1),
            idle_ttl,
            activation_timeout,
            entries: Mutex::new(HashMap::new()),
            activating: Mutex::new(HashMap::new()),
        }
    }

    pub fn factory(&self) -> &Arc<F> {
        &self.factory
    }

    pub async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<RuntimeLease<F::Runtime>, MemoryError> {
        if let Some(entry) = self
            .entries
            .lock()
            .expect("tenancy entries lock")
            .get_mut(&spec.tenant_id)
        {
            if entry.spec != spec {
                return Err(MemoryError::Conflict(
                    "tenant runtime binding is immutable".into(),
                ));
            }
            entry.last_used = std::time::Instant::now();
            return Ok(RuntimeLease {
                runtime: Arc::clone(&entry.runtime),
            });
        }

        self.evict_idle_if_needed().await?;
        let (is_leader, receiver) = {
            let mut activating = self.activating.lock().expect("tenancy activation lock");
            if let Some(sender) = activating.get(&spec.tenant_id) {
                (false, Some(sender.subscribe()))
            } else {
                let (sender, receiver) = tokio::sync::broadcast::channel(1);
                activating.insert(spec.tenant_id.clone(), sender);
                (true, Some(receiver))
            }
        };
        if !is_leader {
            return receiver
                .expect("activation receiver")
                .recv()
                .await
                .map_err(|_| {
                    MemoryError::Unavailable("tenant runtime activation cancelled".into())
                })?
                .map(|runtime| RuntimeLease { runtime });
        }

        let result = match tokio::time::timeout(
            self.activation_timeout,
            self.factory.activate(spec.clone()),
        )
        .await
        {
            Ok(Ok(runtime)) => {
                let mut entries = self.entries.lock().expect("tenancy entries lock");
                if entries.len() >= self.capacity {
                    Err(MemoryError::Unavailable(
                        "tenant runtime capacity exhausted".into(),
                    ))
                } else {
                    let runtime = Arc::new(runtime);
                    entries.insert(
                        spec.tenant_id.clone(),
                        Entry {
                            spec: spec.clone(),
                            runtime: Arc::clone(&runtime),
                            last_used: std::time::Instant::now(),
                        },
                    );
                    Ok(runtime)
                }
            }
            Ok(Err(RuntimeFactoryError::Storage(error))) => Err(error),
            Err(_) => Err(MemoryError::Unavailable(
                "tenant runtime activation timed out".into(),
            )),
        };
        let sender = self
            .activating
            .lock()
            .expect("tenancy activation lock")
            .remove(&spec.tenant_id);
        if let Some(sender) = sender {
            let _ = sender.send(result.clone());
        }
        result.map(|runtime| RuntimeLease { runtime })
    }

    pub fn evict_tenant(&self, tenant_id: &str) -> bool {
        self.entries
            .lock()
            .expect("tenancy entries lock")
            .remove(tenant_id)
            .is_some()
    }

    /// Evict runtimes idle for longer than the configured TTL.
    pub async fn evict_idle(&self) -> usize {
        let threshold = std::time::Instant::now().checked_sub(self.idle_ttl);
        let mut entries = self.entries.lock().expect("tenancy entries lock");
        let before = entries.len();
        entries.retain(|_, entry| !threshold.is_some_and(|threshold| entry.last_used <= threshold));
        before - entries.len()
    }

    /// Make room for one more binding before the factory runs. Without
    /// this, a cache full of recently-used bindings would reject the
    /// activation outright instead of letting the pool reclaim an idle
    /// one first.
    async fn evict_idle_if_needed(&self) -> Result<(), MemoryError> {
        let at_capacity = self.entries.lock().expect("tenancy entries lock").len() >= self.capacity;
        if at_capacity {
            self.evict_idle().await;
        }
        Ok(())
    }
}
