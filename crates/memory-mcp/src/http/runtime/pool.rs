//! Runtime pool. Phase 3 stub gate; real pool in Task 5.5.

use std::sync::atomic::{AtomicBool, Ordering};

/// Phase 3 stub admission gate: always open, never limits. Task 5.5
/// replaces the internals without changing these two method names.
pub struct AdmissionGate {
    closed: AtomicBool,
}

impl AdmissionGate {
    pub fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
        }
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
}

impl Default for AdmissionGate {
    fn default() -> Self {
        Self::new()
    }
}
