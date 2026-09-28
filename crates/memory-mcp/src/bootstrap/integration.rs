//! Narrow adapters from published module interfaces to existing infrastructure.

#[cfg(feature = "control-plane")]
pub mod auth_method_policy;
#[cfg(feature = "control-plane")]
pub mod control_sessions;
pub mod durable_tasks;
pub mod provisioning;
pub mod provisioning_app_sessions;
pub mod registry_identity_links;
#[cfg(feature = "control-plane")]
pub mod registry_operations;
pub mod tenancy_resolution;
pub mod tenancy_runtime;
