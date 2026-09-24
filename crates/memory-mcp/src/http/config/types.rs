//! [`HttpConfig`] type, [`HmacKeys`] secrets bundle, [`SignupMode`]
//! enum, the env loader, and the test fixtures.
//!
//! The validator lives in `validate.rs`; everything else (struct
//! shape, env loading, defaults) is here.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use serde::Deserialize;

use crate::error::MemoryError;
use crate::http::registry::models::PlanLimits;

use super::parse::{
    DEFAULT_BIND, DEFAULT_BODY_LIMIT_BYTES, DEFAULT_GLOBAL_REQUEST_LIMIT,
    DEFAULT_MAINTENANCE_PARALLELISM, DEFAULT_OIDC_ALG, DEFAULT_POOL_CAP, DEFAULT_REQUEST_DEADLINE,
    DEFAULT_RUNTIME_ACTIVATION_TIMEOUT, DEFAULT_RUNTIME_CAPACITY_WAIT, DEFAULT_RUNTIME_IDLE_TTL,
    DEFAULT_SHUTDOWN_GRACE, DEFAULT_SUBSCRIPTION_AUTH_RECHECK, DEFAULT_SUBSCRIPTION_LIMIT,
    DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY, DEFAULT_TASK_QUEUE_CAPACITY, DEFAULT_TASK_RETENTION_SECS,
    DEFAULT_TASK_SYNC_MAX_BYTES, TrustedCidr, deserialize_duration_secs, deserialize_hex_32,
    load_signup_plan_limits, optional_env, parse_bool, parse_csv, parse_env_or, parse_hex_32_env,
    require_env,
};
use super::validate::validate;
pub use crate::config::SurrealTargetConfig;

/// Substituted for a secret field by a hand-written `Debug` (plan Task 1:
/// "Redact Debug of config and keys").
const REDACTED: &str = "<redacted>";

/// One browser authentication method (ADR-0057). A deployment enables a **set**
/// of these rather than choosing one, and each enabled method mounts its own
/// routes.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserAuthMethod {
    Local,
    Oidc,
}

impl BrowserAuthMethod {
    /// Every method a deployment can serve. The set is closed: adding one
    /// touches the durable schema, the router, the login page and the removal
    /// guard, which is why the guard may name the tokens individually.
    pub const ALL: [Self; 2] = [Self::Local, Self::Oidc];

    /// The durable token for this method, as stored in `browser_auth_policy`.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Local => AUTH_METHOD_LOCAL,
            Self::Oidc => AUTH_METHOD_OIDC,
        }
    }

    /// Parse a durable token, or `None` for an unknown method so the caller can
    /// fail closed with its own error type.
    pub fn parse(token: &str) -> Option<Self> {
        match token {
            AUTH_METHOD_LOCAL => Some(Self::Local),
            AUTH_METHOD_OIDC => Some(Self::Oidc),
            _ => None,
        }
    }

    /// The enabled methods in the canonical order every representation uses:
    /// `local` before `oidc`.
    ///
    /// A set has one representation however it was enumerated, which is what
    /// lets the durable row, the configuration and the login page be compared
    /// as values instead of as sets.
    pub fn canonical_set(desired: &[Self]) -> Vec<Self> {
        Self::ALL
            .into_iter()
            .filter(|method| desired.contains(method))
            .collect()
    }
}

/// The browser authentication methods this deployment enables, each with the
/// configuration it needs (ADR-0057). `None` on `HttpConfig` when the control
/// plane is disabled; otherwise at least one method is enabled.
#[derive(Debug, Clone, Deserialize)]
pub struct BrowserAuthMethods {
    /// Present when `local` is enabled.
    pub local: Option<LocalBrowserConfig>,
    /// Present when `oidc` is enabled.
    pub oidc: Option<OidcBrowserConfig>,
}

impl BrowserAuthMethods {
    /// Whether `method` is enabled.
    ///
    /// The two `Option`s are the source of truth and the enabled set is
    /// derived from them, so the two can never disagree.
    pub fn has(&self, method: BrowserAuthMethod) -> bool {
        match method {
            BrowserAuthMethod::Local => self.local.is_some(),
            BrowserAuthMethod::Oidc => self.oidc.is_some(),
        }
    }

    /// The enabled methods, in canonical order.
    pub fn enabled(&self) -> Vec<BrowserAuthMethod> {
        let mut methods = Vec::new();
        if self.local.is_some() {
            methods.push(BrowserAuthMethod::Local);
        }
        if self.oidc.is_some() {
            methods.push(BrowserAuthMethod::Oidc);
        }
        methods
    }
}

/// Configuration for local administrator browser authentication.
#[derive(Clone, Deserialize)]
pub struct LocalBrowserConfig {
    /// HMAC key for session signing/verification.
    pub session_key: [u8; 32],
    /// HMAC key for CSRF token signing/verification.
    pub csrf_key: [u8; 32],
    /// Default plan version to ensure at startup.
    pub default_plan_version: u32,
    /// Plan limits for the default local plan.
    pub default_plan_limits: PlanLimits,
}

/// Both keys are raw HMAC key material: redacted. The OIDC variant needs no
/// hand-written impl because its only secret is the already-redacted
/// [`HmacKeys`].
impl fmt::Debug for LocalBrowserConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalBrowserConfig")
            .field("session_key", &REDACTED)
            .field("csrf_key", &REDACTED)
            .field("default_plan_version", &self.default_plan_version)
            .field("default_plan_limits", &self.default_plan_limits)
            .finish()
    }
}

/// Configuration for OIDC browser authentication. Owns the existing
/// OIDC fields that were previously flat on `HttpConfig`.
#[derive(Debug, Clone, Deserialize)]
pub struct OidcBrowserConfig {
    pub issuer: String,
    pub client_id: String,
    pub audience: String,
    pub redirect_uri: String,
    pub allowed_alg: String,
    pub operator_identity_allowlist: Vec<String>,
    pub signup_mode: SignupMode,
    pub keys: HmacKeys,
}

#[derive(Clone, Deserialize)]
pub struct HttpConfig {
    pub bind: SocketAddr,
    pub public_base_url: String,
    /// Mount base path derived from the path of `public_base_url`
    /// (`""` = origin root). See `derive_base_path`.
    pub base_path: String,
    pub trusted_proxy_cidrs: Vec<TrustedCidr>,
    pub allowed_hosts: Vec<String>,
    pub allowed_origins: Vec<String>,
    pub body_limit_bytes: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub request_deadline: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub shutdown_grace: Duration,
    pub pool_cap: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_idle_ttl: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_capacity_wait: Duration,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub runtime_activation_timeout: Duration,
    pub global_request_limit: u32,
    pub subscription_limit: u32,
    pub maintenance_parallelism: usize,
    pub subscription_queue_capacity: usize,
    #[serde(deserialize_with = "deserialize_duration_secs")]
    pub subscription_auth_recheck: Duration,
    pub task_retention_secs: u64,
    pub task_queue_capacity: usize,
    pub task_sync_max_bytes: usize,
    pub control_db: SurrealTargetConfig,
    pub tenant_db: SurrealTargetConfig,
    pub api_key_pepper: String,
    /// Legacy flat keys retained for backward compatibility during
    /// incremental migration to `browser_auth`.
    pub keys: HmacKeys,
    pub oidc_issuer: String,
    pub oidc_client_id: String,
    pub oidc_audience: String,
    pub oidc_redirect_uri: String,
    pub oidc_allowed_alg: String,
    pub operator_identity_allowlist: Vec<String>,
    pub signup_mode: SignupMode,
    pub enable_control_plane: bool,
    pub enable_control_plane_ui: bool,
    /// Explicit plan values for open signup. The plan is persisted in the
    /// durable Registry at startup and is never read from request input.
    #[serde(skip)]
    pub signup_plan_limits: Option<PlanLimits>,
    /// Mode-specific browser authentication configuration. `None` when
    /// the control plane is disabled. When `Some`, it owns the mode
    /// selection and all mode-specific secrets/limits.
    #[serde(skip)]
    pub browser_auth: Option<BrowserAuthMethods>,
}

