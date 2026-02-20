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
}
