//! Narrow adapters from published module interfaces to existing infrastructure.

#[cfg(feature = "control-plane")]
pub mod auth_method_policy;
#[cfg(feature = "control-plane")]
pub mod control_sessions;
pub mod durable_tasks;
pub mod legacy_registry_identity;
#[cfg(feature = "control-plane")]
pub mod legacy_registry_operations;
pub mod provisioning;
pub mod provisioning_app_sessions;
pub mod tenancy_resolution;
pub mod tenancy_runtime;
