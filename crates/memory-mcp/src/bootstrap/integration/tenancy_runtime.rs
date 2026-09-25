use std::sync::Arc;

use crate::http::registry::RegistryHandle;
use crate::http::registry::models::Tenant;
use crate::http::runtime::storage::{RuntimeOptions, TenantRuntime, build_runtime_with_options};
use crate::tenancy::api::{RuntimeFactoryError, TenantRuntimeFactory, TenantRuntimeSpec};

pub(crate) struct LegacyTenantRuntimeFactory {
    registry: Arc<RegistryHandle>,
    options: std::sync::RwLock<RuntimeOptions>,
}

impl LegacyTenantRuntimeFactory {
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
impl TenantRuntimeFactory for LegacyTenantRuntimeFactory {
    type Runtime = TenantRuntime;

    async fn activate(
        &self,
        spec: TenantRuntimeSpec,
    ) -> Result<Self::Runtime, RuntimeFactoryError> {
        let tenant = Tenant {
            id: spec.tenant_id,
            status: crate::http::registry::models::TenantStatus::Ready,
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
