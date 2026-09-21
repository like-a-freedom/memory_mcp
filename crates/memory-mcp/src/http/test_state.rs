//! Feature-gated `HttpState` test builder.
//!
//! The builder is the supported way to construct `HttpState` in unit
//! and integration tests. It keeps feature-gated fields such as
//! `metrics_handle` consistent across every build so test call sites
//! never encode the Cargo feature matrix themselves.
//!
//! [`HttpStateTestBuilder::local_admin`] is the explicit durable
//! local composition: a migrated in-memory Registry store exposed as
//! both `RegistryStore` and `LocalAdminStore`, a local-mode browser
//! policy, and fixed test keys. Nothing is partially injected — the
//! policy, the store and the config are validated together by
//! `HttpState::assemble`.

pub struct HttpStateTestBuilder {
    config: super::config::HttpConfig,
    registry: super::registry::RegistryHandle,
    #[cfg(feature = "control-plane")]
    browser_policy: Option<crate::http::registry::models::BrowserPolicyFence>,
    #[cfg(feature = "prometheus")]
    metrics_handle: Option<super::MetricsHandle>,
}

impl HttpStateTestBuilder {
    pub async fn new() -> Self {
        Self {
            config: super::config::HttpConfig::default_for_test(),
            registry: super::registry::RegistryHandle::in_memory_with_default_mem_engine().await,
            #[cfg(feature = "control-plane")]
            browser_policy: None,
            #[cfg(feature = "prometheus")]
            metrics_handle: super::HttpState::test_metrics_handle(),
        }
    }

    /// Fixed test-only session key for the local browser policy.
    #[cfg(feature = "control-plane")]
    pub const LOCAL_TEST_SESSION_KEY: [u8; 32] = [0x11; 32];
    /// Fixed test-only CSRF key for the local browser policy.
    #[cfg(feature = "control-plane")]
    pub const LOCAL_TEST_CSRF_KEY: [u8; 32] = [0x22; 32];

    /// Build the explicit durable local-admin composition.
    ///
    /// Returns the builder together with the concrete store so a test
    /// can drive the CLI/management surface (which joins the same
    /// durable policy) before exercising the HTTP routes.
    #[cfg(feature = "control-plane")]
    pub async fn local_admin() -> (Self, std::sync::Arc<super::registry::SurrealRegistryStore>) {
        use crate::http::config::{BrowserAuthMethods, LocalBrowserConfig};
        use crate::service::local_admin::contracts::LocalAdminStore;

        let namespace = format!("http_local_admin_{}", uuid::Uuid::new_v4().simple());
        let store = std::sync::Arc::new(
            super::registry::SurrealRegistryStore::connect_in_memory(&namespace, "registry")
                .await
                .expect("migrated in-memory registry"),
        );
        let local_store: std::sync::Arc<dyn LocalAdminStore> = store.clone();
        let registry_store: std::sync::Arc<dyn super::registry::RegistryStore> = store.clone();
        let engine = store.privileged_engine();
        let registry = super::registry::RegistryHandle::from_durable(
            registry_store,
            Some(local_store),
            engine,
        );
        let mut config = super::config::HttpConfig::default_for_test();
        config.enable_control_plane = true;
        config.browser_auth = Some(BrowserAuthMethods {
            local: Some(LocalBrowserConfig {
                session_key: Self::LOCAL_TEST_SESSION_KEY,
                csrf_key: Self::LOCAL_TEST_CSRF_KEY,
                default_plan_version: 1,
                default_plan_limits: super::registry::models::PlanLimits::default(),
            }),
            oidc: None,
        });
        (
            Self {
                config,
                registry,
                #[cfg(feature = "control-plane")]
                browser_policy: None,
                #[cfg(feature = "prometheus")]
                metrics_handle: super::HttpState::test_metrics_handle(),
            },
            store,
        )
    }

    pub fn with_config(mut self, config: super::config::HttpConfig) -> Self {
        self.config = config;
        self
    }

    /// Pre-join the durable OIDC browser policy so a unit test can drive
    /// the OIDC handlers without a live identity provider. Production
    /// composition joins the policy itself; this path is test-only and
    /// never accepts a browser-supplied value.
    #[cfg(feature = "control-plane")]
    pub fn with_browser_policy(
        mut self,
        policy: crate::http::registry::models::BrowserPolicyFence,
    ) -> Self {
        self.browser_policy = Some(policy);
        self
    }

    pub fn with_registry(mut self, registry: super::registry::RegistryHandle) -> Self {
        self.registry = registry;
        self
    }

    /// Attach a local-admin store explicitly. Used when a test builds
    /// its own `HttpConfig` and store and wants production-shaped
    /// wiring rather than the [`Self::local_admin`] preset.
    #[cfg(feature = "control-plane")]
    pub fn with_local_admin_store(
        mut self,
        store: std::sync::Arc<dyn crate::service::local_admin::contracts::LocalAdminStore>,
    ) -> Self {
        self.registry = self.registry.with_local_admin_store(store);
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
            #[cfg(feature = "control-plane")]
            {
                super::HttpState::assemble_with_browser_policy(
                    self.config,
                    self.registry,
                    self.metrics_handle,
                    self.browser_policy,
                )
                .await
            }
            #[cfg(not(feature = "control-plane"))]
            {
                super::HttpState::assemble(self.config, self.registry, self.metrics_handle).await
            }
        }
        #[cfg(not(feature = "prometheus"))]
        {
            #[cfg(feature = "control-plane")]
            {
                super::HttpState::assemble_with_browser_policy(
                    self.config,
                    self.registry,
                    None,
                    self.browser_policy,
                )
                .await
            }
            #[cfg(not(feature = "control-plane"))]
            {
                super::HttpState::assemble(self.config, self.registry, None).await
            }
        }
    }
}
