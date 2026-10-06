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

use crate::embedding::providers::{DisabledEmbeddingProvider, EmbeddingProvider};
use crate::error::MemoryError;
use crate::http::registry::models::Tenant;
use crate::knowledge::entity_extraction::EntityExtractor;
use crate::mcp::handlers::MemoryMcp;
use crate::platform::fault_injection::{FaultInjector, NoFaults};
use crate::service::startup::EmbeddingStartupDecision;
use crate::storage::client::BoundDbClient;
use crate::storage::client::SurrealDbClient;

/// Options that are fixed for a process and copied into each tenant runtime.
#[derive(Clone)]
pub struct RuntimeOptions {
    pub task_retention_secs: i64,
    pub task_queue_capacity: usize,
    pub task_sync_max_bytes: usize,
    /// The fault injector the scheduler copies into the task worker.
    /// Production uses [`NoFaults`]; tests substitute a `FailOnceAt`.
    pub fault_injector: Arc<dyn FaultInjector>,
    /// Deployment-level embedding policy shared by every tenant
    /// (spec §13: providers are deployment-level policy, not
    /// tenant configuration).
    ///
    /// `None` means semantic retrieval is off for this process and
    /// every tenant gets [`DisabledEmbeddingProvider`]. `Some` carries
    /// the provider resolved once by the composition root, its
    /// dimension, and the identity signature that tenant namespaces
    /// are reconciled against.
    pub embedding_policy: Option<EmbeddingPolicy>,
    /// Similarity threshold applied to the per-tenant service.
    pub embedding_similarity_threshold: f64,
    /// Entity extractor resolved once by the composition root and
    /// shared across tenants. `None` means the tenant root falls back
    /// to its built-in extractor.
    pub entity_extractor: Option<Arc<dyn EntityExtractor>>,
    /// Lifecycle policy copied into each tenant service.
    pub lifecycle_config: crate::config::LifecycleConfig,
}

/// The deployment-level embedding identity one tenant runtime starts from.
///
/// The provider itself is process-global (spec §7.1 lists provider
/// connection pools as shareable), but which provider a given tenant may
/// use is a per-namespace decision: [`resolve_namespace_embedding_decision`]
/// reads each namespace's stored `embedding_state` and may downgrade a
/// tenant to [`DisabledEmbeddingProvider`] without touching the shared
/// provider.
#[derive(Clone)]
pub struct EmbeddingPolicy {
    pub provider: Arc<dyn EmbeddingProvider>,
    pub dimension: usize,
    pub signature: String,
    pub model: Option<String>,
    pub provider_label: &'static str,
}

impl std::fmt::Debug for EmbeddingPolicy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbeddingPolicy")
            .field("provider_label", &self.provider_label)
            .field("dimension", &self.dimension)
            .field("signature", &self.signature)
            .field("model", &self.model)
            .finish()
    }
}

impl std::fmt::Debug for RuntimeOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RuntimeOptions")
            .field("task_retention_secs", &self.task_retention_secs)
            .field("task_queue_capacity", &self.task_queue_capacity)
            .field("task_sync_max_bytes", &self.task_sync_max_bytes)
            .field("fault_injector", &"<dyn FaultInjector>")
            .field("embedding_policy", &self.embedding_policy)
            .field(
                "embedding_similarity_threshold",
                &self.embedding_similarity_threshold,
            )
            .field("entity_extractor", &self.entity_extractor.is_some())
            .field("lifecycle_config", &self.lifecycle_config)
            .finish()
    }
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            task_retention_secs: crate::http::config::DEFAULT_TASK_RETENTION_SECS as i64,
            task_queue_capacity: crate::http::config::DEFAULT_TASK_QUEUE_CAPACITY,
            task_sync_max_bytes: crate::http::config::DEFAULT_TASK_SYNC_MAX_BYTES,
            fault_injector: Arc::new(NoFaults),
            embedding_policy: None,
            embedding_similarity_threshold: crate::config::DEFAULT_EMBEDDING_SIMILARITY_THRESHOLD,
            entity_extractor: None,
            lifecycle_config: crate::config::LifecycleConfig::default(),
        }
    }
}

