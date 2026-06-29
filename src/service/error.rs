//! Error types for memory operations.

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
}

/// Returns `true` if the error is a transient database error that can be retried.
///
/// SurrealDB raises transaction conflicts when two concurrent write operations
/// affect the same record. These are safe to retry with exponential backoff.
#[must_use]
pub fn is_transient_db_error(err: &MemoryError) -> bool {
    match err {
        MemoryError::Storage(msg) => {
            msg.contains("Transaction conflict")
                || msg.contains("Resource busy")
                || msg.contains("would block")
        }
        _ => false,
    }
}

/// Shared validation error messages.
pub mod error_messages {
    pub const SCOPE_REQUIRED: &str = "scope is required";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_transient_db_error_matches_transaction_conflict() {
        let err = MemoryError::Storage("Transaction conflict: Resource busy".into());
        assert!(is_transient_db_error(&err));
    }

    #[test]
    fn is_transient_db_error_matches_resource_busy() {
        let err = MemoryError::Storage("Resource busy: table fact".into());
        assert!(is_transient_db_error(&err));
    }

    #[test]
    fn is_transient_db_error_matches_would_block() {
        let err = MemoryError::Storage("database would block".into());
        assert!(is_transient_db_error(&err));
    }

    #[test]
    fn is_transient_db_error_rejects_other_storage_errors() {
        let err = MemoryError::Storage("connection refused".into());
        assert!(!is_transient_db_error(&err));
    }

    #[test]
    fn is_transient_db_error_rejects_non_storage_errors() {
        let err = MemoryError::Validation("bad input".into());
        assert!(!is_transient_db_error(&err));
        let err = MemoryError::NotFound("missing".into());
        assert!(!is_transient_db_error(&err));
        let err = MemoryError::Transient("embedding timeout".into());
        assert!(!is_transient_db_error(&err));
        let err = MemoryError::ConfigMissing("SURREALDB_URL".into());
        assert!(!is_transient_db_error(&err));
    }
}
