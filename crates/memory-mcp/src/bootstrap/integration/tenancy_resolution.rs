use std::sync::Arc;

use crate::MemoryError;
use crate::http::registry::account::AccountResolver;
use crate::tenancy::api::{
    ResolveTenantPort, TenantResolution, TenantResolutionStatus, TenantRuntimeSpec,
};

pub(crate) struct LegacyTenantResolver {
    resolver: Arc<AccountResolver>,
}

impl LegacyTenantResolver {
    pub(crate) fn new(resolver: Arc<AccountResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait::async_trait]
impl ResolveTenantPort for LegacyTenantResolver {
    async fn resolve_tenant(&self, account_id: &str) -> Result<TenantResolution, MemoryError> {
        Ok(
            match self.resolver.resolve_ready_tenant(account_id).await? {
                crate::http::registry::account::ResolvedTenant::Ready(tenant) => {
                    TenantResolution::Ready(TenantRuntimeSpec {
                        tenant_id: tenant.id,
                        namespace: tenant.namespace_binding.namespace,
                        database: tenant.namespace_binding.database,
                        plan_version: tenant.plan_version,
                        schema_version: tenant.schema_version,
                    })
                }
                crate::http::registry::account::ResolvedTenant::Provisioning(status, _) => {
                    TenantResolution::Provisioning(match status {
                        crate::http::registry::models::TenantStatus::Reserved => {
                            TenantResolutionStatus::Reserved
                        }
                        crate::http::registry::models::TenantStatus::NamespaceCreating => {
                            TenantResolutionStatus::NamespaceCreating
                        }
                        crate::http::registry::models::TenantStatus::Migrating => {
                            TenantResolutionStatus::Migrating
                        }
                        other => {
                            return Err(MemoryError::Unavailable(format!(
                                "unexpected provisioning status {other:?}"
                            )));
                        }
                    })
                }
                crate::http::registry::account::ResolvedTenant::Suspended => {
                    TenantResolution::Suspended
                }
                crate::http::registry::account::ResolvedTenant::Failed(tenant_id) => {
                    TenantResolution::Failed(tenant_id)
                }
                crate::http::registry::account::ResolvedTenant::NotFound => {
                    TenantResolution::NotFound
                }
            },
        )
    }
}