impl RuntimeOptions {
    pub fn from_http_config(config: &crate::http::config::HttpConfig) -> Self {
        // `HttpConfig::validate` constrains `task_retention_secs`
        // to values representable in `i64`; the saturating cast
        // would otherwise silently turn any oversight into a
        // 68-year retention window. Surface the violation loudly
        // so the regression is visible.
        let task_retention_secs = i64::try_from(config.task_retention_secs)
            .expect("HttpConfig::validate bounds task_retention_secs within i64::MAX");
        Self {
            task_retention_secs,
            task_queue_capacity: config.task_queue_capacity,
            task_sync_max_bytes: config.task_sync_max_bytes,
            fault_injector: Arc::new(NoFaults),
            ..Self::default()
        }
    }

    /// Override the fault injector. The binary uses this in
    /// startup so the composition-owned injector reaches every
    /// scheduler option object (ADR-0053).
    pub fn with_fault_injector(mut self, injector: Arc<dyn FaultInjector>) -> Self {
        self.fault_injector = injector;
        self
    }

    /// Install the deployment-level embedding policy. Until the
    /// composition root calls this, every tenant runtime is built
    /// with a disabled provider — the state this wiring exists to
    /// eliminate.
    pub fn with_embedding_policy(mut self, policy: EmbeddingPolicy) -> Self {
        self.embedding_policy = Some(policy);
        self
    }

    pub fn with_entity_extractor(mut self, extractor: Arc<dyn EntityExtractor>) -> Self {
        self.entity_extractor = Some(extractor);
        self
    }

    pub fn with_lifecycle_config(
        mut self,
        lifecycle_config: crate::config::LifecycleConfig,
    ) -> Self {
        self.lifecycle_config = lifecycle_config;
        self
    }

    /// The dimension every tenant client's migrations must render,
    /// or `None` when this deployment has no embedding policy and
    /// tenants should keep the historical default.
    pub fn fact_embedding_dimension(&self) -> Option<usize> {
        self.embedding_policy
            .as_ref()
            .map(|policy| policy.dimension)
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

/// What one tenant namespace resolved to: the provider it may use, and the
/// identity that provider writes into stored facts.
struct TenantEmbedding {
    provider: Arc<dyn EmbeddingProvider>,
    signature: Option<String>,
    model: Option<String>,
    dimension: Option<usize>,
}

/// Decide which provider a single tenant namespace serves requests with.
///
/// Returns the shared deployment provider when the namespace's stored
/// embedding state is compatible with it, and a disabled provider of the
/// deployment's dimension otherwise. A namespace read failure degrades the
/// tenant rather than failing activation: the tenant must still serve
/// lexical retrieval, and `DisabledEmbeddingProvider` is the only answer
/// that cannot write a vector the index would reject.
async fn resolve_tenant_embedding(
    options: &RuntimeOptions,
    bound_db: &BoundDbClient,
    namespace: &str,
) -> TenantEmbedding {
    let Some(policy) = options.embedding_policy.as_ref() else {
        // No deployment-level policy: semantic retrieval is off process-wide.
        return TenantEmbedding {
            provider: Arc::new(DisabledEmbeddingProvider::new(
                crate::config::DEFAULT_EMBEDDING_DIMENSION,
            )),
            signature: None,
            model: None,
            dimension: None,
        };
    };
    let target = crate::embedding::providers::ResolvedEmbeddingTarget {
        provider_label: policy.provider_label,
        model: policy.model.clone(),
        dimension: policy.dimension,
        signature: policy.signature.clone(),
    };
    let decision = match crate::service::startup::resolve_namespace_embedding_decision(
        bound_db, namespace, &target,
    )
    .await
    {
        Ok(decision) => decision,
        Err(err) => {
            log_tenant_embedding_degraded(namespace, &policy.signature, &err.to_string());
            EmbeddingStartupDecision::DisableSemantic {
                reason: format!("tenant embedding state could not be read: {err}"),
            }
        }
    };
    log_tenant_embedding_decision(namespace, &decision, &policy.signature);
    match decision {
        EmbeddingStartupDecision::UseConfiguredProvider
        | EmbeddingStartupDecision::ResumePendingBackfill { .. }
        | EmbeddingStartupDecision::BootstrapReadyNamespace { .. } => TenantEmbedding {
            provider: policy.provider.clone(),
            signature: Some(policy.signature.clone()),
            model: policy.model.clone(),
            dimension: Some(policy.dimension),
        },
        // The namespace's stored vectors disagree with the deployment
        // provider. Serving it anyway would write vectors the tenant's
        // HNSW index cannot accept, so the tenant degrades to lexical.
        EmbeddingStartupDecision::RecoverMissingEmbeddings { .. }
        | EmbeddingStartupDecision::DisableSemantic { .. } => TenantEmbedding {
            provider: Arc::new(DisabledEmbeddingProvider::new(policy.dimension)),
            signature: None,
            model: None,
            dimension: Some(policy.dimension),
        },
    }
}

fn log_tenant_embedding_decision(
    namespace: &str,
    decision: &EmbeddingStartupDecision,
    target_signature: &str,
) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.tenant_embedding_decision"),
    );
    event.insert("namespace".to_string(), serde_json::json!(namespace));
    event.insert(
        "decision".to_string(),
        serde_json::json!(format!("{decision:?}")),
    );
    event.insert(
        "target_signature".to_string(),
        serde_json::json!(target_signature),
    );
    crate::logging::StdoutLogger::from_env().log(event, crate::logging::LogLevel::Info);
}

