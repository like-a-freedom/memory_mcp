//! Polling policy for pages that follow an asynchronous backend state.
//!
//! Provisioning is asynchronous: a client row is created before its tenant can
//! serve data. The list page and the detail page both watch for that transition,
//! and both must back off the same way when a poll fails, so the interval and
//! the backoff live here rather than in whichever page happened to need them
//! first.

/// Interval between polls while a visible row is still provisioning.
pub const POLL_INTERVAL_MS: u32 = 2_000;

/// Ceiling for the poll backoff after a failed poll.
pub const POLL_MAX_BACKOFF_MS: u32 = 30_000;

/// The same interval expressed for operator-facing copy, so the sentence an
/// operator reads cannot drift from the interval actually used.
pub const POLL_INTERVAL_SECONDS: u32 = POLL_INTERVAL_MS / 1_000;

/// Next poll delay after a failed poll: double, capped at 30 seconds.
pub const fn next_backoff(current: u32) -> u32 {
    if current == 0 {
        POLL_INTERVAL_MS * 2
    } else if current >= POLL_MAX_BACKOFF_MS {
        POLL_MAX_BACKOFF_MS
    } else {
        let doubled = current * 2;
        if doubled > POLL_MAX_BACKOFF_MS {
            POLL_MAX_BACKOFF_MS
        } else {
            doubled
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{POLL_INTERVAL_MS, POLL_MAX_BACKOFF_MS, next_backoff};

    #[test]
    fn poll_backoff_doubles_and_caps_at_thirty_seconds() {
        assert_eq!(next_backoff(0), POLL_INTERVAL_MS * 2);
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 2), POLL_INTERVAL_MS * 4);
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 4), POLL_INTERVAL_MS * 8);
        // 4s, 8s, 16s, then the ceiling: 32s is clamped to 30s.
        assert_eq!(next_backoff(POLL_INTERVAL_MS * 8), POLL_MAX_BACKOFF_MS);
        assert_eq!(next_backoff(POLL_MAX_BACKOFF_MS), POLL_MAX_BACKOFF_MS);
        // Absurd input cannot overflow or exceed the ceiling.
        assert_eq!(next_backoff(u32::MAX), POLL_MAX_BACKOFF_MS);
    }
}
