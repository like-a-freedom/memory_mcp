use std::sync::Arc;

use crate::http::registry::RegistryHandle;
use crate::http::registry::models::{Tenant, TenantStatus};
use crate::http::runtime::storage::{RuntimeOptions, TenantRuntime, build_runtime_with_options};
use crate::tenancy::api::{
    RuntimeFactoryError, TenantLifecycleStatus, TenantRuntimeFactory, TenantRuntimeSpec,
};

/// Adapts a resolved [`TenantRuntimeSpec`] onto [`TenantRuntimeFactory`].
///
/// The name says registry because the *source* of the tenant record is the
/// control registry. The runtime it builds is not a legacy path: it is the only
/// way a tenant runtime is assembled, used by the request path and the
/// maintenance path alike. It is an adapter rather than inline logic because the
/// port is what lets `http/runtime` acquire a tenant without holding a registry
/// handle of its own.
pub(crate) struct RegistryTenantRuntimeFactory {
    registry: Arc<RegistryHandle>,
    options: std::sync::RwLock<RuntimeOptions>,
}

impl RegistryTenantRuntimeFactory {
    pub(crate) fn new(registry: Arc<RegistryHandle>, options: RuntimeOptions) -> Self {
        Self {
            registry,
            options: std::sync::RwLock::new(options),
        }
    }

    pub(crate) fn set_options(&self, options: RuntimeOptions) {
        *self.options.write().expect("runtime options lock") = options;
    }
}

#[async_trait::async_trait]
impl TenantRuntimeFactory for RegistryTenantRuntimeFactory {
    type Runtime = TenantRuntime;

    async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<Self::Runtime, RuntimeFactoryError> {
        // The status comes from the spec rather than an assumed `Ready`. A
        // maintenance caller binds a tenant that is being deleted, and a
        // runtime assembled from a fabricated `Ready` record would be a lie
        // the stores below could act on.
        let status = match spec.status {
            TenantLifecycleStatus::Ready => TenantStatus::Ready,
            TenantLifecycleStatus::Deleting => TenantStatus::Deleting,
            TenantLifecycleStatus::Purged => TenantStatus::Purged,
        };
        let tenant = Tenant {
            id: spec.tenant_id,
            status,
            namespace_binding: crate::http::registry::models::NamespaceBinding {
                namespace: spec.namespace,
                database: spec.database,
            },
            plan_version: spec.plan_version,
            schema_version: spec.schema_version,
            retry_stage: None,
            provisioning_lease: None,
            created_at: chrono::Utc::now(),
            version: 0,
        };
        let options = self.options.read().expect("runtime options lock").clone();
        build_runtime_with_options(&self.registry, &tenant, options)
            .await
            .map_err(RuntimeFactoryError::Storage)
    }
}
