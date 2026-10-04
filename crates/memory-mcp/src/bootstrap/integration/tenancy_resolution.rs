use std::sync::Arc;

use crate::MemoryError;
use crate::http::registry::account::AccountResolver;
use crate::tenancy::api::{
    ResolveTenantPort, TenantBinding, TenantLifecycleStatus, TenantResolution,
    TenantResolutionStatus, TenantRuntimeSpec,
};

/// Adapts the registry's account→tenant resolution onto [`ResolveTenantPort`].
///
/// One struct serves both methods the port declares, and they serve different
/// callers. `resolve_tenant` is the request path and refuses every tenant that
/// is not `Ready`, because a request must not reach a tenant mid-deletion.
/// `resolve_tenant_for_maintenance` is the privileged path the deletion-recovery
/// workflow uses, keyed by tenant id rather than account id, and admits
/// `Deleting` and `Purged`. They live together because they share the one
/// registry resolver — not because one tenant resolution serves both purposes.
pub(crate) struct RegistryTenantResolver {
    resolver: Arc<AccountResolver>,
}

impl RegistryTenantResolver {
    pub(crate) fn new(resolver: Arc<AccountResolver>) -> Self {
        Self { resolver }
    }
}

#[async_trait::async_trait]
impl ResolveTenantPort for RegistryTenantResolver {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{
        Account, AccountStatus, NamespaceBinding, Tenant, TenantStatus,
    };
    use crate::http::registry::storage::{AccountStore, InMemoryStore, TenantStore};
    use crate::http::registry::surreal_store::SurrealRegistryStore;
    use crate::tenancy::api::resolve_tenant_runtime;

