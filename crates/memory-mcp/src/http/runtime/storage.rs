//! Tenant Runtime contents.
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

/// Options that are fixed for a process and copied into each tenant runtime.
#[derive(Debug, Clone, Copy)]
pub struct RuntimeOptions {
    pub task_retention_secs: i64,
    pub task_queue_capacity: usize,
    pub task_sync_max_bytes: usize,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            task_retention_secs: crate::http::config::DEFAULT_TASK_RETENTION_SECS as i64,
            task_queue_capacity: crate::http::config::DEFAULT_TASK_QUEUE_CAPACITY,
            task_sync_max_bytes: crate::http::config::DEFAULT_TASK_SYNC_MAX_BYTES,
        }
    }
}

impl RuntimeOptions {
    pub fn from_http_config(config: &crate::http::config::HttpConfig) -> Self {
        Self {
            task_retention_secs: i64::try_from(config.task_retention_secs).unwrap_or(i64::MAX),
            task_queue_capacity: config.task_queue_capacity,
            task_sync_max_bytes: config.task_sync_max_bytes,
        }
    }
}

/// Per-tenant runtime bundle. Lives in the LRU pool; the
/// `mcp_service` is the per-request dispatch target once the
/// HTTP pipeline has resolved and acquired the runtime.
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
    /// and a `Tenant` row. Used by both `build_runtime` and tests.
    pub fn from_bound_client(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
    ) -> Result<Self, MemoryError> {
        Self::from_bound_client_with_runtime_options(
            tenant,
            tenant_db,
            crate::http::registry::plan::Plan::default(),
            RuntimeOptions::default(),
        )
    }

    /// Construct a runtime with the immutable plan selected during activation.
    /// The plan is copied into the MCP adapter so app/task admission cannot
    /// silently fall back to a process-wide constant.
    pub fn from_bound_client_with_plan(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
        plan: crate::http::registry::plan::Plan,
    ) -> Result<Self, MemoryError> {
        Self::from_bound_client_with_runtime_options(
            tenant,
            tenant_db,
            plan,
            RuntimeOptions::default(),
        )
    }

    pub fn from_bound_client_with_runtime_options(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
        plan: crate::http::registry::plan::Plan,
        options: RuntimeOptions,
    ) -> Result<Self, MemoryError> {
        let namespace = tenant.namespace_binding.namespace.clone();
        let database = tenant.namespace_binding.database.clone();
        let bound_db = Arc::new(BoundDbClient::new(tenant_db.clone(), namespace.clone()));
        let service = crate::service::MemoryService::new(
            tenant_db.clone(),
            namespace.clone(),
            "info".into(),
            100, // rate_limit_rps; access-payload limiter remains separate
            100, // rate_limit_burst
        )?
        .with_http_outbox();
        let mut mcp_service = MemoryMcp::new_modern(service)
            .with_tenant_id(tenant.id.clone())
            .with_tenant_plan(plan)
            .with_task_sync_max_bytes(options.task_sync_max_bytes);

        // Wire durable backends. The stdio path never reaches
        // this function (it uses the test pool), so the
        // feature-gated overlay is unconditional here.
        {
            #[cfg(feature = "mcp-apps")]
            {
                use crate::http::app_sessions::store::AppSessionStore;
                mcp_service = mcp_service.with_durable_app_sessions(Arc::new(
                    AppSessionStore::new(bound_db.clone()).with_outbox(),
                ));
            }
            mcp_service = mcp_service.with_durable_tasks(Arc::new(
                crate::http::tasks::worker::DurableTaskStore::new_with_options(
                    bound_db.clone(),
                    tenant.id.clone(),
                    options.task_retention_secs,
                    options.task_queue_capacity,
                ),
            ));
            mcp_service = mcp_service.with_durable_subscriptions(Arc::new(
                crate::http::subscriptions::DurableSubscriptionStore::new(
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
    build_runtime_with_options(registry, tenant, RuntimeOptions::default()).await
}

pub async fn build_runtime_with_options(
    registry: &super::super::registry::RegistryHandle,
    tenant: &Tenant,
    options: RuntimeOptions,
) -> Result<TenantRuntime, MemoryError> {
    let plan = crate::http::registry::plan::Plan::from(
        &registry
            .store_clone()
            .load_plan(tenant.plan_version)
            .await?,
    );
    let tenant_db = match registry.tenant_engine()? {
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
    TenantRuntime::from_bound_client_with_runtime_options(tenant, tenant_db, plan, options)
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
