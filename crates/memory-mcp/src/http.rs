//! HTTP SaaS profile. Gated on `streamable-http` in lib.rs:
//! `#[cfg(feature = "streamable-http")] pub mod http;`

pub mod app_sessions;
pub mod composition;
pub mod config;
pub mod embedding;
pub mod health;
pub mod leases;
pub mod lifecycle;
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

use config::{BrowserAuthMethod, HttpConfig};

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
    pub(crate) preflight_budget: Arc<middleware::preflight_budget::PreflightBudget>,
    pub registry: registry::RegistryHandle,
    #[cfg(feature = "control-plane")]
    pub(crate) identity_link_transactions: Arc<dyn crate::identity::api::IdentityLinkTransactions>,
    #[cfg(feature = "control-plane")]
    pub(crate) verified_identity_transactions:
        Arc<dyn crate::identity::api::VerifiedIdentityLinkTransactions>,
    #[cfg(feature = "control-plane")]
    pub(crate) identity_invitation_port: Arc<dyn crate::identity::api::IdentityInvitationPort>,
    #[cfg(feature = "control-plane")]
    pub(crate) invitation_session_port: Arc<dyn crate::identity::api::InvitationSessionPort>,
    #[cfg(feature = "control-plane")]
    pub(crate) account_deletion_port: Arc<dyn crate::operations::api::AccountDeletionPort>,
    /// Bearer-token authenticator. The auth middleware
    /// dispatches to it for every POST /mcp.
    pub authenticator: Arc<principal::auth::Authenticator>,
    /// Trusted Account → Tenant resolution owned by tenancy.
    pub tenant_resolver: Arc<dyn crate::tenancy::api::ResolveTenantPort>,
    pub api_key_issuance: Arc<dyn crate::provisioning::api::ApiKeyIssuancePort>,
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
        Self::assemble_with_deployment_policy(
            config,
            registry,
            _metrics_handle,
            browser_policy_override,
            None,
            Arc::new(crate::embedding::providers::task_runner::BackgroundTaskRunner::new()),
        )
        .await
    }

    /// [`Self::assemble`] with the deployment-level policy threaded into every
    /// tenant runtime the pool builds. This is the body of the assembly; every
    /// other entry point above adds one parameter and forwards here.
    ///
    /// `None` means the operator did not enable embeddings, so tenant
    /// runtimes serve lexical retrieval only.
    pub(crate) async fn assemble_with_deployment_policy(
        config: HttpConfig,
        registry: registry::RegistryHandle,
        _metrics_handle: AssembleMetrics,
        browser_policy_override: Option<crate::http::registry::models::BrowserPolicyFence>,
        deployment_policy: Option<runtime::bootstrap::DeploymentPolicy>,
        background_task_runner: Arc<crate::embedding::providers::task_runner::BackgroundTaskRunner>,
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
        let shutdown = shutdown::ShutdownState::new();
        let preflight_budget = Arc::new(middleware::preflight_budget::PreflightBudget::new(
            config.preflight_request_limit,
            config.preflight_bytes,
        )?);
        let pool = Arc::new(runtime::pool::Pool::from_http_config_with_shutdown(
            &config,
            Arc::new(registry.clone()),
            shutdown.clone(),
            deployment_policy,
            background_task_runner,
        ));
        // Each consumer below is handed the owner traits it uses, not the
        // registry. One handle is still built above, but nothing below reaches
        // through it for a table it does not own.
        let stores = registry.stores().clone();
        // The configured method set is what the durable policy reconciles to.
        // The join is reconcile-and-extend and never removes: a pre-existing
        // policy that enables a method this configuration omits fails startup
        // here, before any browser request is served (ADR-0057).
        let desired_methods = config
            .browser_auth_methods()
            .into_iter()
            .map(|method| match method {
                BrowserAuthMethod::Local => crate::identity::api::AuthMethod::Local,
                BrowserAuthMethod::Oidc => crate::identity::api::AuthMethod::Oidc,
            })
            .collect::<Vec<_>>();
        let policy_port: Arc<dyn crate::identity::api::AuthMethodPolicyPort> = Arc::new(
            crate::bootstrap::integration::auth_method_policy::RegistryAuthMethodPolicy::new(
                stores.browser_policy(),
                config.clone(),
            ),
        );
        let browser_policy = if let Some(policy) = browser_policy_override.clone() {
            Some(policy)
        } else if config.browser_auth.is_some() {
            let policy = crate::identity::api::reconcile_auth_methods(
                policy_port.as_ref(),
                &desired_methods,
            )
            .await
            .map_err(|error| match error {
                crate::identity::api::IdentityError::Persistence(error) => error,
                _ => crate::error::MemoryError::ConfigInvalid("invalid auth methods".into()),
            })?;
            Some(registry::models::BrowserPolicyFence {
                methods: policy
                    .methods
                    .into_iter()
                    .map(|method| match method {
                        crate::identity::api::AuthMethod::Local => BrowserAuthMethod::Local,
                        crate::identity::api::AuthMethod::Oidc => BrowserAuthMethod::Oidc,
                    })
                    .collect(),
                epoch: policy.epoch,
            })
        } else {
            None
        };
        #[cfg(not(feature = "control-plane"))]
        let _ = browser_policy_override;
        let authenticator = Arc::new(principal::auth::Authenticator::new(
            stores.accounts(),
            stores.api_keys(),
            Arc::new(principal::cache::PrincipalCache::new(1024)),
            config.api_key_pepper.as_bytes().to_vec(),
            Arc::new(principal::auth::RateLimiter::new(
                4096,
                std::time::Duration::from_secs(1),
                20,
            )),
        ));
        let api_key_issuance: Arc<dyn crate::provisioning::api::ApiKeyIssuancePort> = Arc::new(
            crate::bootstrap::integration::provisioning::RegistryApiKeyIssuance::new(
                stores.accounts(),
                stores.tenants(),
                stores.usage(),
                stores.api_keys(),
                config.api_key_pepper.clone(),
            ),
        );
        let account_resolver = Arc::new(registry::account::AccountResolver::new(stores.tenants()));
        let tenant_resolver: Arc<dyn crate::tenancy::api::ResolveTenantPort> = Arc::new(
            crate::bootstrap::integration::tenancy_resolution::RegistryTenantResolver::new(
                account_resolver.clone(),
            ),
        );
        // The OIDC client performs discovery against the configured
        // issuer at startup. Without the `oidc` method there is no issuer to
        // reach and no OIDC route is mounted, so discovery must not run: such a
        // deployment has no dependency on any identity provider being online.
        // A disabled control plane mounts no browser surface at all.
        #[cfg(feature = "control-plane")]
        let oidc_client = if browser_policy_override.is_none()
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
        let local_admin = if let Some(local_config) = config
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
            let client_creation: Arc<dyn crate::provisioning::api::ClientCreationPort> = Arc::new(
                crate::bootstrap::integration::provisioning::LocalAdminClientCreation::new(
                    Arc::clone(&registry.local_admin_store_clone().ok_or_else(|| {
                        crate::error::MemoryError::ConfigInvalid(
                            "local browser auth requires the durable local admin store".into(),
                        )
                    })?),
                ),
            );
            Some(crate::control::local_admin::LocalAdminExtension {
                authority,
                hasher,
                plan_version: local_config.default_plan_version,
                client_creation,
            })
        } else {
            None
        };
        #[cfg(feature = "control-plane")]
        let identity_transactions = Arc::new(
            crate::bootstrap::integration::registry_identity_links::RegistryIdentityLinkTransactions::from_stores(registry.stores()),
        );
        let identity_link_transactions: Arc<dyn crate::identity::api::IdentityLinkTransactions> =
            identity_transactions.clone();
        let verified_identity_transactions: Arc<
            dyn crate::identity::api::VerifiedIdentityLinkTransactions,
        > = identity_transactions.clone();
        let identity_invitation_port: Arc<dyn crate::identity::api::IdentityInvitationPort> =
            identity_transactions.clone();
        let policy_for_sessions = browser_policy.clone().ok_or_else(|| {
            crate::error::MemoryError::ConfigInvalid(
                "OIDC browser policy is required for invitation sessions".into(),
            )
        })?;
        let invitation_session_port: Arc<dyn crate::identity::api::InvitationSessionPort> = Arc::new(
            crate::bootstrap::integration::control_sessions::ControlSessionInvitationAdapter::new(
                stores.accounts(),
                stores.sessions(),
                config.clone(),
                policy_for_sessions,
            ),
        );
        #[cfg(feature = "control-plane")]
        let account_deletion_port: Arc<dyn crate::operations::api::AccountDeletionPort> = Arc::new(
            crate::bootstrap::integration::registry_operations::RegistryAccountDeletionAdapter::from_stores(
                registry.stores(),
            ),
        );
        #[cfg(feature = "prometheus")]
        let metrics_handle = _metrics_handle;
        Ok(Arc::new(Self {
            config: config.clone(),
            pool,
            shutdown,
            admission: Arc::new(runtime::pool::AdmissionGate::new_with_limits(
                config.global_request_limit,
                config.subscription_limit,
            )),
            preflight_budget,
            registry,
            #[cfg(feature = "control-plane")]
            identity_link_transactions,
            #[cfg(feature = "control-plane")]
            verified_identity_transactions,
            #[cfg(feature = "control-plane")]
            identity_invitation_port,
            #[cfg(feature = "control-plane")]
            invitation_session_port,
            #[cfg(feature = "control-plane")]
            account_deletion_port,
            authenticator,
            api_key_issuance,
            tenant_resolver,
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
