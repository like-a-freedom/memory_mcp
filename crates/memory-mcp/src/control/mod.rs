//! Control plane (ADR-0052, plan §4.7, §10).
//!
//! Phase 4 adds the operator stub: an endpoint to create an
//! Account + reserved Tenant + enqueue a provisioning event.
//! Phase 10 replaces the stub operator principal with
//! OIDC-derived identity and mounts the production routes. This
//! module is feature-gated on `control-plane` so a data-plane-only
//! HTTP build does not compile in operator endpoints.

pub mod account_api;
pub mod error;
#[cfg(feature = "control-plane")]
pub mod oidc;
pub mod operator;
#[cfg(feature = "control-plane")]
pub mod session;
