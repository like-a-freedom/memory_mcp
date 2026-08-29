//! Principal resolution (ADR-0052, plan §4.3-4.6).
//!
//! Phase 4 introduces the request-scoped `AuthenticatedPrincipal`
//! and the parser/verifier pieces needed by the auth pipeline.
//! Tasks 4.4-4.7 add the cache, the account→tenant resolver, and
//! the auth middleware that turns a header into a principal.

pub mod api_keys;
