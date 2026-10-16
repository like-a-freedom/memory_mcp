//! Bounded provider scheduling; monotonic time is injectable at the owner.
use super::provider::{ProviderFailure, ProviderFailureKind};
use std::{
    future::Future,
    pin::Pin,
    time::{Duration, Instant},
};
pub(crate) trait OidcClock: Send + Sync {
    fn now(&self) -> Instant;
    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>>;
}
pub(crate) struct SystemOidcClock;
impl OidcClock for SystemOidcClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn sleep_until(&self, deadline: Instant) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(tokio::time::sleep_until(deadline.into()))
    }
}
pub(crate) fn retry_delay(attempt: u32, sample: u64, failure: &ProviderFailure) -> Duration {
    let retryable = matches!(
        failure.kind,
        ProviderFailureKind::Transport | ProviderFailureKind::Timeout
    ) || failure.status.is_some_and(|s| s == 429 || s >= 500);
    let ceiling = if retryable {
        (1u64 << attempt.min(6)).min(60)
    } else {
        60
    };
    let floor = (ceiling / 2).max(1);
    let seconds = if retryable {
        floor + sample % (ceiling - floor + 1)
    } else {
        60
    };
    let hint = if matches!(failure.status, Some(429 | 503)) {
        failure
            .retry_after
            .unwrap_or_default()
            .min(Duration::from_secs(300))
    } else {
        Duration::ZERO
    };
    Duration::from_secs(seconds).max(hint)
}
pub(crate) fn refresh_age(sample: u64) -> Duration {
    Duration::from_secs(240 + sample % 31)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn failure(status: u16) -> ProviderFailure {
        ProviderFailure {
            kind: ProviderFailureKind::Http,
            status: Some(status),
            retry_after: None,
        }
    }
    #[test]
    fn retry_502_uses_backoff() {
        assert_eq!(retry_delay(0, 0, &failure(502)), Duration::from_secs(1));
        assert_eq!(retry_delay(3, 0, &failure(502)), Duration::from_secs(4));
    }
    #[test]
    fn retry_exponent_saturates() {
        assert_eq!(
            retry_delay(u32::MAX, 0, &failure(502)),
            Duration::from_secs(30)
        );
    }
    #[test]
    fn retry_after_delta_is_capped() {
        let mut f = failure(503);
        f.retry_after = Some(Duration::from_secs(900));
        assert_eq!(retry_delay(0, 0, &f), Duration::from_secs(300));
    }
    #[test]
    fn invalid_metadata_retries_after_60_seconds() {
        assert_eq!(
            retry_delay(0, 0, &ProviderFailure::invalid()),
            Duration::from_secs(60)
        );
    }
    #[test]
    fn proactive_refresh_is_early() {
        assert_eq!(refresh_age(0), Duration::from_secs(240));
        assert_eq!(refresh_age(30), Duration::from_secs(270));
    }
}
