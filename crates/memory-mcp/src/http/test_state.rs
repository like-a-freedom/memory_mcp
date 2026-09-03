//! Feature-gated `HttpState` test builder.
//!
//! The builder is the supported way to construct `HttpState` in unit
//! and integration tests. It keeps feature-gated fields such as
//! `metrics_handle` consistent across every build so test call sites
//! never encode the Cargo feature matrix themselves.

use std::sync::Arc;

pub struct HttpStateTestBuilder {
    config: super::config::HttpConfig,
    registry: super::registry::RegistryHandle,
    fault_injector: Arc<dyn super::fault_injection::FaultInjector>,
    #[cfg(feature = "prometheus")]
    metrics_handle: Option<super::MetricsHandle>,
}

impl HttpStateTestBuilder {
    pub async fn new() -> Self {
        Self {
            config: super::config::HttpConfig::default_for_test(),
            registry: super::registry::RegistryHandle::in_memory_with_default_mem_engine().await,
            fault_injector: Arc::new(super::fault_injection::NoFaults),
            #[cfg(feature = "prometheus")]
            metrics_handle: super::HttpState::test_metrics_handle(),
        }
    }

    pub fn with_config(mut self, config: super::config::HttpConfig) -> Self {
        self.config = config;
        self
    }

    pub fn with_registry(mut self, registry: super::registry::RegistryHandle) -> Self {
        self.registry = registry;
        self
    }

    pub fn with_fault_injector(
        mut self,
        injector: Arc<dyn super::fault_injection::FaultInjector>,
    ) -> Self {
        self.fault_injector = injector;
        self
    }

    #[cfg(feature = "prometheus")]
    pub fn with_metrics_handle(mut self, handle: Option<super::MetricsHandle>) -> Self {
        self.metrics_handle = handle;
        self
    }

    pub async fn build(
        self,
    ) -> Result<std::sync::Arc<super::HttpState>, crate::error::MemoryError> {
        #[cfg(feature = "prometheus")]
        {
            super::HttpState::assemble(
                self.config,
                self.registry,
                self.fault_injector,
                self.metrics_handle,
            )
            .await
        }
        #[cfg(not(feature = "prometheus"))]
        {
            super::HttpState::assemble(self.config, self.registry, self.fault_injector, None).await
        }
    }
}
