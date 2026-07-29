//! Shared durable-worker mechanics for lifecycle and claim projection.
//!
//! Shares cancellation, empty-poll backoff, transient-error backoff, and
/// logging across workers without coupling bounded-context schemas.
use std::time::Duration;

/// Default empty-poll interval (seconds).
#[allow(dead_code)]
pub const DEFAULT_EMPTY_POLL_SECS: u64 = 10;

/// Default transient-error backoff (seconds).
#[allow(dead_code)]
pub const DEFAULT_TRANSIENT_BACKOFF_SECS: u64 = 5;

/// Default lease duration (seconds).
#[allow(dead_code)]
pub const DEFAULT_LEASE_SECS: u64 = 120;

/// Default max attempts before dead-lettering.
#[allow(dead_code)]
pub const DEFAULT_MAX_ATTEMPTS: i64 = 5;

/// Returns the empty-poll backoff duration.
#[must_use]
#[allow(dead_code)]
pub fn empty_poll_backoff() -> Duration {
    Duration::from_secs(DEFAULT_EMPTY_POLL_SECS)
}

/// Returns the transient-error backoff duration.
#[must_use]
#[allow(dead_code)]
pub fn transient_error_backoff() -> Duration {
    Duration::from_secs(DEFAULT_TRANSIENT_BACKOFF_SECS)
}

/// Returns the lease duration.
#[must_use]
#[allow(dead_code)]
pub fn lease_duration() -> Duration {
    Duration::from_secs(DEFAULT_LEASE_SECS)
}

/// Returns `true` if the error is transient and should be retried.
#[must_use]
#[allow(dead_code)]
pub fn is_transient(err: &crate::service::MemoryError) -> bool {
    crate::service::is_transient_db_error(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoffs_are_bounded_and_positive() {
        assert!(empty_poll_backoff().as_secs() > 0);
        assert!(transient_error_backoff().as_secs() > 0);
        assert!(lease_duration().as_secs() > 0);
    }

    #[test]
    fn max_attempts_is_bounded() {
        let attempts = DEFAULT_MAX_ATTEMPTS;
        assert!(attempts > 0);
        assert!(attempts <= 10);
    }
}
