//! Error types for memory operations.

/// Error type for memory operations.
#[derive(thiserror::Error, Debug)]
pub enum MemoryError {
    #[error("config missing: {0}")]
    ConfigMissing(String),

    #[error("config invalid: {0}")]
    ConfigInvalid(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("validation error: {0}")]
    Validation(String),

    #[error("session not found: {0}")]
    SessionNotFound(String),

    #[error("session expired: {0}")]
    SessionExpired(String),

    #[error("session limit exceeded")]
    SessionLimitExceeded,

    #[error("draft expired: {0}")]
    DraftExpired(String),

    #[error("confirmation required")]
    ConfirmationRequired,

    #[error("app error: {0}")]
    App(String),

    #[error("invalid parameter: {0}")]
    InvalidParameter(String),
}
