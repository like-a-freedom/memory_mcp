//! Shared durable-worker mechanics for lifecycle and claim projection.
//!
//! Shares cancellation, empty-poll backoff, transient-error backoff, and
/// logging across workers without coupling bounded-context schemas.
use std::time::Duration;

/// Default empty-poll interval (seconds).
pub const DEFAULT_EMPTY_POLL_SECS: u64 = 10;

/// Default transient-error backoff (seconds).
pub const DEFAULT_TRANSIENT_BACKOFF_SECS: u64 = 5;

/// Default lease duration (seconds).
pub const DEFAULT_LEASE_SECS: u64 = 120;

/// Default max attempts before dead-lettering.
pub const DEFAULT_MAX_ATTEMPTS: i64 = 5;

/// Returns the empty-poll backoff duration.
#[must_use]
pub fn empty_poll_backoff() -> Duration {
    Duration::from_secs(DEFAULT_EMPTY_POLL_SECS)
}

/// Returns the transient-error backoff duration.
#[must_use]
pub fn transient_error_backoff() -> Duration {
    Duration::from_secs(DEFAULT_TRANSIENT_BACKOFF_SECS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoffs_are_bounded_and_positive() {
        assert!(empty_poll_backoff().as_secs() > 0);
        assert!(transient_error_backoff().as_secs() > 0);
    }

    #[test]
    fn lease_secs_is_bounded() {
        const {
            assert!(DEFAULT_LEASE_SECS > 0);
        }
    }

    #[test]
    fn max_attempts_is_bounded() {
        let attempts = DEFAULT_MAX_ATTEMPTS;
        assert!(attempts > 0);
        assert!(attempts <= 10);
    }
}