fn log_tenant_embedding_degraded(namespace: &str, target_signature: &str, reason: &str) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.tenant_embedding_read_failed"),
    );
    event.insert("namespace".to_string(), serde_json::json!(namespace));
    event.insert(
        "target_signature".to_string(),
        serde_json::json!(target_signature),
    );
    event.insert("reason".to_string(), serde_json::json!(reason));
    crate::logging::StdoutLogger::from_env().log(event, crate::logging::LogLevel::Warn);
}

impl TenantRuntime {
    /// Construct a runtime from a pre-bound `SurrealDbClient`
    /// and a `Tenant` row. Used by both `build_runtime` and tests.
    pub async fn from_bound_client(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
    ) -> Result<Self, MemoryError> {
        Self::from_bound_client_with_runtime_options(
            tenant,
            tenant_db,
            crate::operations::quota::QuotaPlan::default(),
            RuntimeOptions::default(),
        )
        .await
    }

    /// Construct a runtime with the immutable plan selected during activation.
    /// The plan is copied into the MCP adapter so app/task admission cannot
    /// silently fall back to a process-wide constant.
    pub async fn from_bound_client_with_plan(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
        plan: crate::operations::quota::QuotaPlan,
    ) -> Result<Self, MemoryError> {
        Self::from_bound_client_with_runtime_options(
            tenant,
            tenant_db,
            plan,
            RuntimeOptions::default(),
        )
        .await
    }

