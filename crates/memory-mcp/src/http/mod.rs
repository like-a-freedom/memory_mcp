//! HTTP SaaS profile. Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod app_sessions;
pub mod composition;
pub mod config;
pub mod health;
pub mod leases;
pub mod logging;
pub mod metrics;
pub mod middleware;
#[cfg(feature = "control-plane")]
pub mod oauth;
pub mod principal;
pub mod registry;
pub mod router;
pub mod runtime;
pub mod server;
pub mod shutdown;
pub mod subscriptions;
pub mod sync;
pub mod tasks;
pub mod transport;
pub mod validation;

#[cfg(feature = "test-fixtures")]
pub mod test_bootstrap;

#[cfg(any(test, feature = "test-fixtures"))]
pub mod test_state;

use std::sync::Arc;

use config::HttpConfig;

/// Process-wide HTTP state. Config + the
/// runtime pool + shutdown/admission/registry/auth/resolver.
pub struct HttpState {
    pub config: HttpConfig,
    /// The runtime pool. The `acquire_runtime`
    /// middleware calls `acquire_or_wait`; the `mcp_handler`
    /// extracts the resulting `OperationGuard` and moves it
    /// into the response body.
    pub pool: Arc<runtime::pool::Pool>,
    pub shutdown: shutdown::ShutdownState,
    pub admission: Arc<runtime::pool::AdmissionGate>,
    pub registry: registry::RegistryHandle,
    /// Bearer-token authenticator. The auth middleware
    /// dispatches to it for every POST /mcp.
    pub authenticator: Arc<principal::auth::Authenticator>,
    /// Account → Tenant resolver. The Tenant Runtime
    /// consumes the `Ready` arm; the others become 4xx/5xx.
    pub account_resolver: Arc<registry::account::AccountResolver>,
    /// OIDC client for the control-plane login flow.
    /// `None` when the control plane is disabled.
    #[cfg(feature = "control-plane")]
    pub oidc_client: Option<Arc<crate::control::oidc::OidcClient>>,
    /// The Prometheus handle is `Some` only when the `prometheus`
    /// feature is enabled. When `None`, the `/metrics` route is
    /// not wired into the router.
    #[cfg(feature = "prometheus")]
    pub metrics_handle: Option<MetricsHandle>,
}

#[cfg(feature = "prometheus")]
pub type MetricsHandle = metrics_exporter_prometheus::PrometheusHandle;

/// The metrics parameter accepted by `HttpState::assemble`. Builds
/// without the `prometheus` feature carry a zero-sized placeholder so
/// both feature arms share one assembly body without `cfg` attributes
/// on individual parameters or call arguments.
#[cfg(feature = "prometheus")]
type AssembleMetrics = Option<MetricsHandle>;
#[cfg(not(feature = "prometheus"))]
type AssembleMetrics = Option<()>;

impl HttpState {
    /// Production constructor. The two arms differ only in the metrics
    /// handle type. Storage selection happens inside
    /// `HttpProductionComposition::connect`.
    #[cfg(feature = "prometheus")]
    pub async fn new(
        config: HttpConfig,
        metrics_handle: Option<MetricsHandle>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let composition = composition::HttpProductionComposition::connect(&config).await?;
        Self::assemble(config, composition.registry, metrics_handle).await
    }

    /// Production constructor (no Prometheus).
    #[cfg(not(feature = "prometheus"))]
    pub async fn new(config: HttpConfig) -> Result<Arc<Self>, crate::error::MemoryError> {
        let composition = composition::HttpProductionComposition::connect(&config).await?;
        Self::assemble(config, composition.registry, None).await
    }

    /// The single state-assembly path shared by every constructor and
    /// by the feature-gated test builder. Storage selection stays in
    /// `build_registry`; this function only wires the state itself.
    pub(crate) async fn assemble(
        config: HttpConfig,
        registry: registry::RegistryHandle,
        _metrics_handle: AssembleMetrics,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        let signup_plan = registry::models::Plan {
            id: "free".into(),
            version: 1,
            limits: config.signup_plan_limits.clone().unwrap_or_default(),
        };
        registry.ensure_plan(&signup_plan).await?;
        let pool = Arc::new(runtime::pool::Pool::from_http_config(
            &config,
            Arc::new(registry.clone()),
        ));
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
        #[cfg(feature = "control-plane")]
        let oidc_client = if config.enable_control_plane {
            Some(Arc::new(
                crate::control::oidc::OidcClient::new(
                    &config.oidc_issuer,
                    &config.oidc_client_id,
                    &config.oidc_audience,
                    &config.oidc_redirect_uri,
                    &config.oidc_allowed_alg,
                )
                .await?,
            ))
        } else {
            None
        };
        #[cfg(feature = "prometheus")]
        let metrics_handle = _metrics_handle;
        Ok(Arc::new(Self {
            config: config.clone(),
            pool,
            shutdown: shutdown::ShutdownState::new(),
            admission: Arc::new(runtime::pool::AdmissionGate::new_with_limits(
                config.global_request_limit,
                config.subscription_limit,
            )),
            registry,
            authenticator,
            account_resolver,
            #[cfg(feature = "control-plane")]
            oidc_client,
            #[cfg(feature = "prometheus")]
            metrics_handle,
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
        test_state::HttpStateTestBuilder::new()
            .await
            .build()
            .await
            .expect("HTTP state for test builds")
    }
}
