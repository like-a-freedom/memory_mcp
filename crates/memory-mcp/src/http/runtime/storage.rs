//! Tenant Runtime contents (ADR-0052, plan §5.4).
//!
//! The runtime is the per-Tenant bundle: a tenant-bound
//! SurrealDB client, a `MemoryService` (and the modern
//! `MemoryMcp` handler built from it), a `BoundDbClient` for
//! namespace-free adapters, and a creation timestamp the
//! pool uses for idle eviction. The construction rule is
//! "clone once, bind once" — see `build_runtime`.

use std::sync::Arc;
use std::time::Instant;

use crate::error::MemoryError;
use crate::http::registry::models::Tenant;
use crate::mcp::handlers::MemoryMcp;
use crate::storage::client::BoundDbClient;
use crate::storage::client::SurrealDbClient;

/// Per-tenant runtime bundle. Lives in the LRU pool (Task
/// 5.5); the `mcp_service` is the per-request dispatch
/// target once the HTTP pipeline has resolved and acquired
/// the runtime.
pub struct TenantRuntime {
    pub tenant_id: String,
    pub namespace: String,
    pub database: String,
    pub schema_version: u32,
    /// Tenant-bound SurrealDB client. Acquired by cloning the
    /// privileged raw handle and calling `use_ns(...).use_db(...)`
    /// exactly once at build time; the resulting adapter is
    /// never rebound.
    pub tenant_db: Arc<SurrealDbClient>,
    /// Namespace-free adapter for App Sessions, the outbox,
    /// and other tenant stores. Always delegates with this
    /// runtime's immutable namespace.
    pub bound_db: Arc<BoundDbClient>,
    pub mcp_service: MemoryMcp,
    pub created_at: Instant,
}

impl TenantRuntime {
    /// Construct a runtime from a pre-bound `SurrealDbClient`
    /// and a `Tenant` row. Used by both `build_runtime` (Task
    /// 5.4) and tests.
    pub fn from_bound_client(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
    ) -> Result<Self, MemoryError> {
        let namespace = tenant.namespace_binding.namespace.clone();
        let database = tenant.namespace_binding.database.clone();
        let bound_db = Arc::new(BoundDbClient::new(tenant_db.clone(), namespace.clone()));
        let service = crate::service::MemoryService::new(
            tenant_db.clone(),
            namespace.clone(),
            "info".into(),
            100, // rate_limit_rps; plan-driven value arrives with quotas (Task 6.4)
            100, // rate_limit_burst
        )?;
        let mut mcp_service = MemoryMcp::new_modern(service);

        // Wire durable backends. The stdio path never reaches
        // this function (it uses the test pool), so the
        // feature-gated overlay is unconditional here.
        {
            #[cfg(feature = "mcp-apps")]
            {
                use crate::http::app_sessions::store::AppSessionStore;
                mcp_service = mcp_service
                    .with_durable_app_sessions(Arc::new(AppSessionStore::new(bound_db.clone())));
            }
            mcp_service = mcp_service.with_durable_tasks(Arc::new(
                crate::http::tasks::worker::DurableTaskStore::new(
                    bound_db.clone(),
                    tenant.id.clone(),
                ),
            ));
        }
        Ok(Self {
            tenant_id: tenant.id.clone(),
            namespace,
            database,
            schema_version: tenant.schema_version,
            tenant_db,
            bound_db,
            mcp_service,
            created_at: Instant::now(),
        })
    }
}

/// Build a runtime by cloning the privileged `Surreal<C>`
/// handle from the registry, binding it once, and wrapping
/// the result. The engine variant determines which
/// `from_prebound*` constructor to call.
pub async fn build_runtime(
    registry: &super::super::registry::RegistryHandle,
    tenant: &Tenant,
) -> Result<TenantRuntime, MemoryError> {
    let tenant_db = match registry.tenant_engine() {
        super::super::registry::PrivilegedEngine::Remote(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound_remote(
                ns_client,
                &tenant.namespace_binding.namespace,
                "info",
            ))
        }
        super::super::registry::PrivilegedEngine::Local(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound(
                ns_client,
                &tenant.namespace_binding.namespace,
                "info",
            ))
        }
        super::super::registry::PrivilegedEngine::LocalMem(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound_mem(
                ns_client,
                &tenant.namespace_binding.namespace,
                "info",
            ))
        }
    };
    TenantRuntime::from_bound_client(tenant, tenant_db)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::registry::models::{NamespaceBinding, Tenant, TenantStatus};
    use chrono::Utc;
    use surrealdb::Surreal;
    use surrealdb::engine::local::Mem;

    fn tenant(id: &str, namespace: &str) -> Tenant {
        Tenant {
            id: id.to_string(),
            status: TenantStatus::Ready,
            namespace_binding: NamespaceBinding {
                namespace: namespace.to_string(),
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

    #[tokio::test]
    async fn prebound_client_rejects_foreign_namespace_queries() {
        use crate::storage::client::DbClient;
        // The pre-bound client's ensure_active_namespace guard
        // is the proof that no re-binding happens: a query
        // naming another namespace must fail.
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("tenant_a").use_db("memory").await.unwrap();
        let client = SurrealDbClient::from_prebound_mem(db, "tenant_a", "error");
        let r = DbClient::select_one(&client, "fact:x", "tenant_b").await;
        assert!(matches!(
            r,
            Err(crate::error::MemoryError::ConfigInvalid(_))
        ));
    }

    #[tokio::test]
    async fn two_runtimes_have_independent_db_handles() {
        // Build two runtimes with two distinct namespaces; the
        // wrapped `SurrealDbClient` handles must be distinct.
        let db_a = Surreal::new::<Mem>(()).await.unwrap();
        db_a.use_ns("tenant_a").use_db("memory").await.unwrap();
        let db_b = Surreal::new::<Mem>(()).await.unwrap();
        db_b.use_ns("tenant_b").use_db("memory").await.unwrap();
        let c_a = Arc::new(SurrealDbClient::from_prebound_mem(
            db_a, "tenant_a", "error",
        ));
        let c_b = Arc::new(SurrealDbClient::from_prebound_mem(
            db_b, "tenant_b", "error",
        ));
        let r_a = TenantRuntime::from_bound_client(&tenant("ten_a", "tenant_a"), c_a).unwrap();
        let r_b = TenantRuntime::from_bound_client(&tenant("ten_b", "tenant_b"), c_b).unwrap();
        assert_ne!(r_a.namespace, r_b.namespace);
        assert!(!Arc::ptr_eq(&r_a.tenant_db, &r_b.tenant_db));
    }
}