    /// Build a tenant runtime with the deployment-level embedding policy.
    ///
    /// The namespace's own stored `embedding_state` decides whether this
    /// tenant may use the shared provider: a namespace whose vectors were
    /// written by a different provider signature is degraded to a
    /// [`DisabledEmbeddingProvider`] rather than served vectors the index
    /// cannot hold. That is the same rule `bootstrap::stdio` applies to the
    /// single Active Namespace, applied per tenant.
    pub async fn from_bound_client_with_runtime_options(
        tenant: &Tenant,
        tenant_db: Arc<SurrealDbClient>,
        plan: crate::operations::quota::QuotaPlan,
        options: RuntimeOptions,
    ) -> Result<Self, MemoryError> {
        let namespace = tenant.namespace_binding.namespace.clone();
        let database = tenant.namespace_binding.database.clone();
        let bound_db = Arc::new(BoundDbClient::new(tenant_db.clone(), namespace.clone()));
        // Propagate the composition-owned injector to the
        // per-tenant bound client so the outbox commit path
        // consults the same injector the scheduler does.
        // `Arc::try_unwrap` succeeds in the common path: this
        // function is the only `Arc` holder at this point and
        // the stores below clone from the rebound handle, so
        // the in-place mutation propagates to every clone. If
        // a pre-existing `Arc` holds the same `BoundDbClient`
        // the fallback clones the inner value; the old
        // outstanding `Arc`s keep their pre-injection state.
        // In practice the caller is the composition root, so
        // the pre-existing-`Arc` branch is not exercised, but
        // the comment records the invariant so a future
        // refactor does not silently lose the propagation.
        #[cfg(feature = "streamable-http")]
        let bound_db = {
            let injector = options.fault_injector.clone();
            let mut owned = Arc::try_unwrap(bound_db).unwrap_or_else(|arc| (*arc).clone());
            owned.set_fault_injector(injector);
            Arc::new(owned)
        };

        let embedding = resolve_tenant_embedding(&options, &bound_db, &namespace).await;
        let entity_extractor = match options.entity_extractor.clone() {
            Some(extractor) => extractor,
            None => Arc::new(crate::knowledge::entity_extraction::AnnoEntityExtractor::new()?)
                as Arc<dyn EntityExtractor>,
        };
        let mut service = crate::service::MemoryService::new_with_embedding_provider(
            tenant_db.clone(),
            namespace.clone(),
            crate::logging::StdoutLogger::directives_from_env(),
            100, // rate_limit_rps; access-payload limiter remains separate
            100, // rate_limit_burst
            embedding.provider.clone(),
            options.embedding_similarity_threshold,
            entity_extractor,
        )?
        .with_http_outbox();
        service.lifecycle_config = options.lifecycle_config.clone();
        service.replace_embedding_runtime_state(
            crate::embedding::runtime::EmbeddingRuntimeState::new(
                embedding.provider,
                embedding.signature,
                embedding.model,
                embedding.dimension,
            ),
        );
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
                crate::bootstrap::integration::durable_tasks::DurableTaskAdapter::new(Arc::new(
                    crate::http::tasks::worker::DurableTaskStore::new_with_options(
                        bound_db.clone(),
                        tenant.id.clone(),
                        options.task_retention_secs,
                        options.task_queue_capacity,
                    ),
                )),
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
///
/// This entry point is retained for compatibility but
/// accepts no options. Use `build_runtime_with_options`
/// when a task retention or queue capacity override is
/// needed.
#[deprecated(note = "use build_runtime_with_options")]
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
    let plan = crate::operations::quota::QuotaPlan::from(
        &registry.usage().load_plan(tenant.plan_version).await?,
    );
    // A tenant client's dimension decides what its migrations render for
    // `fact_embedding_hnsw`. When the deployment resolved a provider, that
    // must be the provider's dimension — a namespace indexed at 1536 cannot
    // hold the 2048-dimension vectors this deployment writes.
    let dimension = options
        .fact_embedding_dimension()
        .unwrap_or(crate::config::DEFAULT_EMBEDDING_DIMENSION);
    let tenant_db = match registry.tenant_engine()? {
        super::super::registry::PrivilegedEngine::Remote(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound_remote_with_dimension(
                ns_client,
                &tenant.namespace_binding.namespace,
                &crate::logging::StdoutLogger::directives_from_env(),
                dimension,
            ))
        }
        super::super::registry::PrivilegedEngine::Local(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound_with_dimension(
                ns_client,
                &tenant.namespace_binding.namespace,
                &crate::logging::StdoutLogger::directives_from_env(),
                dimension,
            ))
        }
        super::super::registry::PrivilegedEngine::LocalMem(privileged) => {
            let ns_client = (*privileged).clone();
            ns_client
                .use_ns(&tenant.namespace_binding.namespace)
                .use_db(&tenant.namespace_binding.database)
                .await
                .map_err(|err| MemoryError::Storage(format!("tenant bind failed: {err}")))?;
            Arc::new(SurrealDbClient::from_prebound_mem_with_dimension(
                ns_client,
                &tenant.namespace_binding.namespace,
                &crate::logging::StdoutLogger::directives_from_env(),
                dimension,
            ))
        }
    };
    // Reconcile the index for a namespace provisioned before this deployment's
    // dimension existed. A tenant indexed at a different width rejects every
    // embedding this provider writes, so the mismatch is repaired here, at
    // activation, where a schema change belongs — not on the request path.
    if let Some(policy) = options.embedding_policy.as_ref() {
        reconcile_tenant_index_dimension(&tenant_db, &tenant.namespace_binding.namespace, policy)
            .await;
    }
    TenantRuntime::from_bound_client_with_runtime_options(tenant, tenant_db, plan, options).await
}

