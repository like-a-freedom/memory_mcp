//! The error vocabulary every bounded context shares.
//!
//! These variants are pure: they name a condition, not a
//! mechanism. Nothing here reads the environment, opens a
//! connection or formats an HTTP response — `control/error.rs`
//! owns the HTTP mapping and stays in `http`.
//!
//! Database error *classification* deliberately does not live here.
//! `is_transient_db_error` inspects SurrealDB's wording, so it is a
//! persistence concern and moved to
//! [`crate::platform::persistence::db_errors`] with the retry
//! mechanism that consumes it.

/// Error type for memory operations.
#[derive(thiserror::Error, Debug, Clone)]
pub enum MemoryError {
    #[error("config missing: {0}")]
    ConfigMissing(String),

    #[error("config invalid: {0}")]
    ConfigInvalid(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("transient error: {0}")]
    Transient(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation error: {0}")]
    Validation(String),

    /// A capture identity conflict: the event exists with a different identity.
    #[error("conflict: {0}")]
    Conflict(String),

    /// The capture budget is exhausted before episode preparation.
    #[error("budget exhausted: {0}")]
    BudgetExhausted(String),

    /// The configured model is not available locally; restart after the
    /// background preparation completes. Distinct from
    /// [`MemoryError::Storage`] and [`MemoryError::Transient`] because the
    /// active extractor is immutable in this process and retries cannot
    /// change the outcome.
    #[error("model not ready: {0}")]
    ModelNotReady(String),

    /// Authentication or authorization failed (bad/unknown/revoked
    /// credential, expired key, suspended account). Never reveals
    /// which check failed to the caller.
    #[error("auth error: {0}")]
    Auth(String),

    /// The request cannot be served right now but may be later
    /// (provisioning in flight, schema incompatible, draining).
    /// Maps to HTTP 503 with retry guidance.
    #[error("unavailable: {0}")]
    Unavailable(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_and_unavailable_variants_display_stable_prefixes() {
        assert!(
            MemoryError::Auth("x".into())
                .to_string()
                .starts_with("auth error:")
        );
        assert!(
            MemoryError::Unavailable("x".into())
                .to_string()
                .starts_with("unavailable:")
        );
    }
}
