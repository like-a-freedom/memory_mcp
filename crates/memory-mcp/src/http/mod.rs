//! HTTP SaaS profile (ADR-0052). Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod config;
pub mod health;
pub mod leases;
pub mod logging;
pub mod metrics;
pub mod middleware;
pub mod principal;
pub mod registry;
pub mod router;
pub mod runtime;
pub mod server;
pub mod shutdown;
pub mod sync;
pub mod transport;
pub mod validation;

#[cfg(feature = "test-fixtures")]
pub mod test_bootstrap;

use std::sync::Arc;

use config::HttpConfig;

/// Process-wide HTTP state. Phase 5 shape: config + the
/// runtime pool + shutdown/admission/registry/auth/resolver.
pub struct HttpState {
    pub config: HttpConfig,
    /// The runtime pool (Task 5.5). The `acquire_runtime`
    /// middleware calls `acquire_or_wait`; the `mcp_handler`
    /// extracts the resulting `OperationGuard` and moves it
    /// into the response body.
    pub pool: Arc<runtime::pool::Pool>,
    pub shutdown: shutdown::ShutdownState,
    pub admission: Arc<runtime::pool::AdmissionGate>,
    pub registry: registry::RegistryHandle,
    /// Bearer-token authenticator. The auth middleware
    /// (Task 4.6) dispatches to it for every POST /mcp.
    pub authenticator: Arc<principal::auth::Authenticator>,
    /// Account → Tenant resolver. The Tenant Runtime (Task 5.6)
    /// consumes the `Ready` arm; the others become 4xx/5xx.
    pub account_resolver: Arc<registry::account::AccountResolver>,
    /// The Prometheus handle is `Some` only when the `prometheus`
    /// feature is enabled. When `None`, the `/metrics` route is
    /// not wired into the router.
    #[cfg(feature = "prometheus")]
    pub metrics_handle: Option<MetricsHandle>,
}

#[cfg(feature = "prometheus")]
pub type MetricsHandle = metrics_exporter_prometheus::PrometheusHandle;

impl HttpState {
    /// Phase 5 production constructor: registry + auth + resolver
    /// + runtime pool. The two arms differ only in the metrics
    /// handle type. Environment-driven pool overrides land with
    /// Task 6.4.
    #[cfg(feature = "prometheus")]
    pub async fn new(
        config: HttpConfig,
        metrics_handle: Option<MetricsHandle>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let registry = Self::build_registry().await;
        let pool = Arc::new(runtime::pool::Pool::with_defaults(Arc::new(
            registry.clone(),
        )));
        let store = registry.store_clone();
        let authenticator = Arc::new(principal::auth::Authenticator::new(
            store.clone(),
            Arc::new(principal::cache::PrincipalCache::new(1024)),
            config.api_key_pepper.as_bytes().to_vec(),
            Arc::new(principal::auth::RateLimiter::new(
                4096,
                std::time::Duration::from_secs(1),
                20,
            )),
        ));
        let account_resolver = Arc::new(registry::account::AccountResolver::new(store));
        Ok(Arc::new(Self {
            config,
            pool,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::open()),
            registry,
            authenticator,
            account_resolver,
            metrics_handle,
        }))
    }

    /// Phase 5 production constructor (no Prometheus).
    #[cfg(not(feature = "prometheus"))]
    pub async fn new(config: HttpConfig) -> Result<Arc<Self>, crate::error::MemoryError> {
        let registry = Self::build_registry().await;
        let pool = Arc::new(runtime::pool::Pool::with_defaults(Arc::new(
            registry.clone(),
        )));
        let store = registry.store_clone();
        let authenticator = Arc::new(principal::auth::Authenticator::new(
            store.clone(),
            Arc::new(principal::cache::PrincipalCache::new(1024)),
            config.api_key_pepper.as_bytes().to_vec(),
            Arc::new(principal::auth::RateLimiter::new(
                4096,
                std::time::Duration::from_secs(1),
                20,
            )),
        ));
        let account_resolver = Arc::new(registry::account::AccountResolver::new(store));
        Ok(Arc::new(Self {
            config,
            pool,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::open()),
            registry,
            authenticator,
            account_resolver,
        }))
    }

    /// Build the registry handle. Test-fixtures builds use
    /// the in-memory backend with a privileged Mem engine so
    /// the bootstrap (Task 5.8) can write accounts, tenants,
    /// and api keys, and the runtime pool can build
    /// per-tenant handles. Production builds use the
    /// placeholder; Task 5.x replaces the placeholder with a
    /// real SurrealDB backend.
    async fn build_registry() -> registry::RegistryHandle {
        #[cfg(any(test, feature = "test-fixtures"))]
        {
            registry::RegistryHandle::in_memory_with_default_mem_engine().await
        }
        #[cfg(not(any(test, feature = "test-fixtures")))]
        {
            registry::RegistryHandle::new()
        }
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpState {
    /// Test-only handle. Delegates to the shared OnceLock in
    /// `observability` so that the observability test suite and
    /// this fixture can both run without racing the
    /// `install_recorder` panic.
    #[cfg(feature = "prometheus")]
    pub fn test_metrics_handle() -> Option<MetricsHandle> {
        crate::observability::shared_test_handle()
    }

    #[cfg(not(feature = "prometheus"))]
    pub fn test_metrics_handle() -> Option<()> {
        None
    }

    pub async fn default_for_test() -> Arc<Self> {
        let config = HttpConfig::default_for_test();
        #[cfg(feature = "prometheus")]
        {
            Self::new(config, Self::test_metrics_handle())
                .await
                .expect("HTTP state for test builds")
        }
        #[cfg(not(feature = "prometheus"))]
        {
            let _ = Self::test_metrics_handle();
            Self::new(config).await.expect("HTTP state for test builds")
        }
    }
}
