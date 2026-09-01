//! Shared sync helpers.
//!
//! The cache and rate-limiter use `std::sync::Mutex`; both
//! want the same poisoned-guard recovery. Keep it in one
//! place so the rationale ("recover instead of panic on a
//! poisoned lock") is documented once.

use std::sync::{Mutex, MutexGuard};

/// Recover the data behind a poisoned `Mutex` instead of
/// panicking. A poisoned guard still protects the data; the
/// only invariant the poison flag carries is that the
/// previous holder panicked while holding it.
pub fn recover_lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn recovers_after_panic() {
        let m = Arc::new(Mutex::new(42));
        let m_clone = m.clone();
        let _ = std::thread::spawn(move || {
            let _g = m_clone.lock().unwrap();
            panic!("poison");
        })
        .join();
        assert!(m.lock().is_err()); // poisoned
        let g = recover_lock(&m);
        assert_eq!(*g, 42);
    }
}
