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
pub mod transport;
pub mod validation;

use std::sync::Arc;

use config::HttpConfig;

/// Common fields between the `prometheus` and `!prometheus`
/// builds. Lets the constructor share one body via the
/// `#[cfg]`-only `metrics_handle` field added by each `new_tenantless`
/// arm.
pub struct HttpStateCore {
    pub config: HttpConfig,
    pub shared_handler: Arc<crate::mcp::handlers::MemoryMcp>,
    pub shutdown: shutdown::ShutdownState,
    pub admission: Arc<runtime::pool::AdmissionGate>,
    pub registry: registry::RegistryHandle,
    pub authenticator: Arc<principal::auth::Authenticator>,
    pub account_resolver: Arc<registry::account::AccountResolver>,
}

/// Process-wide HTTP state. Phase 3 shape: config + the tenantless
/// MCP handler + shutdown/admission/registry. Phase 4 added the
/// authenticator and account→tenant resolver (Tasks 4.4, 4.5).
/// Task 5.6 replaces `shared_handler` with a runtime-pool guard.
pub struct HttpState {
    pub core: Arc<HttpStateCore>,
    /// The Prometheus handle is `Some` only when the `prometheus`
    /// feature is enabled. When `None`, the `/metrics` route is
    /// not wired into the router.
    #[cfg(feature = "prometheus")]
    pub metrics_handle: Option<MetricsHandle>,
}

#[cfg(feature = "prometheus")]
pub type MetricsHandle = metrics_exporter_prometheus::PrometheusHandle;

impl HttpState {
    /// Phase 4 production constructor: tenantless handler + auth +
    /// resolver. The two arms differ only in the metrics handle
    /// type; the shared core is constructed once.
    #[cfg(feature = "prometheus")]
    pub async fn new_tenantless(
        config: HttpConfig,
        metrics_handle: Option<MetricsHandle>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let core = Self::build_core(config).await?;
        Ok(Arc::new(Self {
            core,
            metrics_handle,
        }))
    }

    #[cfg(not(feature = "prometheus"))]
    pub async fn new_tenantless(
        config: HttpConfig,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let core = Self::build_core(config).await?;
        Ok(Arc::new(Self { core }))
    }

    async fn build_core(
        config: HttpConfig,
    ) -> Result<Arc<HttpStateCore>, crate::error::MemoryError> {
        let shared_handler = transport::build_tenantless_handler(&config).await?;
        let registry = registry::RegistryHandle::stub();
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
        Ok(Arc::new(HttpStateCore {
            config,
            shared_handler,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::new()),
            registry,
            authenticator,
            account_resolver,
        }))
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
            Self::new_tenantless(config, Self::test_metrics_handle())
                .await
                .expect("tenantless test state builds")
        }
        #[cfg(not(feature = "prometheus"))]
        {
            let _ = Self::test_metrics_handle();
            Self::new_tenantless(config)
                .await
                .expect("tenantless test state builds")
        }
    }
}

// Convenience accessors so existing call sites that read
// `state.config`, `state.shared_handler`, etc. keep compiling.
impl std::ops::Deref for HttpState {
    type Target = HttpStateCore;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
}