/// `HttpConfig` carries four independent secret classes — the API-key pepper,
/// the five HMAC keys, the local session/CSRF keys (through `browser_auth`) and
/// the two SurrealDB passwords — so its `Debug` is hand-written rather than
/// derived. Add any future secret field here as `&REDACTED`.
impl fmt::Debug for HttpConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpConfig")
            .field("bind", &self.bind)
            .field("public_base_url", &self.public_base_url)
            .field("base_path", &self.base_path)
            .field("trusted_proxy_cidrs", &self.trusted_proxy_cidrs)
            .field("allowed_hosts", &self.allowed_hosts)
            .field("allowed_origins", &self.allowed_origins)
            .field("body_limit_bytes", &self.body_limit_bytes)
            .field("request_deadline", &self.request_deadline)
            .field("shutdown_grace", &self.shutdown_grace)
            .field("pool_cap", &self.pool_cap)
            .field("runtime_idle_ttl", &self.runtime_idle_ttl)
            .field("runtime_capacity_wait", &self.runtime_capacity_wait)
            .field(
                "runtime_activation_timeout",
                &self.runtime_activation_timeout,
            )
            .field("global_request_limit", &self.global_request_limit)
            .field("subscription_limit", &self.subscription_limit)
            .field("maintenance_parallelism", &self.maintenance_parallelism)
            .field(
                "subscription_queue_capacity",
                &self.subscription_queue_capacity,
            )
            .field("subscription_auth_recheck", &self.subscription_auth_recheck)
            .field("task_retention_secs", &self.task_retention_secs)
            .field("task_queue_capacity", &self.task_queue_capacity)
            .field("task_sync_max_bytes", &self.task_sync_max_bytes)
            .field("control_db", &RedactedTarget(&self.control_db))
            .field("tenant_db", &RedactedTarget(&self.tenant_db))
            .field("api_key_pepper", &REDACTED)
            .field("keys", &self.keys)
            .field("oidc_issuer", &self.oidc_issuer)
            .field("oidc_client_id", &self.oidc_client_id)
            .field("oidc_audience", &self.oidc_audience)
            .field("oidc_redirect_uri", &self.oidc_redirect_uri)
            .field("oidc_allowed_alg", &self.oidc_allowed_alg)
            .field(
                "operator_identity_allowlist",
                &self.operator_identity_allowlist,
            )
            .field("signup_mode", &self.signup_mode)
            .field("enable_control_plane", &self.enable_control_plane)
            .field("enable_control_plane_ui", &self.enable_control_plane_ui)
            .field("signup_plan_limits", &self.signup_plan_limits)
            .field("browser_auth", &self.browser_auth)
            .finish()
    }
}

/// A [`SurrealTargetConfig`] with its password withheld, for use inside the
/// hand-written [`HttpConfig`] `Debug`. The target type's own `Debug` is left
/// as it was: it predates this feature and is printed from call sites outside
/// the local-admin surface.
struct RedactedTarget<'a>(&'a SurrealTargetConfig);

impl fmt::Debug for RedactedTarget<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SurrealTargetConfig")
            .field("url", &self.0.url)
            .field("username", &self.0.username)
            .field("password", &REDACTED)
            .field("database", &self.0.database)
            .field("namespace", &self.0.namespace)
            .finish()
    }
}

#[derive(Clone, Copy, Deserialize)]
pub struct HmacKeys {
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub identity_index: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub control_plane_session: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_state: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub oidc_nonce: [u8; 32],
    #[serde(deserialize_with = "deserialize_hex_32")]
    pub csrf: [u8; 32],
}

/// Every field is raw HMAC key material, so a derived `Debug` would publish all
/// five keys to any log that prints the config.
impl fmt::Debug for HmacKeys {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HmacKeys")
            .field("identity_index", &REDACTED)
            .field("control_plane_session", &REDACTED)
            .field("oidc_state", &REDACTED)
            .field("oidc_nonce", &REDACTED)
            .field("csrf", &REDACTED)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignupMode {
    InviteOnly,
    Open,
}

/// Resolve the enabled browser authentication methods (ADR-0057).
///
/// `MEMORY_MCP_HTTP_AUTH_METHODS` is the contract: a comma-separated set. The
/// deprecated `MEMORY_MCP_HTTP_AUTH_MODE` is still accepted for one release as
/// a one-element set and must not contradict a set that is also supplied.
/// Supplying neither keeps the historical default of `oidc` alone.
///
/// Public within the crate because the admin CLI reads the same contract: one
/// resolver means the CLI and the server can never disagree about which methods
/// a deployment enables.
pub(crate) fn resolve_auth_methods() -> Result<Vec<BrowserAuthMethod>, MemoryError> {
    let set = optional_env("MEMORY_MCP_HTTP_AUTH_METHODS");
    let legacy = optional_env("MEMORY_MCP_HTTP_AUTH_MODE");
    let Some(set) = set else {
        return match legacy.as_deref() {
            None => Ok(vec![BrowserAuthMethod::Oidc]),
            Some(token) => BrowserAuthMethod::parse(token)
                .map(|method| vec![method])
                .ok_or_else(|| auth_method_error(token)),
        };
    };
    let mut methods = Vec::new();
    for token in set.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let method = BrowserAuthMethod::parse(token).ok_or_else(|| auth_method_error(token))?;
        if !methods.contains(&method) {
            methods.push(method);
        }
    }
    if methods.is_empty() {
        return Err(MemoryError::ConfigInvalid(
            "MEMORY_MCP_HTTP_AUTH_METHODS must name at least one method".into(),
        ));
    }
    if let Some(token) = legacy.as_deref() {
        match BrowserAuthMethod::parse(token) {
            Some(method) if methods.contains(&method) => {}
            Some(_) => {
                return Err(MemoryError::ConfigInvalid(format!(
                    "MEMORY_MCP_HTTP_AUTH_MODE={token} contradicts \
                     MEMORY_MCP_HTTP_AUTH_METHODS={set}; set only one of them"
                )));
            }
            None => return Err(auth_method_error(token)),
        }
    }
    Ok(methods)
}

fn auth_method_error(token: &str) -> MemoryError {
    MemoryError::ConfigInvalid(format!(
        "browser authentication methods must be 'local' or 'oidc', got '{token}'"
    ))
}

/// Build the `local` method's configuration, demanding the plan material that
/// method uses and nothing else.
fn build_local_browser_config() -> Result<LocalBrowserConfig, MemoryError> {
    let default_plan_version: u32 = require_env("MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION")?
        .parse()
        .map_err(|_| {
            MemoryError::ConfigInvalid(
                "MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION must be a positive u32".into(),
            )
        })?;
    if default_plan_version == 0 {
        return Err(MemoryError::ConfigInvalid(
            "default plan version must be positive".into(),
        ));
    }
    let default_plan_limits = load_signup_plan_limits()?.ok_or_else(|| {
        MemoryError::ConfigInvalid(
            "the 'local' browser authentication method requires all seven plan limit \
             environment variables"
                .into(),
        )
    })?;
    Ok(LocalBrowserConfig {
        session_key: parse_hex_32_env("MEMORY_MCP_HTTP_SESSION_KEY")?,
        csrf_key: parse_hex_32_env("MEMORY_MCP_HTTP_CSRF_KEY")?,
        default_plan_version,
        default_plan_limits,
    })
}

/// The two method tokens `MEMORY_MCP_HTTP_AUTH_METHODS` accepts, shared by the
/// parser, the validator and the durable policy so no caller re-types the
/// literal.
pub const AUTH_METHOD_LOCAL: &str = "local";
pub const AUTH_METHOD_OIDC: &str = "oidc";

