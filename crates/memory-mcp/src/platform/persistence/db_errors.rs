//! Classification of database errors.
//!
//! This belongs in persistence rather than the shared kernel because
//! it matches on SurrealDB's own error wording. The kernel holds the
//! error *vocabulary*; deciding which storage failures are safe to
//! retry is a fact about the database, and only the retry mechanism
//! that consumes it needs to know it.

use crate::shared::error::MemoryError;

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

/// Returns `true` only when the storage error indicates that the underlying
/// client session is no longer usable. Query errors and transaction conflicts
/// remain attached to the current binding so they do not cause needless
/// reconnects across every tenant using it.
#[must_use]
pub fn is_db_session_failure(err: &MemoryError) -> bool {
    let MemoryError::Storage(message) = err else {
        return false;
    };
    let message = message.to_ascii_lowercase();
    if message.contains("transaction conflict")
        || message.contains("resource busy")
        || message.contains("would block")
    {
        return false;
    }
    [
        "connection lost",
        "connection reset",
        "connection closed",
        "connection refused",
        "websocket closed",
        "websocket connection closed",
        "websocket disconnected",
        "broken pipe",
        "not connected",
        "disconnected from server",
        "session closed",
    ]
    .iter()
    .any(|signature| message.contains(signature))
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
        for err in [
            MemoryError::Validation("bad input".into()),
            MemoryError::NotFound("missing".into()),
            MemoryError::Transient("embedding timeout".into()),
            MemoryError::ConfigMissing("SURREALDB_URL".into()),
            MemoryError::ModelNotReady("classic gliner".into()),
            MemoryError::Auth("bad credential".into()),
            MemoryError::Unavailable("draining".into()),
        ] {
            assert!(!is_transient_db_error(&err), "{err} is not a storage error");
        }
    }

    #[test]
    fn is_db_session_failure_matches_broken_connections() {
        for message in [
            "connection lost",
            "connection reset by peer",
            "WebSocket connection closed",
            "broken pipe while sending query",
            "not connected to server",
        ] {
            assert!(
                is_db_session_failure(&MemoryError::Storage(message.into())),
                "{message} should retire its pooled session"
            );
        }
    }

    #[test]
    fn is_db_session_failure_preserves_query_and_retry_errors() {
        for message in [
            "Transaction conflict: Resource busy",
            "Resource busy: table fact",
            "syntax error in query",
            "permission denied for table fact",
            "connection string is malformed",
        ] {
            assert!(
                !is_db_session_failure(&MemoryError::Storage(message.into())),
                "{message} must not retire a pooled session"
            );
        }
        assert!(!is_db_session_failure(&MemoryError::Auth(
            "connection lost".into()
        )));
    }
}