/// Whether a vector may be written into a namespace at the deployment's
/// dimension.
///
/// This is the one question the activation path and the backfill tick must
/// answer the same way. They asked it independently at first, and the answers
/// disagreed: activation declined to re-declare an index that stood under
/// foreign vectors, while backfill went ahead and paid the provider for a write
/// the database then rejected — every tick, for every affected tenant, with the
/// job marked degraded each time. One function, two callers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexWriteGate {
    /// The index already reports the deployment's dimension.
    Matches,
    /// The index stood at another dimension and the namespace stored no
    /// vector, so it was re-declared. Writes are now safe.
    Redeclared,
    /// The index stands at another dimension and vectors exist at that width.
    /// Rewriting them is a reembed's work, not a backfill's.
    ForeignVectors,
    /// The index could not be read. The caller must not guess, because a wrong
    /// guess here is a write that fails after it has been paid for.
    Unreadable,
}

/// Re-define a tenant's fact index when it was provisioned at a dimension other
/// than the deployment's, and report whether writing is safe afterwards.
///
/// Best-effort by design: a namespace that cannot be inspected or re-defined
/// must still activate, because lexical retrieval does not depend on the index.
/// The tenant's embedding decision is separate and degrades on its own.
pub(crate) async fn reconcile_tenant_index_dimension(
    tenant_db: &Arc<SurrealDbClient>,
    namespace: &str,
    policy: &EmbeddingPolicy,
) -> IndexWriteGate {
    // The index vocabulary lives on `ReembedStoreClient`, the owner of the
    // reembed-owned DDL. `Arc<SurrealDbClient>` coerces to `Arc<dyn DbClient>`
    // here; a `&SurrealDbClient` could not, because the client is neither
    // `Clone` nor able to lend its engine.
    let store = crate::embedding::reembed_store::ReembedStoreClient::new(
        Arc::clone(tenant_db) as Arc<dyn crate::storage::client::DbClient>,
        namespace.to_string(),
    );
    match store.embedding_index_dimension().await {
        Ok(Some(existing)) if existing != policy.dimension => {
            // Only a namespace that stores no vector may have its index
            // re-declared at the deployment's width. Stored vectors at another
            // width are a reembed's business (ADR-0042): re-declaring here
            // would leave vectors that the new index cannot accept, stranding
            // the namespace in a state only an operator can exit. A
            // vector-less namespace may be re-declared by anyone, because
            // both the activation path and the backfill tick come through this
            // function — and the latter must fix the index itself when it
            // meets a ready tenant that was never activated.
            match store.count_stored_vectors().await {
                Ok(stored) if stored > 0 => {
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::json!("http.tenant_embedding_index_reconcile_skipped"),
                    );
                    event.insert("namespace".to_string(), serde_json::json!(namespace));
                    event.insert(
                        "existing_dimension".to_string(),
                        serde_json::json!(existing),
                    );
                    event.insert("dimension".to_string(), serde_json::json!(policy.dimension));
                    event.insert("stored_vectors".to_string(), serde_json::json!(stored));
                    crate::logging::StdoutLogger::from_env()
                        .log(event, crate::logging::LogLevel::Warn);
                    IndexWriteGate::ForeignVectors
                }
                Ok(_) => {
                    if let Err(err) = redefine_embedding_index(&store, policy.dimension).await {
                        log_index_reconcile(
                            namespace,
                            Some(existing),
                            policy.dimension,
                            &err.to_string(),
                        );
                        return IndexWriteGate::Unreadable;
                    }
                    let mut event = std::collections::HashMap::new();
                    event.insert(
                        "op".to_string(),
                        serde_json::json!("http.tenant_embedding_index_redefined"),
                    );
                    event.insert("namespace".to_string(), serde_json::json!(namespace));
                    event.insert(
                        "previous_dimension".to_string(),
                        serde_json::json!(existing),
                    );
                    event.insert("dimension".to_string(), serde_json::json!(policy.dimension));
                    crate::logging::StdoutLogger::from_env()
                        .log(event, crate::logging::LogLevel::Warn);
                    IndexWriteGate::Redeclared
                }
                Err(err) => {
                    log_index_reconcile(
                        namespace,
                        Some(existing),
                        policy.dimension,
                        &err.to_string(),
                    );
                    IndexWriteGate::Unreadable
                }
            }
        }
        Ok(_) => IndexWriteGate::Matches,
        Err(err) => {
            log_index_reconcile(namespace, None, policy.dimension, &err.to_string());
            IndexWriteGate::Unreadable
        }
    }
}