impl HttpConfig {
    /// Loads the HTTP config from process environment variables.
    pub fn from_env() -> Result<Self, MemoryError> {
        let default_bind: SocketAddr = DEFAULT_BIND
            .parse()
            .map_err(|e| MemoryError::ConfigInvalid(format!("DEFAULT_BIND parse failed: {e}")))?;
        let bind = parse_env_or("MEMORY_MCP_HTTP_BIND", default_bind)?;
        let public_base_url = require_env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL")?;
        let base_path = super::parse::derive_base_path(&public_base_url)?;
        let allowed_hosts = parse_csv("ALLOWED_HOSTS")?;
        let allowed_origins = parse_csv("ALLOWED_ORIGINS")?;
        let body_limit_bytes: usize =
            parse_env_or("MEMORY_MCP_HTTP_BODY_LIMIT", DEFAULT_BODY_LIMIT_BYTES)?;
        let request_deadline = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS",
            DEFAULT_REQUEST_DEADLINE.as_secs(),
        )?);
        let shutdown_grace = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS",
            DEFAULT_SHUTDOWN_GRACE.as_secs(),
        )?);
        let pool_cap = parse_env_or("MEMORY_MCP_HTTP_POOL_CAP", DEFAULT_POOL_CAP)?;
        let runtime_idle_ttl = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS",
            DEFAULT_RUNTIME_IDLE_TTL.as_secs(),
        )?);
        let runtime_capacity_wait = Duration::from_millis(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS",
            DEFAULT_RUNTIME_CAPACITY_WAIT.as_millis() as u64,
        )?);
        let runtime_activation_timeout = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
            DEFAULT_RUNTIME_ACTIVATION_TIMEOUT.as_secs(),
        )?);
        let global_request_limit = parse_env_or(
            "MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT",
            DEFAULT_GLOBAL_REQUEST_LIMIT,
        )?;
        let subscription_limit = parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT",
            DEFAULT_SUBSCRIPTION_LIMIT,
        )?;
        let maintenance_parallelism = parse_env_or(
            "MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM",
            DEFAULT_MAINTENANCE_PARALLELISM,
        )?;
        let subscription_queue_capacity = parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY",
            DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY,
        )?;
        let subscription_auth_recheck = Duration::from_secs(parse_env_or(
            "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
            DEFAULT_SUBSCRIPTION_AUTH_RECHECK.as_secs(),
        )?);
        let task_retention_secs = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_RETENTION_SECS",
            DEFAULT_TASK_RETENTION_SECS,
        )?;
        let task_queue_capacity = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY",
            DEFAULT_TASK_QUEUE_CAPACITY,
        )?;
        let task_sync_max_bytes = parse_env_or(
            "MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES",
            DEFAULT_TASK_SYNC_MAX_BYTES,
        )?;
        let trusted_proxy_cidrs = parse_csv("MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS")?
            .into_iter()
            .map(|s| TrustedCidr::parse(&s))
            .collect::<Result<Vec<_>, _>>()?;
        // One root secret can derive every secret slot (the five HMAC keys and
        // the API-key pepper). An explicit variable always wins over the root
        // derivation for its slot, and without either form the original
        // `ConfigMissing` contract stands: secret material is never invented.
        let root_secret = optional_env("MEMORY_MCP_HTTP_SECRET_KEY");
        let api_key_pepper = match optional_env("MEMORY_MCP_API_KEY_PEPPER") {
            Some(supplied) => supplied,
            None => match root_secret.as_deref() {
                Some(root) => hex::encode(derive_key_from_root_secret(
                    root,
                    "MEMORY_MCP_API_KEY_PEPPER",
                )?),
                None => require_env("MEMORY_MCP_API_KEY_PEPPER")?,
            },
        };
        // Enable flags and the method set are read before any secret so a
        // deployment that does not enable `oidc` is never forced to configure
        // OIDC-only material, and so each method's keys are demanded only when
        // that method is enabled.
        let enable_control_plane = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", false)?;
        let enable_control_plane_ui = parse_bool("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", false)?;
        let configured_methods = if enable_control_plane {
            resolve_auth_methods()?
        } else {
            Vec::new()
        };
        // `local` derives the three key slots it cannot use from the session
        // key, which is only coherent while it is the *whole* set: with `oidc`
        // enabled those slots are real material for a real provider.
        let derives_local_keys = configured_methods == [BrowserAuthMethod::Local];

        // Slot resolution: an explicit variable wins, then the root-secret
        // derivation, then (for the three OIDC-typed slots under a `local`-
        // only set) the local-mode derivation from the session key.
        let control_plane_session = resolve_key_slot(
            "MEMORY_MCP_HTTP_SESSION_KEY",
            root_secret.as_deref(),
            || parse_hex_32_env("MEMORY_MCP_HTTP_SESSION_KEY"),
        )?;
        let local_slot = |label: &str| {
            if derives_local_keys {
                derive_local_key(&control_plane_session, label)
            } else {
                parse_hex_32_env(label)
            }
        };
        let identity_index = resolve_key_slot(
            "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY",
            root_secret.as_deref(),
            || local_slot("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY"),
        )?;
        let oidc_state = resolve_key_slot(
            "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
            root_secret.as_deref(),
            || local_slot("MEMORY_MCP_HTTP_OIDC_STATE_KEY"),
        )?;
        let oidc_nonce = resolve_key_slot(
            "MEMORY_MCP_HTTP_OIDC_NONCE_KEY",
            root_secret.as_deref(),
            || local_slot("MEMORY_MCP_HTTP_OIDC_NONCE_KEY"),
        )?;
        let csrf = resolve_key_slot("MEMORY_MCP_HTTP_CSRF_KEY", root_secret.as_deref(), || {
            parse_hex_32_env("MEMORY_MCP_HTTP_CSRF_KEY")
        })?;
        let keys = HmacKeys {
            identity_index,
            control_plane_session,
            oidc_state,
            oidc_nonce,
            csrf,
        };
        // Signup policy: only the `oidc` method has an identity provider to
        // sign up with, so a set without it is invite-only and rejects an
        // explicit `open` rather than silently ignoring it. Defaulting the
        // variable keeps a local environment from carrying an OIDC-only key.
        let signup_mode = if configured_methods.contains(&BrowserAuthMethod::Oidc) {
            // Zero-config default: `invite_only` is the safe policy; `open` is
            // always an explicit choice.
            match optional_env("MEMORY_MCP_HTTP_SIGNUP_MODE").as_deref() {
                None | Some("invite_only") => SignupMode::InviteOnly,
                Some("open") => SignupMode::Open,
                Some(other) => {
                    return Err(MemoryError::ConfigInvalid(format!("signup mode: {other}")));
                }
            }
        } else {
            match optional_env("MEMORY_MCP_HTTP_SIGNUP_MODE").as_deref() {
                None | Some("invite_only") => SignupMode::InviteOnly,
                Some("open") => {
                    return Err(MemoryError::ConfigInvalid(
                        "signup mode 'open' requires the 'oidc' authentication method".into(),
                    ));
                }
                Some(other) => {
                    return Err(MemoryError::ConfigInvalid(format!("signup mode: {other}")));
                }
            }
        };
        let signup_plan_limits = load_signup_plan_limits()?;
        let oidc_issuer = optional_env("MEMORY_MCP_HTTP_OIDC_ISSUER").unwrap_or_default();
        let oidc_client_id = optional_env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID").unwrap_or_default();
        let oidc_audience = optional_env("MEMORY_MCP_HTTP_OIDC_AUDIENCE").unwrap_or_default();
        let oidc_redirect_uri =
            optional_env("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI").unwrap_or_default();
        let oidc_allowed_alg = optional_env("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG")
            .unwrap_or_else(|| DEFAULT_OIDC_ALG.into());
        let operator_identity_allowlist = parse_csv("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES")?;

        // Material for a method this deployment does not enable is a
        // configuration error rather than something to ignore: a provider that
        // is configured but not enabled is a deployment that believes it has
        // SSO when it does not.
        if !configured_methods.contains(&BrowserAuthMethod::Oidc) {
            let stray = [
                ("MEMORY_MCP_HTTP_OIDC_ISSUER", &oidc_issuer),
                ("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", &oidc_client_id),
                ("MEMORY_MCP_HTTP_OIDC_AUDIENCE", &oidc_audience),
                ("MEMORY_MCP_HTTP_OIDC_REDIRECT_URI", &oidc_redirect_uri),
            ]
            .into_iter()
            .find_map(|(name, value)| (!value.is_empty()).then_some(name))
            .or_else(|| {
                (oidc_allowed_alg != DEFAULT_OIDC_ALG).then_some("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG")
            })
            .or_else(|| {
                (!operator_identity_allowlist.is_empty())
                    .then_some("MEMORY_MCP_HTTP_OPERATOR_IDENTITIES")
            });
            if let Some(name) = stray {
                return Err(MemoryError::ConfigInvalid(format!(
                    "{name} is set but the 'oidc' browser authentication method is not enabled; \
                     add 'oidc' to MEMORY_MCP_HTTP_AUTH_METHODS"
                )));
            }
        }
        // Zero-config derivation: only the issuer and the client id are
        // vitally necessary. The audience defaults to the client id (OIDC
        // Core: `aud` is the RP's client id), the redirect URI to this
        // deployment's own callback under the public base URL, and the
        // algorithm allowlist to `auto`. A supplied value always wins.
        let (oidc_audience, oidc_redirect_uri, oidc_allowed_alg) =
            if configured_methods.contains(&BrowserAuthMethod::Oidc) {
                let audience = if oidc_audience.is_empty() {
                    oidc_client_id.clone()
                } else {
                    oidc_audience
                };
                let redirect_uri = if oidc_redirect_uri.is_empty() {
                    super::parse::derive_oidc_redirect_uri(&public_base_url)
                } else {
                    oidc_redirect_uri
                };
                (audience, redirect_uri, oidc_allowed_alg)
            } else {
                (oidc_audience, oidc_redirect_uri, oidc_allowed_alg)
            };
        // The three OIDC-typed HMAC slots are real key material in every set
        // that enables `oidc`, and are derived from the session key only while
        // `local` is the *whole* set. A deployment that supplies one there is
        // ambiguous about which key it means, so it is refused rather than
        // silently overridden. Only the parser can tell a supplied value from
        // the derived one.
        if derives_local_keys
            && let Some(name) = [
                "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY",
                "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
                "MEMORY_MCP_HTTP_OIDC_NONCE_KEY",
            ]
            .into_iter()
            .find(|name| optional_env(name).is_some())
        {
            return Err(MemoryError::ConfigInvalid(format!(
                "{name} is set but the 'oidc' browser authentication method is not enabled; \
                 add 'oidc' to MEMORY_MCP_HTTP_AUTH_METHODS"
            )));
        }

        // One configuration per enabled method, so each method demands exactly
        // the material it uses (ADR-0057).
        let browser_auth = if enable_control_plane {
            Some(BrowserAuthMethods {
                local: configured_methods
                    .contains(&BrowserAuthMethod::Local)
                    .then(build_local_browser_config)
                    .transpose()?,
                oidc: configured_methods
                    .contains(&BrowserAuthMethod::Oidc)
                    .then(|| OidcBrowserConfig {
                        issuer: oidc_issuer.clone(),
                        client_id: oidc_client_id.clone(),
                        audience: oidc_audience.clone(),
                        redirect_uri: oidc_redirect_uri.clone(),
                        allowed_alg: oidc_allowed_alg.clone(),
                        operator_identity_allowlist: operator_identity_allowlist.clone(),
                        signup_mode,
                        keys,
                    }),
            })
        } else {
            None
        };

        let control_db = SurrealTargetConfig {
            url: require_env("SURREALDB_CONTROL_URL")?,
            username: require_env("SURREALDB_CONTROL_USERNAME")?,
            password: require_env("SURREALDB_CONTROL_PASSWORD")?,
            database: require_env("SURREALDB_CONTROL_DB")?,
            namespace: require_env("SURREALDB_CONTROL_NAMESPACE")?,
        };
        let tenant_db = SurrealTargetConfig {
            url: require_env("SURREALDB_TENANT_URL")?,
            username: require_env("SURREALDB_TENANT_USERNAME")?,
            password: require_env("SURREALDB_TENANT_PASSWORD")?,
            database: require_env("SURREALDB_TENANT_DB")?,
            namespace: require_env("SURREALDB_TENANT_NAMESPACE")?,
        };

        let cfg = Self {
            bind,
            public_base_url,
            base_path,
            trusted_proxy_cidrs,
            allowed_hosts,
            allowed_origins,
            body_limit_bytes,
            request_deadline,
            shutdown_grace,
            pool_cap,
            runtime_idle_ttl,
            runtime_capacity_wait,
            runtime_activation_timeout,
            global_request_limit,
            subscription_limit,
            maintenance_parallelism,
            subscription_queue_capacity,
            subscription_auth_recheck,
            task_retention_secs,
            task_queue_capacity,
            task_sync_max_bytes,
            control_db,
            tenant_db,
            api_key_pepper,
            keys,
            oidc_issuer,
            oidc_client_id,
            oidc_audience,
            oidc_redirect_uri,
            oidc_allowed_alg,
            operator_identity_allowlist,
            signup_mode,
            enable_control_plane,
            enable_control_plane_ui,
            signup_plan_limits,
            browser_auth,
        };
        Ok(cfg)
    }

    pub fn validate(&self) -> Result<(), MemoryError> {
        validate(self)
    }

    /// Whether `method` is enabled on this deployment.
    ///
    /// Every consumer that used to ask "is this deployment OIDC?" now asks
    /// about one method, so enabling a second method never changes the answer
    /// for the first.
    pub fn has_method(&self, method: BrowserAuthMethod) -> bool {
        self.browser_auth
            .as_ref()
            .is_some_and(|methods| methods.has(method))
    }

    /// The enabled browser authentication methods, empty when the control plane
    /// is disabled.
    pub fn browser_auth_methods(&self) -> Vec<BrowserAuthMethod> {
        self.browser_auth
            .as_ref()
            .map(BrowserAuthMethods::enabled)
            .unwrap_or_default()
    }
}

