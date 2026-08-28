//! HTTP SaaS profile (ADR-0052). Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod config;
pub mod health;
pub mod metrics;
pub mod middleware;
pub mod router;
pub mod server;
pub mod shutdown;
pub mod transport;
pub mod validation;

use std::sync::Arc;

use config::HttpConfig;

/// Process-wide HTTP state. Phase 3 shape: config + the tenantless MCP
/// factory. Later tasks extend this struct (Task 3.8 metrics handle,
/// Task 3.9 shutdown/admission/registry, Task 4.4 authenticator, Task
/// 4.5 account_resolver, Task 5.6 pool).
pub struct HttpState {
    pub config: HttpConfig,
    /// Phase 3 dispatch factory: clones a prebuilt tenantless handler
    /// per request. Replaced by the runtime-pool guard in Task 5.6.
    pub mcp_factory:
        Arc<dyn Fn() -> Result<crate::mcp::handlers::MemoryMcp, std::io::Error> + Send + Sync>,
    #[cfg(feature = "prometheus")]
    pub metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
}

impl HttpState {
    /// Phase 3 production constructor: single-tenant handler over the
    /// configured tenant target (no auth yet — auth lands in Phase 4).
    #[cfg(feature = "prometheus")]
    pub async fn new_tenantless(
        config: HttpConfig,
        metrics_handle: metrics_exporter_prometheus::PrometheusHandle,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let mcp = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            mcp_factory: Arc::new(move || Ok((*mcp).clone())),
            config,
            metrics_handle,
        }))
    }

    #[cfg(not(feature = "prometheus"))]
    pub async fn new_tenantless(config: HttpConfig) -> Result<Arc<Self>, crate::error::MemoryError> {
        let mcp = transport::build_tenantless_handler(&config).await?;
        Ok(Arc::new(Self {
            mcp_factory: Arc::new(move || Ok((*mcp).clone())),
            config,
        }))
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpState {
    /// Test-only OnceLock that installs the recorder exactly once per
    /// test process. Without this, the second test that builds a
    /// `default_for_test` state would double-install the recorder
    /// and panic.
    #[cfg(feature = "prometheus")]
    fn test_metrics_handle() -> metrics_exporter_prometheus::PrometheusHandle {
        use std::sync::OnceLock;
        static HANDLE: OnceLock<metrics_exporter_prometheus::PrometheusHandle> = OnceLock::new();
        HANDLE
            .get_or_init(|| {
                metrics_exporter_prometheus::PrometheusBuilder::new()
                    .install_recorder()
                    .expect("first recorder install in the test process")
            })
            .clone()
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
            Self::new_tenantless(config)
                .await
                .expect("tenantless test state builds")
        }
    }
}
