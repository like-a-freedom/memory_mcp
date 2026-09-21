//! HTTP SaaS profile. Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod app_sessions;
pub mod composition;
pub mod config;
pub mod fault_injection;
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
    /// Local admin authentication extension. `Some` only when local auth mode
    /// is active. Provides the authority, hasher, and auth service.
    #[cfg(feature = "control-plane")]
    pub local_admin: Option<crate::control::local_admin::LocalAdminExtension>,
    /// The durable browser-auth policy fence for OIDC mode, joined at
    /// composition time from the durable singleton. `None` when the control
    /// plane is disabled or the deployment runs the local-admin surface
    /// (which owns its own fence through `LocalAdminAuthority`). It is never
    /// populated from a browser-supplied value; OIDC handlers and signup pass
    /// it to the dedicated guarded store operations.
    #[cfg(feature = "control-plane")]
    pub browser_policy: Option<crate::http::registry::models::BrowserPolicyFence>,
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
    ///
    /// The OIDC browser policy is always joined from the durable store;
    /// tests that need an OIDC-shaped state without a live issuer use
    /// [`Self::assemble_with_browser_policy`].
    pub(crate) async fn assemble(
        config: HttpConfig,
        registry: registry::RegistryHandle,
        metrics_handle: AssembleMetrics,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        Self::assemble_with_browser_policy(config, registry, metrics_handle, None).await
    }

    /// [`Self::assemble`] with an optional pre-joined browser policy.
    ///
    /// Production passes `None` and the OIDC policy is joined from the
    /// durable singleton. The `test-fixtures` builder may pass a fence it
    /// joined directly so unit tests can drive the OIDC handlers without
    /// spinning up an identity provider; the value is never accepted from
    /// a browser.
    pub(crate) async fn assemble_with_browser_policy(
        config: HttpConfig,
        registry: registry::RegistryHandle,
        _metrics_handle: AssembleMetrics,
        browser_policy_override: Option<crate::http::registry::models::BrowserPolicyFence>,
    ) -> Result<Arc<Self>, crate::error::MemoryError> {
        // The `free` plan backs the data plane: tenants created by signup, and
        // tenants that predate this change, carry `plan_version 1`, and the data
        // plane resolves that row on every ingest. A `local`-only deployment
        // instead creates its own `local_plan_v{version}` row and compares the
        // stored limits, so a hardcoded `free` plan must not be published at
        // all. As soon as `oidc` is enabled the plan is needed again, because
        // that is the method whose signup creates those tenants.
        let local_only = config.has_method(crate::http::config::BrowserAuthMethod::Local)
            && !config.has_method(crate::http::config::BrowserAuthMethod::Oidc);
        if !local_only {
            let signup_plan = registry::models::Plan {
                id: "free".into(),
                version: 1,
                limits: config.signup_plan_limits.clone().unwrap_or_default(),
            };
            registry.ensure_plan(&signup_plan).await?;
        }
        let pool = Arc::new(runtime::pool::Pool::from_http_config(
            &config,
            Arc::new(registry.clone()),
        ));
        let store = registry.store_clone();
        // The configured method set is what the durable policy reconciles to.
        // The join is reconcile-and-extend and never removes: a pre-existing
        // policy that enables a method this configuration omits fails startup
        // here, before any browser request is served (ADR-0057).
        let desired_methods = config.browser_auth_methods();
        // One reconciliation, from the full configured set. `local` and `oidc`
        // are not two competing claims on the singleton row: whichever methods
        // are enabled are written together, in one transaction, by one writer.
        #[cfg(feature = "control-plane")]
        let browser_policy = if let Some(policy) = browser_policy_override {
            Some(policy)
        } else if config.enable_control_plane && config.browser_auth.is_some() {
            let local = config
                .browser_auth
                .as_ref()
                .and_then(|methods| methods.local.as_ref())
                .map(|local| {
                    crate::service::local_admin::auth::compute_fingerprints(
                        &local.session_key,
                        &local.csrf_key,
                    )
                })
                .transpose()
                .map_err(|error| crate::error::MemoryError::Auth(error.to_string()))?;
            Some(
                store
                    .reconcile_browser_policy(&desired_methods, local)
                    .await?,
            )
        } else {
            None
        };
        #[cfg(not(feature = "control-plane"))]
        let _ = browser_policy_override;
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
        // The OIDC client performs discovery against the configured
        // issuer at startup. Without the `oidc` method there is no issuer to
        // reach and no OIDC route is mounted, so discovery must not run: such a
        // deployment has no dependency on any identity provider being online.
        // A disabled control plane mounts no browser surface at all.
        #[cfg(feature = "control-plane")]
        let oidc_client = if config.enable_control_plane
            && config.has_method(crate::http::config::BrowserAuthMethod::Oidc)
        {
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
        #[cfg(feature = "control-plane")]
        let local_admin = if !config.enable_control_plane {
            None
        } else if let Some(local_config) = config
            .browser_auth
            .as_ref()
            .and_then(|methods| methods.local.as_ref())
        {
            // Ensure the deployment's version-1 local plan exists
            // and has not drifted. `ensure_local_plan` returns
            // `plan_limit_mismatch` when an operator has already
            // stored a different limit set, so a silent downgrade
            // or upgrade of limits is impossible.
            registry
                .ensure_local_plan(&registry::models::Plan {
                    id: format!("local_plan_v{}", local_config.default_plan_version),
                    version: local_config.default_plan_version,
                    limits: local_config.default_plan_limits.clone(),
                })
                .await?;
            let store = registry.local_admin_store_clone().ok_or_else(|| {
                crate::error::MemoryError::ConfigInvalid(
                    "local browser auth requires the durable local admin store".into(),
                )
            })?;
            use crate::service::local_admin::auth::LocalAdminAuthority;
            use crate::service::local_admin::password::PasswordHasher;
            // The reconciled fence is handed to the authority rather than
            // joined a second time: the singleton has one writer, and this is
            // that writer's result.
            let policy = browser_policy.clone().ok_or_else(|| {
                crate::error::MemoryError::ConfigInvalid(
                    "local browser auth requires the durable browser-auth policy".into(),
                )
            })?;
            let authority = LocalAdminAuthority::join(
                store,
                local_config.session_key,
                local_config.csrf_key,
                policy,
            )
            .map_err(|e| crate::error::MemoryError::Auth(e.to_string()))?;
            let hasher = Arc::new(
                PasswordHasher::new()
                    .map_err(|e| crate::error::MemoryError::Auth(e.to_string()))?,
            );
            Some(crate::control::local_admin::LocalAdminExtension {
                authority,
                hasher,
                plan_version: local_config.default_plan_version,
            })
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
            #[cfg(feature = "control-plane")]
            local_admin,
            #[cfg(feature = "control-plane")]
            browser_policy,
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