#[cfg(any(test, feature = "test-fixtures"))]
impl HttpConfig {
    pub fn default_for_test() -> Self {
        let control_db = SurrealTargetConfig::default_for_test();
        let mut tenant_db = SurrealTargetConfig::default_for_test();
        tenant_db.database = "memory_tenant_test".into();
        tenant_db.namespace = "tenant_test".into();
        Self {
            bind: "127.0.0.1:0".parse().expect("test bind"),
            public_base_url: "http://localhost".into(),
            base_path: String::new(),
            trusted_proxy_cidrs: Vec::new(),
            allowed_hosts: vec!["localhost".into(), "127.0.0.1".into()],
            allowed_origins: vec!["http://localhost".into()],
            body_limit_bytes: DEFAULT_BODY_LIMIT_BYTES,
            request_deadline: DEFAULT_REQUEST_DEADLINE,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
            pool_cap: DEFAULT_POOL_CAP,
            runtime_idle_ttl: DEFAULT_RUNTIME_IDLE_TTL,
            runtime_capacity_wait: DEFAULT_RUNTIME_CAPACITY_WAIT,
            runtime_activation_timeout: DEFAULT_RUNTIME_ACTIVATION_TIMEOUT,
            global_request_limit: DEFAULT_GLOBAL_REQUEST_LIMIT,
            subscription_limit: DEFAULT_SUBSCRIPTION_LIMIT,
            maintenance_parallelism: DEFAULT_MAINTENANCE_PARALLELISM,
            subscription_queue_capacity: DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY,
            subscription_auth_recheck: DEFAULT_SUBSCRIPTION_AUTH_RECHECK,
            task_retention_secs: DEFAULT_TASK_RETENTION_SECS,
            task_queue_capacity: DEFAULT_TASK_QUEUE_CAPACITY,
            task_sync_max_bytes: DEFAULT_TASK_SYNC_MAX_BYTES,
            control_db,
            tenant_db,
            api_key_pepper: "x".repeat(40),
            keys: HmacKeys {
                identity_index: [0; 32],
                control_plane_session: [0; 32],
                oidc_state: [0; 32],
                oidc_nonce: [0; 32],
                csrf: [0; 32],
            },
            oidc_issuer: "https://issuer.invalid".into(),
            oidc_client_id: "test-client".into(),
            oidc_audience: "memory-mcp".into(),
            oidc_redirect_uri: "http://localhost/auth/oidc/callback".into(),
            oidc_allowed_alg: DEFAULT_OIDC_ALG.into(),
            operator_identity_allowlist: Vec::new(),
            signup_mode: SignupMode::InviteOnly,
            enable_control_plane: false,
            enable_control_plane_ui: false,
            signup_plan_limits: None,
            browser_auth: Some(BrowserAuthMethods {
                local: None,
                oidc: Some(OidcBrowserConfig {
                    issuer: "https://issuer.invalid".into(),
                    client_id: "test-client".into(),
                    audience: "memory-mcp".into(),
                    redirect_uri: "http://localhost/auth/oidc/callback".into(),
                    allowed_alg: DEFAULT_OIDC_ALG.into(),
                    operator_identity_allowlist: Vec::new(),
                    signup_mode: SignupMode::InviteOnly,
                    keys: HmacKeys {
                        identity_index: [0; 32],
                        control_plane_session: [0; 32],
                        oidc_state: [0; 32],
                        oidc_nonce: [0; 32],
                        csrf: [0; 32],
                    },
                }),
            }),
        }
    }
}

