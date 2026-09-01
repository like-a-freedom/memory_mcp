//! Control plane.
//!
//! Adds the operator stub: an endpoint to create an
//! Account + reserved Tenant + enqueue a provisioning event.
//! OIDC-derived identity replaces the stub operator principal
//! and mounts the production routes. This
//! module is feature-gated on `control-plane` so a data-plane-only
//! HTTP build does not compile in operator endpoints.

pub mod account_api;
#[cfg(feature = "control-plane")]
pub mod csrf;
#[cfg(feature = "control-plane")]
pub mod deletion;
pub mod error;
#[cfg(feature = "control-plane")]
pub mod oidc;
pub mod operator;
#[cfg(feature = "control-plane")]
pub mod recent_auth;
#[cfg(feature = "control-plane")]
pub mod session;
#[cfg(feature = "control-plane")]
pub mod static_assets;