/// Drop and re-declare the fact index at `dimension`.
///
/// Removing an absent index is not an error here: the store reports it as
/// `IndexRemoval::AlreadyAbsent`, which is the state this leaves behind anyway.
async fn redefine_embedding_index(
    store: &crate::embedding::reembed_store::ReembedStoreClient,
    dimension: usize,
) -> Result<(), MemoryError> {
    store.remove_embedding_index().await?;
    store.define_embedding_index(dimension).await
}

fn log_index_reconcile(namespace: &str, existing: Option<usize>, target: usize, reason: &str) {
    let mut event = std::collections::HashMap::new();
    event.insert(
        "op".to_string(),
        serde_json::json!("http.tenant_embedding_index_reconcile_failed"),
    );
    event.insert("namespace".to_string(), serde_json::json!(namespace));
    event.insert(
        "existing_dimension".to_string(),
        serde_json::json!(existing),
    );
    event.insert("target_dimension".to_string(), serde_json::json!(target));
    event.insert("reason".to_string(), serde_json::json!(reason));
    crate::logging::StdoutLogger::from_env().log(event, crate::logging::LogLevel::Warn);
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
        let r_a = TenantRuntime::from_bound_client(&tenant("ten_a", "tenant_a"), c_a)
            .await
            .unwrap();
        let r_b = TenantRuntime::from_bound_client(&tenant("ten_b", "tenant_b"), c_b)
            .await
            .unwrap();
        assert_ne!(r_a.namespace, r_b.namespace);
        assert!(!Arc::ptr_eq(&r_a.tenant_db, &r_b.tenant_db));
    }

    /// The regression this file's wiring exists for: the HTTP tenant root
    /// used to call `MemoryService::new`, which hardcodes
    /// `DisabledEmbeddingProvider`. An operator who configured
    /// `EMBEDDINGS_*` on the HTTP profile got a service whose provider
    /// reported `disabled` no matter what the environment said. A tenant
    /// whose namespace is empty is exactly the `BootstrapReadyNamespace`
    /// case, so it must receive the deployment's real provider and
    /// dimension.
    #[tokio::test]
    async fn tenant_runtime_installs_the_configured_embedding_provider() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("tenant_embed").use_db("memory").await.unwrap();
        let client = Arc::new(SurrealDbClient::from_prebound_mem(
            db,
            "tenant_embed",
            "error",
        ));

        let signature = crate::config::build_embedding_signature(
            "openai-compatible",
            Some("nvidia/nemotron-3-embed-1b"),
            Some("https://integrate.api.nvidia.com/v1"),
            2048,
        );
        let policy = EmbeddingPolicy {
            provider: Arc::new(StaticEmbeddingProvider { dimension: 2048 }),
            dimension: 2048,
            signature,
            model: Some("nvidia/nemotron-3-embed-1b".to_string()),
            provider_label: "openai-compatible",
        };
        let options = RuntimeOptions::default().with_embedding_policy(policy);
        let runtime = TenantRuntime::from_bound_client_with_runtime_options(
            &tenant("ten_embed", "tenant_embed"),
            client,
            crate::operations::quota::QuotaPlan::default(),
            options,
        )
        .await
        .unwrap();

        let snapshot = runtime.mcp_service.service().embedding_runtime_snapshot();
        assert_eq!(snapshot.dimension, Some(2048));
        assert_eq!(snapshot.provider.provider_name(), "static-test-provider");
        assert!(snapshot.provider.is_enabled());
    }

    /// A namespace whose stored `embedding_state` names a different provider
    /// signature must not be handed the deployment's provider: its vectors
    /// were written by something else, and serving it would write vectors the
    /// tenant's index cannot hold. It degrades to lexical instead.
    #[tokio::test]
    async fn tenant_with_foreign_embedding_signature_degrades_instead_of_using_the_provider() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("tenant_stale").use_db("memory").await.unwrap();
        let raw = db.clone();
        let client = Arc::new(SurrealDbClient::from_prebound_mem(
            db,
            "tenant_stale",
            "error",
        ));

        // A namespace that already recorded a `ready` state under a
        // different signature, and holds a vector of the old width.
        raw.use_ns("tenant_stale").use_db("memory").await.unwrap();
        raw.query(
            "CREATE embedding_state:fact SET status = 'ready', active_signature = 'embsig:other', provider = 'ollama', model = 'other', dimension = 1536",
        )
        .await
        .unwrap();
        raw.query("CREATE fact:only SET content = 'x', embedding = [1.0, 2.0]")
            .await
            .unwrap();

        let policy = EmbeddingPolicy {
            provider: Arc::new(StaticEmbeddingProvider { dimension: 2048 }),
            dimension: 2048,
            signature: crate::config::build_embedding_signature(
                "openai-compatible",
                Some("nvidia/nemotron-3-embed-1b"),
                Some("https://integrate.api.nvidia.com/v1"),
                2048,
            ),
            model: Some("nvidia/nemotron-3-embed-1b".to_string()),
            provider_label: "openai-compatible",
        };
        let options = RuntimeOptions::default().with_embedding_policy(policy);
        let runtime = TenantRuntime::from_bound_client_with_runtime_options(
            &tenant("ten_stale", "tenant_stale"),
            client,
            crate::operations::quota::QuotaPlan::default(),
            options,
        )
        .await
        .unwrap();

        let snapshot = runtime.mcp_service.service().embedding_runtime_snapshot();
        assert!(
            !snapshot.provider.is_enabled(),
            "a namespace holding foreign vectors must not embed with the deployment provider"
        );
        assert_eq!(snapshot.signature, None);
        // The degradation is to the deployment's *width*, not to the historical
        // default: a 1536-width provider here would be the old hardcoded path,
        // and the namespace it would then write is indexed at a different width.
        assert_eq!(
            snapshot.provider.dimension(),
            2048,
            "a degraded tenant must keep the deployment's dimension"
        );
        assert_eq!(snapshot.dimension, Some(2048));
    }

    /// A tenant that already holds facts but no vectors at all can be served
    /// with the deployment's provider: an empty sample is a proof that no
    /// stored fact carries a vector, which is backfill work, not a migration.
    ///
    /// This is the state every existing tenant is in the moment an operator
    /// enables `EMBEDDINGS_*` on the HTTP profile: facts exist, `embedding` is
    /// unset, and there is no stored `embedding_state`. Degrading it instead —
    /// as this test originally asserted — served it lexically forever and named
    /// `reembed` as the exit, which is a command this binary does not have and
    /// a namespace with nothing to re-embed *from*. The provider is installed;
    /// the scheduler's backfill tick fills the gaps.
    #[tokio::test]
    async fn a_tenant_with_facts_but_no_vectors_serves_the_deployment_provider() {
        let db = Surreal::new::<Mem>(()).await.unwrap();
        db.use_ns("tenant_unembedded")
            .use_db("memory")
            .await
            .unwrap();
        let raw = db.clone();
        let client = Arc::new(SurrealDbClient::from_prebound_mem(
            db,
            "tenant_unembedded",
            "error",
        ));

        raw.query("CREATE fact:a SET content = 'alpha'")
            .await
            .unwrap();
        raw.query("CREATE fact:b SET content = 'beta'")
            .await
            .unwrap();

        let policy = EmbeddingPolicy {
            provider: Arc::new(StaticEmbeddingProvider { dimension: 2048 }),
            dimension: 2048,
            signature: crate::config::build_embedding_signature(
                "openai-compatible",
                Some("nvidia/nemotron-3-embed-1b"),
                Some("https://integrate.api.nvidia.com/v1"),
                2048,
            ),
            model: Some("nvidia/nemotron-3-embed-1b".to_string()),
            provider_label: "openai-compatible",
        };
        let options = RuntimeOptions::default().with_embedding_policy(policy);
        let runtime = TenantRuntime::from_bound_client_with_runtime_options(
            &tenant("ten_unembedded", "tenant_unembedded"),
            client,
            crate::operations::quota::QuotaPlan::default(),
            options,
        )
        .await
        .unwrap();

        let snapshot = runtime.mcp_service.service().embedding_runtime_snapshot();
        assert!(
            snapshot.provider.is_enabled(),
            "facts without vectors are backfill work, and only the deployment's provider can fill them"
        );
        assert_eq!(
            snapshot.dimension,
            Some(2048),
            "the gaps must be filled at the width the index was reconciled to"
        );
    }

    /// The tenant runtime's own loggers must obey the deployment's level.
    ///
    /// They used to be built from the literal `"info"` — the decision helper
    /// with `StdoutLogger::new("info")`, the service with a hardcoded
    /// `"info".into()` — so neither `RUST_LOG=error` nor a subsystem directive
    /// could reach them. Under a level of `error` a correctly-wired runtime
    /// emits nothing at Info, which is the whole assertion.
    #[tokio::test]
    async fn a_tenant_runtime_honors_the_deployment_log_level() {
        let sink = crate::logging::capture::install();

        let runtime = crate::logging::capture::with_level("warn", || async {
            let db = Surreal::new::<Mem>(()).await.unwrap();
            db.use_ns("tenant_lvl").use_db("memory").await.unwrap();
            let client = Arc::new(SurrealDbClient::from_prebound_mem(
                db,
                "tenant_lvl",
                "error",
            ));
            let options = RuntimeOptions::default().with_embedding_policy(EmbeddingPolicy {
                provider: Arc::new(StaticEmbeddingProvider { dimension: 2048 }),
                dimension: 2048,
                signature: crate::config::build_embedding_signature(
                    "openai-compatible",
                    Some("nvidia/nemotron-3-embed-1b"),
                    Some("https://integrate.api.nvidia.com/v1"),
                    2048,
                ),
                model: Some("nvidia/nemotron-3-embed-1b".to_string()),
                provider_label: "openai-compatible",
            });
            TenantRuntime::from_bound_client_with_runtime_options(
                &tenant("ten_lvl", "tenant_lvl"),
                client,
                crate::operations::quota::QuotaPlan::default(),
                options,
            )
            .await
            .unwrap()
        })
        .await;

        let recorded = sink.lines();
        // Filtered by this test's namespace: `record` broadcasts every line to
        // every active sink, so a sibling tenant test building its own runtime
        // lands its decision line here too. Only *this* runtime's decision can
        // prove *this* runtime's logger.
        assert!(
            !recorded.iter().any(|line| {
                line.contains("op=http.tenant_embedding_decision")
                    && line.contains("namespace=tenant_lvl")
            }),
            "the decision helper must obey the deployment level, not log at a \
             hardcoded info: {recorded:?}"
        );
        assert!(
            !runtime
                .mcp_service
                .service()
                .logger
                .is_event_enabled(crate::logging::LogLevel::Info, "any.op"),
            "the tenant service logger must carry the deployment level, not a \
             hardcoded info"
        );
    }

    /// An enabled provider that answers deterministically, so the tests above
    /// can observe which provider the tenant actually received without a
    /// network or model dependency.
    struct StaticEmbeddingProvider {
        dimension: usize,
    }

    #[async_trait::async_trait]
    impl crate::embedding::providers::EmbeddingProvider for StaticEmbeddingProvider {
        fn is_enabled(&self) -> bool {
            true
        }

        fn provider_name(&self) -> &'static str {
            "static-test-provider"
        }

        fn dimension(&self) -> usize {
            self.dimension
        }

        async fn embed(&self, _input: &str) -> Result<Vec<f64>, crate::error::MemoryError> {
            Ok(vec![0.0; self.dimension])
        }
    }
}
