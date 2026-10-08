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

impl MemoryError {
    /// The severity this error earns on a log record, per ADR-0079 §4.1.
    ///
    /// The error class decides, not the callsite. A failed unit of work or a
    /// risk to integrity — storage, a transient failure that survived its
    /// retries, a missing or invalid configuration, an unservable request — is
    /// an `ERROR` that needs attention. A handled refusal — not found, invalid
    /// input, a conflicting capture, an exhausted budget, a model still
    /// loading, a rejected credential — is a `WARN`: the caller was told, and
    /// nothing on our side is broken.
    #[must_use]
    pub fn log_level(&self) -> crate::logging::LogLevel {
        use crate::logging::LogLevel;
        match self {
            MemoryError::Storage(_)
            | MemoryError::Transient(_)
            | MemoryError::ConfigMissing(_)
            | MemoryError::ConfigInvalid(_)
            | MemoryError::Unavailable(_) => LogLevel::Error,
            MemoryError::NotFound(_)
            | MemoryError::Validation(_)
            | MemoryError::Conflict(_)
            | MemoryError::BudgetExhausted(_)
            | MemoryError::ModelNotReady(_)
            | MemoryError::Auth(_) => LogLevel::Warn,
        }
    }
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

    /// The level follows the error class, so a failure is never logged at the
    /// same severity as a refusal.
    #[test]
    fn a_failed_unit_of_work_is_an_error() {
        use crate::logging::LogLevel;
        for error in [
            MemoryError::Storage("x".into()),
            MemoryError::Transient("x".into()),
            MemoryError::ConfigMissing("x".into()),
            MemoryError::ConfigInvalid("x".into()),
            MemoryError::Unavailable("x".into()),
        ] {
            assert_eq!(error.log_level(), LogLevel::Error, "{error:?}");
        }
    }

    #[test]
    fn a_handled_refusal_is_a_warning() {
        use crate::logging::LogLevel;
        for error in [
            MemoryError::NotFound("x".into()),
            MemoryError::Validation("x".into()),
            MemoryError::Conflict("x".into()),
            MemoryError::BudgetExhausted("x".into()),
            MemoryError::ModelNotReady("x".into()),
            MemoryError::Auth("x".into()),
        ] {
            assert_eq!(error.log_level(), LogLevel::Warn, "{error:?}");
        }
    }
}
