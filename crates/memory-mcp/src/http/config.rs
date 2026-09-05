//! HTTP configuration loaded exclusively from environment (12-factor).
//!
//! The implementation is split by concern across `config/`:
//!
//! - `parse` — `DEFAULT_*` constants, the `require_env` / `parse_*`
//!   helper family, the `TrustedCidr` parser, and the
//!   `deserialize_with` adapters for hex/duration fields.
//! - `types` — `HttpConfig`, `HmacKeys`, `SignupMode`, the env
//!   loader, and the test fixture.
//! - `validate` — every startup-time invariant enforced before the
//!   HTTP binary proceeds (allowlists, OIDC completeness, storage
//!   target separation, stdio-only env rejection, feature-gate
//!   consistency).
//!
//! This file is a thin façade: every public name is re-exported so
//! callers continue to use the `crate::http::config::X` paths.

mod parse;
mod types;
mod validate;

// Public re-exports. Every `DEFAULT_*` constant the crate exposes
// under `http::config::DEFAULT_*` is re-exported here so external
// embedders and the runtime helpers (`runtime::pool`,
// `runtime::storage`, `mcp::handlers`) all keep their existing
// import paths.
pub use crate::config::SurrealTargetConfig;
pub use parse::{
    DEFAULT_ALLOWED_HOSTS, DEFAULT_ALLOWED_ORIGINS, DEFAULT_BODY_LIMIT_BYTES,
    DEFAULT_GLOBAL_REQUEST_LIMIT, DEFAULT_MAINTENANCE_PARALLELISM, DEFAULT_OIDC_ALG,
    DEFAULT_POOL_CAP, DEFAULT_REQUEST_DEADLINE, DEFAULT_RUNTIME_ACTIVATION_TIMEOUT,
    DEFAULT_RUNTIME_CAPACITY_WAIT, DEFAULT_RUNTIME_IDLE_TTL, DEFAULT_SHUTDOWN_GRACE,
    DEFAULT_SUBSCRIPTION_AUTH_RECHECK, DEFAULT_SUBSCRIPTION_LIMIT,
    DEFAULT_SUBSCRIPTION_QUEUE_CAPACITY, DEFAULT_TASK_QUEUE_CAPACITY, DEFAULT_TASK_RETENTION_SECS,
    DEFAULT_TASK_SYNC_MAX_BYTES, TrustedCidr,
};
pub use types::{HmacKeys, HttpConfig, SignupMode};