/// Derive a key for an OIDC-typed slot that local mode does not use.
///
/// `label` names the absent environment variable. The derived value comes
/// from the local session key under a purpose-separated label, so it is
/// never a zero key (which would make an accidental use trivially
/// forgeable) and never equal to another slot's value.
fn derive_local_key(session: &[u8; 32], label: &str) -> Result<[u8; 32], MemoryError> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    let mut mac = Hmac::<Sha256>::new_from_slice(session)
        .map_err(|_| MemoryError::ConfigInvalid("invalid session key".into()))?;
    mac.update(b"local_mode_derived_key\0");
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// Purpose-separated derivation from `MEMORY_MCP_HTTP_SECRET_KEY`: one root
/// secret yields every slot, each under the absent variable's name as its
/// label, so derived slots are never zero and never equal to a sibling.
fn derive_key_from_root_secret(root: &str, label: &str) -> Result<[u8; 32], MemoryError> {
    use hmac::{Hmac, KeyInit, Mac};
    use sha2::Sha256;

    let mut mac = Hmac::<Sha256>::new_from_slice(root.as_bytes())
        .map_err(|_| MemoryError::ConfigInvalid("invalid root secret".into()))?;
    mac.update(b"memory_mcp_http_secret_key\0");
    mac.update(label.as_bytes());
    Ok(mac.finalize().into_bytes().into())
}

