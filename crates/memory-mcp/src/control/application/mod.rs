//! Control-plane application workflows.
//!
//! Each submodule owns a business workflow that the Axum
//! adapters in `account_api` and `oidc` used to inline. The
//! split is the architecture-audit-remediation Task 11 / 12
//! deliverable: the HTTP adapter is responsible for
//! transport (parsing, headers, status codes, cookies,
//! redirects); the application workflow is responsible for
//! the business rules (account resolution, identity
//! uniqueness, atomic bundle creation, provisioning-event
//! append, secret generation).
//!
//! Both halves are now testable in isolation:
//!
//! - The application workflow is exercised against an
//!   in-memory `RegistryStore` without an Axum router.
//! - The HTTP adapter is exercised with a fake workflow.
//!
//! The workflows hold the omnibus `Arc<dyn RegistryStore>`
//! (the `RegistryStores` aggregator that would hold typed
//! `Arc<dyn Capability>` views is deferred; see
//! `docs/adr/0054-capability-specific-control-registry-interfaces.md`
//! and the plan's Task 10 status note).

pub mod api_keys;
pub mod oidc_signup;