    fn fixed_time() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-10-03T12:00:00Z")
            .expect("fixed timestamp")
            .with_timezone(&chrono::Utc)
    }

    /// A resolver over a store holding `acc_1` and its tenant in `status`.
    async fn resolver_with(status: TenantStatus) -> RegistryTenantResolver {
        let store = Arc::new(InMemoryStore::default());
        store
            .write_account(&Account {
                id: "acc_1".to_string(),
                status: AccountStatus::Active,
                tenant_id: "ten_1".to_string(),
                created_at: fixed_time(),
                display_name: None,
            })
            .await
            .expect("seed account");
        store
            .write_tenant(&Tenant {
                id: "ten_1".to_string(),
                status,
                namespace_binding: NamespaceBinding {
                    namespace: "tns_1".to_string(),
                    database: "memory".to_string(),
                },
                plan_version: 1,
                schema_version: 2,
                retry_stage: None,
                provisioning_lease: None,
                created_at: fixed_time(),
                version: 0,
            })
            .await
            .expect("seed tenant");
        RegistryTenantResolver::new(Arc::new(AccountResolver::new(store)))
    }

    async fn surreal_resolver_with(status: TenantStatus) -> RegistryTenantResolver {
        let store = Arc::new(
            SurrealRegistryStore::connect_in_memory("tenancy_resolution", "registry")
                .await
                .expect("migrated registry"),
        );
        store
            .write_account(&Account {
                id: "acc_1".to_string(),
                status: AccountStatus::Active,
                tenant_id: "ten_1".to_string(),
                created_at: fixed_time(),
                display_name: None,
            })
            .await
            .expect("seed account");
        store
            .write_tenant(&Tenant {
                id: "ten_1".to_string(),
                status,
                namespace_binding: NamespaceBinding {
                    namespace: "tns_1".to_string(),
                    database: "memory".to_string(),
                },
                plan_version: 1,
                schema_version: 2,
                retry_stage: None,
                provisioning_lease: None,
                created_at: fixed_time(),
                version: 0,
            })
            .await
            .expect("seed tenant");
        let tenant_store: Arc<dyn TenantStore> = store;
        RegistryTenantResolver::new(Arc::new(AccountResolver::new(tenant_store)))
    }

    /// A resolver over a store that holds no tenants at all.
    fn resolver() -> RegistryTenantResolver {
        RegistryTenantResolver::new(Arc::new(AccountResolver::new(Arc::new(
            InMemoryStore::default(),
        ))))
    }

    #[tokio::test]
    async fn a_ready_tenant_resolves_for_the_request_path() {
        let resolution = resolve_with(TenantStatus::Ready).await;

        assert!(matches!(resolution, TenantResolution::Ready(_)));
    }

    #[tokio::test]
    async fn a_reserved_tenant_resolves_as_reserved() {
        let resolution = resolve_with(TenantStatus::Reserved).await;

        assert!(matches!(
            resolution,
            TenantResolution::Provisioning(TenantResolutionStatus::Reserved)
        ));
    }

    #[tokio::test]
    async fn a_namespace_creating_tenant_resolves_as_namespace_creating() {
        let resolution = resolve_with(TenantStatus::NamespaceCreating).await;

        assert!(matches!(
            resolution,
            TenantResolution::Provisioning(TenantResolutionStatus::NamespaceCreating)
        ));
    }

    #[tokio::test]
    async fn a_migrating_tenant_resolves_as_migrating() {
        let resolution = resolve_with(TenantStatus::Migrating).await;

        assert!(matches!(
            resolution,
            TenantResolution::Provisioning(TenantResolutionStatus::Migrating)
        ));
    }

    #[tokio::test]
    async fn a_suspended_tenant_resolves_as_suspended() {
        let resolution = resolve_with(TenantStatus::Suspended).await;

        assert!(matches!(resolution, TenantResolution::Suspended));
    }

    #[tokio::test]
    async fn a_failed_tenant_resolves_as_failed_with_its_id() {
        let observed = resolve_with(TenantStatus::Failed).await;

        match observed {
            TenantResolution::Failed(tenant_id) => assert_eq!(tenant_id, "ten_1"),
            other => panic!("expected a failed resolution, got {other:?}"),
        }
    }

    /// Integration of the actual registry adapter and owner resolver: the
    /// maintenance binding reaches the deleting row while request resolution
    /// refuses that same row.
    #[tokio::test]
    async fn a_deleting_tenant_is_maintenance_visible_but_refused_to_requests() {
        // Deleting is not a provisioning status, so the request path refuses it
        // rather than resolving it — a request must not reach a tenant that is
        // mid-deletion.
        let resolver = surreal_resolver_with(TenantStatus::Deleting).await;

        let binding = resolver
            .resolve_tenant_for_maintenance("ten_1")
            .await
            .expect("maintenance can bind the deleting tenant");
        assert_eq!(binding.status, TenantLifecycleStatus::Deleting);
        assert_eq!(binding.spec.namespace, "tns_1");

        let request = resolve_tenant_runtime(&resolver, "acc_1").await;
        assert!(
            request.is_err(),
            "the request path must still refuse deletion"
        );
    }

    #[tokio::test]
    async fn an_account_with_no_tenant_resolves_as_not_found() {
        let resolver = resolver();

        let observed = resolver.resolve_tenant("acc_missing").await;

        assert!(matches!(
            observed.expect("resolution succeeds"),
            TenantResolution::NotFound
        ));
    }

    #[tokio::test]
    async fn a_purged_tenant_is_visible_to_the_maintenance_path() {
        let resolver = resolver_with(TenantStatus::Purged).await;

        let observed = resolver
            .resolve_tenant_for_maintenance("ten_1")
            .await
            .expect("maintenance resolves");

        assert_eq!(observed.status, TenantLifecycleStatus::Purged);
    }

    #[tokio::test]
    async fn a_suspended_tenant_is_not_visible_to_the_maintenance_path() {
        // The split exists so a suspended tenant cannot be reached by
        // inventing a maintenance reading; it must be refused outright.
        let resolver = resolver_with(TenantStatus::Suspended).await;

        let observed = resolver.resolve_tenant_for_maintenance("ten_1").await;

        assert!(observed.is_err());
    }

    #[tokio::test]
    async fn a_reserved_tenant_is_not_visible_to_the_maintenance_path() {
        let resolver = resolver_with(TenantStatus::Reserved).await;

        let observed = resolver.resolve_tenant_for_maintenance("ten_1").await;

        assert!(
            observed.is_err(),
            "a tenant with no data to sweep is refused"
        );
    }

    #[tokio::test]
    async fn maintenance_resolution_of_an_unknown_tenant_is_a_not_found() {
        let resolver = resolver();

        let observed = resolver.resolve_tenant_for_maintenance("ten_missing").await;

        assert!(matches!(observed, Err(MemoryError::NotFound(_))));
    }

    /// Resolve the account that owns a tenant in `status` on the request path.
    async fn resolve_with(status: TenantStatus) -> TenantResolution {
        resolver_with(status)
            .await
            .resolve_tenant("acc_1")
            .await
            .expect("resolution succeeds")
    }
}