/// Resolve one 32-byte HMAC key slot: an explicit value wins, then the
/// root-secret derivation, then the caller's fallback (which preserves the
/// original `ConfigMissing` contract).
fn resolve_key_slot(
    env_name: &str,
    root_secret: Option<&str>,
    fallback: impl FnOnce() -> Result<[u8; 32], MemoryError>,
) -> Result<[u8; 32], MemoryError> {
    match optional_env(env_name) {
        Some(raw) => {
            let bytes =
                hex::decode(raw).map_err(|_| MemoryError::ConfigInvalid(env_name.into()))?;
            bytes
                .try_into()
                .map_err(|_| MemoryError::ConfigInvalid(env_name.into()))
        }
        None => match root_secret {
            Some(root) => derive_key_from_root_secret(root, env_name),
            None => fallback(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::Mutex;

    // Edition 2024: set_var/remove_var are unsafe. ENV_LOCK serializes all
    // env-mutating tests in this module, which is the safety condition.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for k in [
            "MEMORY_MCP_HTTP_BIND",
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL",
            "ALLOWED_HOSTS",
            "ALLOWED_ORIGINS",
            "MEMORY_MCP_API_KEY_PEPPER",
            "MEMORY_MCP_HTTP_SIGNUP_MODE",
            "MEMORY_MCP_HTTP_BODY_LIMIT",
            "MEMORY_MCP_HTTP_REQUEST_DEADLINE_SECS",
            "MEMORY_MCP_HTTP_SHUTDOWN_GRACE_SECS",
            "MEMORY_MCP_HTTP_TRUSTED_PROXY_CIDRS",
            "SURREALDB_CONTROL_URL",
            "SURREALDB_CONTROL_USERNAME",
            "SURREALDB_CONTROL_PASSWORD",
            "SURREALDB_CONTROL_DB",
            "SURREALDB_CONTROL_NAMESPACE",
            "SURREALDB_TENANT_URL",
            "SURREALDB_TENANT_USERNAME",
            "SURREALDB_TENANT_PASSWORD",
            "SURREALDB_TENANT_DB",
            "SURREALDB_TENANT_NAMESPACE",
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE",
            "MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI",
            "MEMORY_MCP_HTTP_CSRF_KEY",
            "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
            "MEMORY_MCP_HTTP_OIDC_NONCE_KEY",
            "MEMORY_MCP_HTTP_SESSION_KEY",
            "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY",
            "MEMORY_MCP_HTTP_OIDC_ISSUER",
            "MEMORY_MCP_HTTP_OIDC_CLIENT_ID",
            "MEMORY_MCP_HTTP_OIDC_AUDIENCE",
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
            "MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG",
            "MEMORY_MCP_HTTP_SECRET_KEY",
            "MEMORY_MCP_HTTP_AUTH_METHODS",
            "MEMORY_MCP_HTTP_OPERATOR_IDENTITIES",
            "MEMORY_MCP_HTTP_MAX_INGESTED_BYTES",
            "MEMORY_MCP_HTTP_MAX_EPISODE_COUNT",
            "MEMORY_MCP_HTTP_INGEST_PER_MINUTE",
            "MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS",
            "MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS",
            "MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY",
            "MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY",
            "MEMORY_MCP_HTTP_POOL_CAP",
            "MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS",
            "MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS",
            "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
            "MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT",
            "MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY",
            "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
            "MEMORY_MCP_HTTP_TASK_RETENTION_SECS",
            "MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY",
            "MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES",
        ] {
            // SAFETY: serialized by ENV_LOCK; no other thread reads these vars in tests.
            unsafe {
                env::remove_var(k);
            }
        }
        for (k, v) in vars {
            // SAFETY: same as above.
            unsafe {
                env::set_var(k, v);
            }
        }
        f();
        for (k, _) in vars {
            // SAFETY: same as above.
            unsafe {
                env::remove_var(k);
            }
        }
    }

    fn base_required_env() -> Vec<(&'static str, String)> {
        let pepper = "x".repeat(40);
        let key = "0".repeat(64);
        vec![
            ("MEMORY_MCP_HTTP_BIND", "127.0.0.1:8080".into()),
            ("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost".into()),
            ("ALLOWED_HOSTS", "localhost".into()),
            ("ALLOWED_ORIGINS", "http://localhost".into()),
            ("MEMORY_MCP_API_KEY_PEPPER", pepper),
            ("MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_SIGNUP_MODE", "invite_only".into()),
            ("MEMORY_MCP_HTTP_CSRF_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_OIDC_STATE_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_OIDC_NONCE_KEY", key.clone()),
            ("MEMORY_MCP_HTTP_SESSION_KEY", key),
            ("SURREALDB_CONTROL_URL", "ws://localhost:8000".into()),
            ("SURREALDB_CONTROL_USERNAME", "root".into()),
            ("SURREALDB_CONTROL_PASSWORD", "root".into()),
            ("SURREALDB_CONTROL_DB", "control".into()),
            ("SURREALDB_CONTROL_NAMESPACE", "control".into()),
            ("SURREALDB_TENANT_URL", "ws://localhost:8000".into()),
            ("SURREALDB_TENANT_USERNAME", "root".into()),
            ("SURREALDB_TENANT_PASSWORD", "root".into()),
            ("SURREALDB_TENANT_DB", "tenant".into()),
            ("SURREALDB_TENANT_NAMESPACE", "tenant".into()),
            ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "false".into()),
            ("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI", "false".into()),
        ]
    }

    #[test]
    fn default_for_test_validates() {
        HttpConfig::default_for_test().validate().expect("valid");
    }

    #[test]
    fn http_config_loads_from_env_with_minimum_required() {
        let vars = base_required_env();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate().expect("valid");
            assert_eq!(cfg.bind.port(), 8080);
            assert_eq!(cfg.allowed_hosts, vec!["localhost".to_string()]);
            assert_eq!(cfg.signup_mode, SignupMode::InviteOnly);
        });
    }

    /// The required environment for a *local-mode* control plane, with every
    /// OIDC-only key removed: local mode derives identity/state/nonce from the
    /// session key and rejects a supplied value (spec §3).
    fn local_mode_env() -> Vec<(&'static str, String)> {
        let mut vars: Vec<(&'static str, String)> = base_required_env()
            .into_iter()
            .filter(|(k, _)| {
                !matches!(
                    *k,
                    "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY"
                        | "MEMORY_MCP_HTTP_OIDC_STATE_KEY"
                        | "MEMORY_MCP_HTTP_OIDC_NONCE_KEY"
                )
            })
            .collect();
        vars.push(("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "true".into()));
        vars.push(("MEMORY_MCP_HTTP_AUTH_METHODS", "local".into()));
        vars.push(("MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION", "1".into()));
        vars.extend(plan_limit_env());
        vars
    }

    #[test]
    fn local_mode_loads_without_oidc_only_keys() {
        let vars = local_mode_env();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("a local-mode deployment loads");
            assert!(cfg.has_method(BrowserAuthMethod::Local));
            assert!(!cfg.has_method(BrowserAuthMethod::Oidc));
            assert!(cfg.oidc_issuer.is_empty(), "local mode carries no issuer");
            assert!(cfg.oidc_client_id.is_empty());
        });
    }

    /// `MEMORY_MCP_HTTP_AUTH_MODE` is a one-release alias for a one-element
    /// `MEMORY_MCP_HTTP_AUTH_METHODS`. Supplying only the alias still works,
    /// and a set that contradicts it is refused rather than silently winning.
    #[test]
    fn auth_mode_is_a_one_element_alias_for_the_method_set() {
        let mut vars = local_mode_env();
        vars.retain(|(k, _)| *k != "MEMORY_MCP_HTTP_AUTH_METHODS");
        vars.push(("MEMORY_MCP_HTTP_AUTH_MODE", "local".into()));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("the alias alone still resolves");
            assert_eq!(cfg.browser_auth_methods(), vec![BrowserAuthMethod::Local]);
        });

        let mut vars = local_mode_env();
        vars.push(("MEMORY_MCP_HTTP_AUTH_MODE", "oidc".into()));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            assert!(
                matches!(
                    HttpConfig::from_env(),
                    Err(MemoryError::ConfigInvalid(ref message)) if message.contains("contradicts")
                ),
                "an alias that disagrees with the set must fail startup"
            );
        });
    }

    /// Both methods enabled is configuration, not a migration: the OIDC
    /// material is required rather than forbidden, and the local material is
    /// required as well (ADR-0057).
    #[test]
    fn both_methods_load_and_validate_together() {
        let mut vars = base_required_env();
        vars.push(("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "true".into()));
        vars.push(("MEMORY_MCP_HTTP_AUTH_METHODS", "oidc,local".into()));
        vars.push(("MEMORY_MCP_HTTP_LOCAL_DEFAULT_PLAN_VERSION", "1".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_ISSUER",
            "https://issuer.example.com".into(),
        ));
        vars.push(("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", "test-client".into()));
        vars.push(("MEMORY_MCP_HTTP_OIDC_AUDIENCE", "memory-mcp".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
            "https://memory.example.com/callback".into(),
        ));
        vars.push((
            "MEMORY_MCP_HTTP_PUBLIC_BASE_URL",
            "https://memory.example.com".into(),
        ));
        vars.extend(plan_limit_env());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("both methods load");
            assert_eq!(
                cfg.browser_auth_methods(),
                vec![BrowserAuthMethod::Local, BrowserAuthMethod::Oidc]
            );
            cfg.validate().expect("both methods validate together");
        });
    }

    #[test]
    fn local_mode_rejects_a_supplied_oidc_only_key() {
        // Spec §3: nonempty OIDC-only configuration in enabled local mode
        // fails startup rather than being silently ignored. Only the parser
        // can tell a supplied value from the derived one.
        for name in [
            "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY",
            "MEMORY_MCP_HTTP_OIDC_STATE_KEY",
            "MEMORY_MCP_HTTP_OIDC_NONCE_KEY",
        ] {
            let mut vars = local_mode_env();
            vars.push((name, "0".repeat(64)));
            let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
            with_env(&refs, || {
                assert!(
                    matches!(
                        HttpConfig::from_env(),
                        Err(MemoryError::ConfigInvalid(ref message))
                            if message.contains("not enabled")
                    ),
                    "{name} must fail startup in local mode"
                );
            });
        }
    }

    #[test]
    fn open_signup_loads_explicit_plan_limits() {
        let mut vars = base_required_env();
        vars[6] = ("MEMORY_MCP_HTTP_SIGNUP_MODE", "open".into());
        vars.push(("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "true".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_ISSUER",
            "https://issuer.example.com".into(),
        ));
        vars.push(("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", "test-client".into()));
        vars.push(("MEMORY_MCP_HTTP_OIDC_AUDIENCE", "memory-mcp".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
            "http://localhost/callback".into(),
        ));
        vars.extend(plan_limit_env());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate()
                .expect("open signup with explicit quotas is valid");
            let limits = cfg.signup_plan_limits.expect("plan limits");
            assert_eq!(limits.max_ingested_bytes, 1000);
            assert_eq!(limits.ingest_per_minute, 3);
            assert_eq!(limits.extraction_concurrency, 4);
        });
    }

    #[test]
    fn http_config_loads_operational_limits() {
        let mut vars = base_required_env();
        vars.extend([
            ("MEMORY_MCP_HTTP_POOL_CAP", "8".into()),
            ("MEMORY_MCP_HTTP_RUNTIME_IDLE_TTL_SECS", "60".into()),
            ("MEMORY_MCP_HTTP_RUNTIME_CAPACITY_WAIT_MS", "250".into()),
            (
                "MEMORY_MCP_HTTP_RUNTIME_ACTIVATION_TIMEOUT_SECS",
                "10".into(),
            ),
            ("MEMORY_MCP_HTTP_GLOBAL_REQUEST_LIMIT", "20".into()),
            ("MEMORY_MCP_HTTP_SUBSCRIPTION_LIMIT", "3".into()),
            ("MEMORY_MCP_HTTP_MAINTENANCE_PARALLELISM", "2".into()),
            ("MEMORY_MCP_HTTP_SUBSCRIPTION_QUEUE_CAPACITY", "16".into()),
            (
                "MEMORY_MCP_HTTP_SUBSCRIPTION_AUTH_RECHECK_SECS",
                "30".into(),
            ),
            ("MEMORY_MCP_HTTP_TASK_RETENTION_SECS", "3600".into()),
            ("MEMORY_MCP_HTTP_TASK_QUEUE_CAPACITY", "64".into()),
            ("MEMORY_MCP_HTTP_TASK_SYNC_MAX_BYTES", "4096".into()),
        ]);
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate().expect("operational limits are valid");
            assert_eq!(cfg.pool_cap, 8);
            assert_eq!(cfg.runtime_capacity_wait, Duration::from_millis(250));
            assert_eq!(cfg.global_request_limit, 20);
            assert_eq!(cfg.subscription_limit, 3);
            assert_eq!(cfg.subscription_queue_capacity, 16);
            assert_eq!(cfg.task_retention_secs, 3600);
            assert_eq!(cfg.task_queue_capacity, 64);
            assert_eq!(cfg.task_sync_max_bytes, 4096);
        });
    }

    #[test]
    fn http_config_rejects_wildcard_origin() {
        let mut vars = base_required_env();
        vars[3] = ("ALLOWED_ORIGINS", "*".into());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("parses");
            assert!(matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(_))));
        });
    }

    #[test]
    fn http_config_rejects_empty_origin_allowlist() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.allowed_origins.clear();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("ALLOWED_ORIGINS")
        ));
    }

    #[test]
    fn http_config_rejects_zero_limits() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.body_limit_bytes = 0;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("BODY_LIMIT")
        ));
        let mut cfg = HttpConfig::default_for_test();
        cfg.request_deadline = std::time::Duration::ZERO;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("deadline")
        ));
    }

    #[test]
    fn http_config_rejects_shared_control_and_tenant_binding() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.tenant_db = cfg.control_db.clone();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("different namespace/database")
        ));
    }

    #[test]
    fn control_plane_requires_complete_oidc_config() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane = true;
        cfg.oidc_issuer.clear();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("OIDC issuer")
        ));
    }

    #[test]
    fn control_plane_rejects_unknown_oidc_algorithm() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane = true;
        cfg.oidc_allowed_alg = "none".into();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("allowed algorithm")
        ));
    }

    /// RS384 and RS512 are advertised by mainstream providers (Rauthy lists
    /// `RS256, RS384, RS512, EdDSA`) and must be selectable in
    /// `MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG` alongside `auto` (the default: the
    /// safe set intersected with the provider's advertisement at discovery).
    #[test]
    fn control_plane_accepts_derived_and_rs_algorithms() {
        for alg in ["auto", "RS384", "RS512"] {
            let mut cfg = HttpConfig::default_for_test();
            cfg.enable_control_plane = true;
            cfg.oidc_allowed_alg = alg.into();
            cfg.validate()
                .unwrap_or_else(|error| panic!("{alg} must be accepted: {error}"));
        }
    }

    /// `oidc`-only env with every derivable OIDC knob removed: the two vitally
    /// necessary ones (issuer, client id) and the secrets stay.
    fn minimal_oidc_env() -> Vec<(&'static str, String)> {
        let mut vars: Vec<(&'static str, String)> = base_required_env()
            .into_iter()
            .filter(|(k, _)| !matches!(*k, "MEMORY_MCP_HTTP_SIGNUP_MODE"))
            .collect();
        vars.push(("MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE", "true".into()));
        vars.push(("MEMORY_MCP_HTTP_AUTH_METHODS", "oidc".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_ISSUER",
            "https://issuer.example.com".into(),
        ));
        vars.push(("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", "test-client".into()));
        vars
    }

    /// `minimal_oidc_env` with every explicit secret replaced by one root
    /// secret.
    fn root_secret_env() -> Vec<(&'static str, String)> {
        let mut vars: Vec<(&'static str, String)> = minimal_oidc_env()
            .into_iter()
            .filter(|(k, _)| {
                !matches!(
                    *k,
                    "MEMORY_MCP_API_KEY_PEPPER"
                        | "MEMORY_MCP_HTTP_IDENTITY_INDEX_KEY"
                        | "MEMORY_MCP_HTTP_CSRF_KEY"
                        | "MEMORY_MCP_HTTP_OIDC_STATE_KEY"
                        | "MEMORY_MCP_HTTP_OIDC_NONCE_KEY"
                        | "MEMORY_MCP_HTTP_SESSION_KEY"
                )
            })
            .collect();
        vars.push((
            "MEMORY_MCP_HTTP_SECRET_KEY",
            "root-secret-with-plenty-of-entropy-0123456789".into(),
        ));
        vars
    }

    /// Zero-config OIDC: only the issuer and client id are vitally necessary.
    /// The audience defaults to the client id (OIDC Core: `aud` is the RP's
    /// client id), the redirect URI to this deployment's own callback under
    /// the public base URL, the algorithm allowlist to `auto`, and the signup
    /// policy to `invite_only`.
    #[test]
    fn unset_oidc_material_is_derived_from_client_id_and_public_url() {
        let vars = minimal_oidc_env();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("minimal oidc config loads");
            cfg.validate().expect("minimal oidc config validates");
            assert_eq!(cfg.oidc_audience, "test-client");
            assert_eq!(cfg.oidc_redirect_uri, "http://localhost/auth/oidc/callback");
            assert_eq!(cfg.oidc_allowed_alg, "auto");
            assert_eq!(cfg.signup_mode, SignupMode::InviteOnly);
        });
    }

    /// Supplied values win over every derivation, so providers that do not
    /// follow the common shapes keep their escape hatch.
    #[test]
    fn explicit_oidc_material_overrides_the_derivations() {
        let mut vars = minimal_oidc_env();
        vars.push(("MEMORY_MCP_HTTP_OIDC_AUDIENCE", "custom-aud".into()));
        vars.push((
            "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
            "https://memory.example.com/custom-callback".into(),
        ));
        vars.push(("MEMORY_MCP_HTTP_OIDC_ALLOWED_ALG", "EdDSA".into()));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            cfg.validate().expect("valid");
            assert_eq!(cfg.oidc_audience, "custom-aud");
            assert_eq!(
                cfg.oidc_redirect_uri,
                "https://memory.example.com/custom-callback"
            );
            assert_eq!(cfg.oidc_allowed_alg, "EdDSA");
        });
    }

    #[test]
    fn the_oidc_callback_derives_from_the_public_url() {
        assert_eq!(
            crate::http::config::parse::derive_oidc_redirect_uri("https://memory.example.com"),
            "https://memory.example.com/auth/oidc/callback"
        );
        assert_eq!(
            crate::http::config::parse::derive_oidc_redirect_uri(
                "https://memory.example.com/memory"
            ),
            "https://memory.example.com/memory/auth/oidc/callback"
        );
        assert_eq!(
            crate::http::config::parse::derive_oidc_redirect_uri(
                "https://memory.example.com/memory/"
            ),
            "https://memory.example.com/memory/auth/oidc/callback"
        );
    }

    /// One root secret replaces the five HMAC keys and the API-key pepper:
    /// each slot is a purpose-separated derivation, never equal to a sibling.
    #[test]
    fn a_root_secret_derives_every_secret_slot_differently() {
        let vars = root_secret_env();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("root-secret config loads");
            cfg.validate().expect("valid");
            assert!(cfg.api_key_pepper.len() >= 32);
            let keys = &cfg.keys;
            let slots: [&[u8; 32]; 5] = [
                &keys.identity_index,
                &keys.control_plane_session,
                &keys.oidc_state,
                &keys.oidc_nonce,
                &keys.csrf,
            ];
            for (i, slot) in slots.iter().enumerate() {
                for sibling in &slots[i + 1..] {
                    assert_ne!(slot, sibling, "secret slots must be purpose-separated");
                }
            }
        });
    }

    /// A supplied key wins over the root derivation for its slot alone.
    #[test]
    fn a_supplied_secret_wins_over_the_root_secret() {
        let mut vars = root_secret_env();
        vars.push(("MEMORY_MCP_HTTP_SESSION_KEY", "0".repeat(64)));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("config loads");
            assert_eq!(cfg.keys.control_plane_session, [0; 32]);
            assert_ne!(cfg.keys.identity_index, [0; 32]);
            assert_ne!(cfg.keys.csrf, [0; 32]);
        });
    }

    /// The root secret is a derivation source, never a license to invent
    /// material: without either form the old `ConfigMissing` contract stands.
    #[test]
    fn missing_secrets_without_a_root_secret_still_fail() {
        let vars: Vec<(&'static str, String)> = minimal_oidc_env()
            .into_iter()
            .filter(|(k, _)| *k != "MEMORY_MCP_API_KEY_PEPPER")
            .collect();
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            assert!(matches!(
                HttpConfig::from_env(),
                Err(MemoryError::ConfigMissing(name))
                    if name == "MEMORY_MCP_API_KEY_PEPPER"
            ));
        });
    }

    #[test]
    fn control_plane_ui_requires_control_plane() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane_ui = true;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(message)) if message.contains("UI requires control plane")
        ));
    }

    #[test]
    fn csv_allowlists_trim_entries() {
        let mut vars = base_required_env();
        vars[2] = ("ALLOWED_HOSTS", " localhost , 127.0.0.1 ".into());
        vars[3] = ("ALLOWED_ORIGINS", " http://localhost ".into());
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::from_env().expect("parses");
            assert_eq!(
                cfg.allowed_hosts,
                vec!["localhost".to_string(), "127.0.0.1".to_string()]
            );
            assert_eq!(cfg.allowed_origins, vec!["http://localhost".to_string()]);
        });
    }

    #[test]
    fn rejects_fs_watch_env_in_http_mode() {
        let mut vars = base_required_env();
        vars.push(("SURREALDB_FS_WATCH_INBOX", "/tmp/inbox".to_string()));
        let refs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        with_env(&refs, || {
            let cfg = HttpConfig::default_for_test();
            assert!(matches!(
                cfg.validate(),
                Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("SURREALDB_FS_WATCH_INBOX")
            ));
        });
    }

    #[test]
    fn rejects_open_signup_without_quotas() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.signup_mode = SignupMode::Open;
        assert!(matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(_))));
    }

    fn local_browser_config() -> LocalBrowserConfig {
        LocalBrowserConfig {
            session_key: [1u8; 32],
            csrf_key: [2u8; 32],
            default_plan_version: 1,
            default_plan_limits: PlanLimits::default(),
        }
    }

    /// The seven plan limit variables the `local` method publishes for the
    /// clients it provisions.
    fn plan_limit_env() -> Vec<(&'static str, String)> {
        vec![
            ("MEMORY_MCP_HTTP_MAX_INGESTED_BYTES", "1000".into()),
            ("MEMORY_MCP_HTTP_MAX_EPISODE_COUNT", "10".into()),
            ("MEMORY_MCP_HTTP_INGEST_PER_MINUTE", "3".into()),
            ("MEMORY_MCP_HTTP_MAX_OPEN_APP_SESSIONS", "8".into()),
            ("MEMORY_MCP_HTTP_MAX_ACTIVE_API_KEYS", "2".into()),
            ("MEMORY_MCP_HTTP_PER_TENANT_REQUEST_CONCURRENCY", "6".into()),
            ("MEMORY_MCP_HTTP_EXTRACTION_CONCURRENCY", "4".into()),
        ]
    }

    /// A valid, complete local-mode configuration.
    ///
    /// The positive case comes first so each negative test below mutates
    /// exactly one setting from a configuration already known to pass;
    /// otherwise a test can pass for the wrong reason.
    fn valid_local_config() -> HttpConfig {
        let mut cfg = HttpConfig::default_for_test();
        cfg.enable_control_plane = true;
        cfg.browser_auth = Some(BrowserAuthMethods {
            local: Some(local_browser_config()),
            oidc: None,
        });
        cfg.public_base_url = "https://memory.example.com".into();
        cfg.signup_plan_limits = Some(PlanLimits::default());
        cfg.signup_mode = SignupMode::InviteOnly;
        // A set without `oidc` must carry no OIDC-only setting at all.
        cfg.oidc_issuer.clear();
        cfg.oidc_client_id.clear();
        cfg.oidc_audience.clear();
        cfg.oidc_redirect_uri.clear();
        cfg.operator_identity_allowlist.clear();
        cfg
    }

    #[test]
    fn local_mode_valid_config_passes() {
        let cfg = valid_local_config();
        assert!(
            cfg.validate().is_ok(),
            "a complete local config must validate: {:?}",
            cfg.validate().err()
        );
        assert!(cfg.has_method(BrowserAuthMethod::Local));
        assert!(!cfg.has_method(BrowserAuthMethod::Oidc));
    }

    #[test]
    fn local_mode_rejects_each_oidc_only_setting() {
        // One mutation at a time, each asserting the precise variable name
        // so a failure points at the actual offender.
        for (name, mutate) in [
            (
                "MEMORY_MCP_HTTP_OIDC_ISSUER",
                Box::new(|cfg: &mut HttpConfig| {
                    cfg.oidc_issuer = "https://issuer.example.com".into()
                }) as Box<dyn Fn(&mut HttpConfig)>,
            ),
            (
                "MEMORY_MCP_HTTP_OIDC_CLIENT_ID",
                Box::new(|cfg: &mut HttpConfig| cfg.oidc_client_id = "client".into()),
            ),
            (
                "MEMORY_MCP_HTTP_OIDC_AUDIENCE",
                Box::new(|cfg: &mut HttpConfig| cfg.oidc_audience = "audience".into()),
            ),
            (
                "MEMORY_MCP_HTTP_OIDC_REDIRECT_URI",
                Box::new(|cfg: &mut HttpConfig| {
                    cfg.oidc_redirect_uri = "https://memory.example.com/callback".into()
                }),
            ),
        ] {
            let mut cfg = valid_local_config();
            mutate(&mut cfg);
            assert!(
                matches!(cfg.validate(), Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains(name)),
                "setting {name} must be rejected"
            );
        }
    }

    #[test]
    fn local_mode_rejects_operator_allowlist() {
        let mut cfg = valid_local_config();
        cfg.operator_identity_allowlist = vec!["someone@example.com".into()];
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("operator identity allowlist")
        ));
    }

    #[test]
    fn local_mode_rejects_open_signup() {
        let mut cfg = valid_local_config();
        cfg.signup_mode = SignupMode::Open;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("'oidc' authentication method")
        ));
    }

    #[test]
    fn local_mode_requires_explicit_plan_limits() {
        let mut cfg = valid_local_config();
        cfg.signup_plan_limits = None;
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("plan limits")
        ));
    }

    #[test]
    fn local_mode_requires_https() {
        let mut cfg = valid_local_config();
        cfg.public_base_url = "http://example.com".into();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("HTTPS")
        ));
    }

    #[test]
    fn local_mode_rejects_a_loopback_lookalike_host() {
        // A substring test for "localhost" used to accept these: the URL host
        // is not loopback, so plain HTTP would be served from a public name.
        for url in [
            "http://evil.example/?localhost",
            "http://localhost.evil.example",
            "http://notlocal.host/#localhost",
            "http://evil.example/localhost",
        ] {
            let mut cfg = valid_local_config();
            cfg.public_base_url = url.into();
            assert!(
                matches!(
                    cfg.validate(),
                    Err(MemoryError::ConfigInvalid(ref msg)) if msg.contains("HTTPS")
                ),
                "{url} must not satisfy the loopback exception"
            );
        }
    }

    #[test]
    fn local_mode_allows_localhost() {
        for url in [
            "http://localhost:8080",
            "http://localhost",
            "http://127.0.0.1:8080/deep/path",
            "http://[::1]:8080",
            "https://admin.example.com",
        ] {
            let mut cfg = valid_local_config();
            cfg.public_base_url = url.into();
            assert!(
                cfg.validate().is_ok(),
                "{url} must be an acceptable public base URL"
            );
        }
    }

    #[test]
    fn local_mode_does_not_require_oidc_completeness() {
        // The regression that blocked local deployments: the control plane
        // was enabled but the OIDC fields were empty, and validation
        // demanded an issuer nobody had configured.
        let cfg = valid_local_config();
        assert!(cfg.enable_control_plane);
        assert!(cfg.oidc_issuer.is_empty());
        assert!(cfg.oidc_client_id.is_empty());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn off_mode_rejects_oidc_credentials() {
        let mut cfg = HttpConfig::default_for_test();
        cfg.browser_auth = None;
        cfg.oidc_issuer = "https://issuer.example.com".into();
        assert!(matches!(
            cfg.validate(),
            Err(MemoryError::ConfigInvalid(msg)) if msg.contains("MEMORY_MCP_HTTP_OIDC_ISSUER")
        ));
    }
}
