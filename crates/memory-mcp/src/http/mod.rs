//! HTTP SaaS profile (ADR-0052). Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod config;
pub mod health;
pub mod logging;
pub mod metrics;
pub mod middleware;
pub mod registry;
pub mod router;
pub mod runtime;
pub mod server;
pub mod shutdown;
pub mod transport;
pub mod validation;

use std::sync::Arc;

use config::HttpConfig;

/// Process-wide HTTP state. Phase 3 shape: config + the tenantless MCP
/// handler + shutdown/admission/registry stubs. Later tasks extend
/// this struct (Task 4.4 authenticator, Task 4.5 account_resolver,
/// Task 5.6 pool).
pub struct HttpState {
    pub config: HttpConfig,
    /// Phase 3 dispatch: every request clones this single tenantless
    /// handler. Task 5.6 replaces the field with a runtime-pool guard.
    pub shared_handler: Arc<crate::mcp::handlers::MemoryMcp>,
    pub shutdown: shutdown::ShutdownState,
    pub admission: Arc<runtime::pool::AdmissionGate>,
    pub registry: registry::RegistryHandle,
    /// The Prometheus handle is `Some` only when the `prometheus`
    /// feature is enabled. When `None`, the `/metrics` route is not
    /// wired into the router.
    #[cfg(feature = "prometheus")]
    pub metrics_handle: Option<MetricsHandle>,
}

#[cfg(feature = "prometheus")]
pub type MetricsHandle = metrics_exporter_prometheus::PrometheusHandle;

impl HttpState {
    /// Phase 3 production constructor: single-tenant handler over the
    /// configured tenant target (no auth yet — auth lands in Phase 4).
    /// The two arms only differ in the metrics handle type; the
    /// shared `Self { ... }` literal is the same shape either way.
    #[cfg(feature = "prometheus")]
    pub async fn new_tenantless(
        config: HttpConfig,
        metrics_handle: Option<MetricsHandle>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let shared_handler = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            config,
            shared_handler,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::new()),
            registry: registry::RegistryHandle,
            metrics_handle,
        }))
    }

    #[cfg(not(feature = "prometheus"))]
    pub async fn new_tenantless(
        config: HttpConfig,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let shared_handler = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            config,
            shared_handler,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::new()),
            registry: registry::RegistryHandle,
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
