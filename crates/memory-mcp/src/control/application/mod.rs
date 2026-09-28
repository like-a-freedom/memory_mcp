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
//!   in-memory registry backend without an Axum router.
//! - The HTTP adapter is exercised with a fake workflow.
//!
//! Each workflow holds the owner traits it crosses — `OidcSignup` takes the
//! account and provisioning stores and nothing else — so a test wires two
//! fakes rather than a registry it would then have to implement in full. That
//! is the shape ADR-0054 argued for, and
//! [`crate::http::registry::RegistryStores`] now provides it.

pub mod oidc_signup;
