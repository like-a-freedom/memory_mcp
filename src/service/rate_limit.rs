//! Token-bucket rate limiter and mutex helpers.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Instant;

// ---------------------------------------------------------------------------
// SafeMutex — handles poisoned locks gracefully.
// ---------------------------------------------------------------------------

pub(crate) trait SafeMutex<T> {
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
    last: Mutex<HashMap<String, Instant>>,
}

impl RateLimiter {
    pub(crate) fn new(rps: i32, burst: i32) -> Self {
        Self {
            rps: (rps.max(1)) as f64,
            burst: (burst.max(1)) as f64,
            tokens: Mutex::new(HashMap::new()),
            last: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn allow(&self, key: &str) -> bool {
        let mut tokens = self.tokens.safe_lock();
        let mut last = self.last.safe_lock();
        let now = Instant::now();
        let last_time = last.entry(key.to_string()).or_insert(now);
        let elapsed = now.duration_since(*last_time).as_secs_f64();
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_limiter_new_initializes_correctly() {
        let limiter = RateLimiter::new(100, 50);
        let tokens = limiter.tokens.safe_lock();
        let last = limiter.last.safe_lock();
        assert!(tokens.is_empty());
        assert!(last.is_empty());
        drop(tokens);
        drop(last);
    }

    #[test]
    fn rate_limiter_burst_allows_initial_requests() {
        let limiter = RateLimiter::new(10, 5);
        for _ in 0..5 {
            assert!(limiter.allow("burst-user"));
        }
    }

    #[test]
    fn rate_limiter_enforces_limit_after_burst() {
        let limiter = RateLimiter::new(10, 2);
        assert!(limiter.allow("user"));
        assert!(limiter.allow("user"));
        assert!(!limiter.allow("user"));
    }

    #[test]
    fn rate_limiter_refills_over_time() {
        let limiter = RateLimiter::new(100, 1);
        assert!(limiter.allow("user"));
        assert!(!limiter.allow("user"));

        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(limiter.allow("user"));
    }

    #[test]
    fn rate_limiter_is_per_key_isolated() {
        let limiter = RateLimiter::new(1, 1);
        assert!(limiter.allow("user-a"));
        assert!(!limiter.allow("user-a"));
        assert!(limiter.allow("user-b"));
    }
}
