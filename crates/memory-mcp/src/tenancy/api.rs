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

/// The part of a runtime specification that must not change while a runtime is
/// resident: which tenant, and which storage it is bound to.
///
/// Deliberately narrower than the whole spec. Plan version, schema version and
/// concurrency describe the *runtime revision*, and a change there is a
/// replacement rather than a conflict; lifecycle status describes permission,
/// which the request path resolves every time and which is therefore never a
/// reason to reuse a runtime or to refuse one. See ADR-0072.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantRuntimeIdentity {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
}

impl TenantRuntimeSpec {
    pub fn identity(&self) -> TenantRuntimeIdentity {
        TenantRuntimeIdentity {
            tenant_id: self.tenant_id.clone(),
            namespace: self.namespace.clone(),
            database: self.database.clone(),
        }
    }
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
