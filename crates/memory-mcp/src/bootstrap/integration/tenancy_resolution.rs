use std::sync::Arc;

use crate::MemoryError;
use crate::http::registry::account::AccountResolver;
use crate::tenancy::api::{
    ResolveTenantPort, TenantBinding, TenantLifecycleStatus, TenantResolution,
    TenantResolutionStatus, TenantRuntimeSpec,
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
                        // Only `Ready` reaches this arm, by construction:
                        // every other status became one of the refusal arms
                        // below. Stating it rather than defaulting keeps the
                        // compiler honest if a new status is added upstream.
                        status: TenantLifecycleStatus::Ready,
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

    async fn resolve_tenant_for_maintenance(
        &self,
        tenant_id: &str,
    ) -> Result<TenantBinding, MemoryError> {
        let Some(tenant) = self
            .resolver
            .resolve_tenant_for_maintenance(tenant_id)
            .await?
        else {
            return Err(MemoryError::NotFound("tenant not found".into()));
        };
        let status = match tenant.status {
            crate::http::registry::models::TenantStatus::Ready => TenantLifecycleStatus::Ready,
            crate::http::registry::models::TenantStatus::Deleting => {
                TenantLifecycleStatus::Deleting
            }
            crate::http::registry::models::TenantStatus::Purged => TenantLifecycleStatus::Purged,
            // A suspended or failed tenant is not this method's business. It
            // is a request-path refusal, and inventing a maintenance reading
            // for it here would be exactly the back door the split exists to
            // close. Reserved/NamespaceCreating/Migrating have no data to
            // sweep.
            other => {
                return Err(MemoryError::Unavailable(format!(
                    "tenant {tenant_id} is not in a maintenance-visible status: {other:?}"
                )));
            }
        };
        Ok(TenantBinding {
            spec: TenantRuntimeSpec {
                tenant_id: tenant.id,
                namespace: tenant.namespace_binding.namespace,
                database: tenant.namespace_binding.database,
                plan_version: tenant.plan_version,
                schema_version: tenant.schema_version,
                status,
            },
            status,
        })
    }
}
