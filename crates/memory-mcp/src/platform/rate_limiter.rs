//! Token-bucket rate limiter and mutex helpers.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// SafeMutex — handles poisoned locks gracefully.
// ---------------------------------------------------------------------------

pub trait SafeMutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> SafeMutex<T> for Mutex<T> {
    fn safe_lock(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

// ---------------------------------------------------------------------------
// RateLimiter — per-key token bucket.
// ---------------------------------------------------------------------------

pub(crate) struct RateLimiter {
    rps: f64,
    burst: f64,
    tokens: Mutex<HashMap<String, f64>>,
    last: Mutex<HashMap<String, Duration>>,
    clock: Box<dyn Fn() -> Duration + Send + Sync>,
}

impl RateLimiter {
    pub(crate) fn new(rps: i32, burst: i32) -> Self {
        let origin = Instant::now();
        Self::with_clock(rps, burst, move || origin.elapsed())
    }

    /// Bind a monotonic elapsed-time source at construction.
    ///
    /// Production uses `Instant`; controlled adapters can supply virtual time
    /// without changing the caller's admission interface or configuration.
    pub(crate) fn with_clock(
        rps: i32,
        burst: i32,
        clock: impl Fn() -> Duration + Send + Sync + 'static,
    ) -> Self {
        Self {
            rps: (rps.max(1)) as f64,
            burst: (burst.max(1)) as f64,
            tokens: Mutex::new(HashMap::new()),
            last: Mutex::new(HashMap::new()),
            clock: Box::new(clock),
        }
    }

    pub(crate) fn allow(&self, key: &str) -> bool {
        let mut tokens = self.tokens.safe_lock();
        let mut last = self.last.safe_lock();
        let now = (self.clock)();
        let last_time = last.entry(key.to_string()).or_insert(now);
        let elapsed = now.saturating_sub(*last_time).as_secs_f64();
        *last_time = now;
        let entry = tokens.entry(key.to_string()).or_insert(self.burst);
        let refill = elapsed * self.rps;
        *entry = (*entry + refill).min(self.burst);
        if *entry < 1.0 {
            return false;
        }
        *entry -= 1.0;
        true
    }

    /// Enforces the per-caller rate limit for an access payload.
    ///
    /// This is the single enforcement point for the token-bucket policy: it
    /// returns `Ok(())` when no caller is identified (no `access` or no
    /// `caller_id`) or the caller still has budget, and
    /// `Err(MemoryError::Validation)` once the caller's bucket is exhausted.
    /// `MemoryService::enforce_rate_limit` and `IngestionService` both
    /// delegate here so the policy lives in exactly one place.
    pub(crate) fn check_access(
        &self,
        access: Option<&crate::models::AccessPayload>,
    ) -> Result<(), crate::error::MemoryError> {
        if let Some(access) = access
            && let Some(caller) = &access.caller_id
            && !self.allow(caller)
        {
            return Err(crate::error::MemoryError::Validation(
                "rate limit exceeded".into(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod unit_tests {
    use super::*;
    use crate::models::AccessPayload;

    fn frozen_limiter(rps: i32, burst: i32) -> RateLimiter {
        RateLimiter::with_clock(rps, burst, || Duration::ZERO)
    }

    #[test]
    fn an_exhausted_bucket_refills_after_one_controlled_second() {
        let millis = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock = std::sync::Arc::clone(&millis);
        let limiter = RateLimiter::with_clock(1, 1, move || {
            std::time::Duration::from_millis(clock.load(std::sync::atomic::Ordering::SeqCst))
        });
        let _ = limiter.allow("caller");
        millis.store(1_000, std::sync::atomic::Ordering::SeqCst);

        assert!(limiter.allow("caller"));
    }

    #[test]
    fn a_partial_refill_does_not_admit_another_request() {
        let millis = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let clock = std::sync::Arc::clone(&millis);
        let limiter = RateLimiter::with_clock(1, 1, move || {
            std::time::Duration::from_millis(clock.load(std::sync::atomic::Ordering::SeqCst))
        });
        let _ = limiter.allow("caller");
        millis.store(500, std::sync::atomic::Ordering::SeqCst);

        assert!(!limiter.allow("caller"));
    }

    #[test]
    fn a_fresh_caller_can_use_its_first_token() {
        let limiter = frozen_limiter(10, 5);

        assert!(limiter.allow("caller"));
    }

    #[test]
    fn the_last_request_within_a_burst_is_admitted() {
        let limiter = frozen_limiter(10, 5);
        let _ = limiter.allow("caller");
        let _ = limiter.allow("caller");
        let _ = limiter.allow("caller");
        let _ = limiter.allow("caller");

        assert!(limiter.allow("caller"));
    }

    #[test]
    fn rate_limiter_enforces_limit_after_burst() {
        let limiter = frozen_limiter(10, 2);
        let _ = limiter.allow("user");
        let _ = limiter.allow("user");

        assert!(!limiter.allow("user"));
    }

    #[test]
    fn rate_limiter_is_per_key_isolated() {
        let limiter = frozen_limiter(1, 1);
        let _ = limiter.allow("user-a");

        assert!(limiter.allow("user-b"));
    }

    // --- check_access: the shared access-payload enforcement point ---

    #[test]
    fn check_access_allows_without_caller_id() {
        let limiter = frozen_limiter(50, 100);
        let access = AccessPayload::default();
        assert!(limiter.check_access(Some(&access)).is_ok());
    }

    #[test]
    fn check_access_allows_within_limit() {
        let limiter = frozen_limiter(50, 100);
        let access = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        assert!(limiter.check_access(Some(&access)).is_ok());
    }

    #[test]
    fn check_access_accepts_none() {
        let limiter = frozen_limiter(50, 100);
        assert!(limiter.check_access(None).is_ok());
    }

    #[test]
    fn check_access_with_burst_capacity() {
        let limiter = frozen_limiter(10, 2);
        let access = AccessPayload {
            caller_id: Some("burst-test".to_string()),
            ..Default::default()
        };
        limiter
            .check_access(Some(&access))
            .expect("arrange the first request");

        assert!(limiter.check_access(Some(&access)).is_ok());
    }

    #[test]
    fn check_access_multiple_users_isolated() {
        let limiter = frozen_limiter(10, 1);
        let user1 = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        let user2 = AccessPayload {
            caller_id: Some("user-2".to_string()),
            ..Default::default()
        };
        limiter
            .check_access(Some(&user1))
            .expect("arrange the first caller's consumed token");

        assert!(limiter.check_access(Some(&user2)).is_ok());
    }

    #[test]
    fn check_access_rejects_when_bucket_exhausted() {
        let limiter = frozen_limiter(1, 1);
        let access = AccessPayload {
            caller_id: Some("user-1".to_string()),
            ..Default::default()
        };
        limiter
            .check_access(Some(&access))
            .expect("arrange the consumed token");

        let err = limiter.check_access(Some(&access)).unwrap_err();
        assert!(matches!(
            err,
            crate::error::MemoryError::Validation(ref msg) if msg == "rate limit exceeded"
        ));
    }
}
