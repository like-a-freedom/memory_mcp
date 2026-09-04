//! Control plane.
//!
//! Adds the operator stub: an endpoint to create an
//! Account + reserved Tenant + enqueue a provisioning event.
//! OIDC-derived identity replaces the stub operator principal
//! and mounts the production routes. This
//! module is feature-gated on `control-plane` so a data-plane-only
//! HTTP build does not compile in operator endpoints.
//!
//! ## Application workflows
//!
//! The `application` submodule holds the business workflows
//! that the Axum handlers in `account_api` and `oidc` used
//! to inline. Splitting the workflow from the HTTP adapter
//! makes each piece testable in isolation: the workflow can
//! be exercised against an in-memory `RegistryStore` without
//! spinning up an Axum router, and the HTTP adapter can be
//! exercised with a fake workflow. Tasks 11 and 12 of the
//! architecture-audit-remediation plan land here.

pub mod account_api;
pub mod application;
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
