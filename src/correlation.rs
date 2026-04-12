//! Correlation ID utilities for distributed tracing.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

/// A correlation ID for tracking related operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CorrelationId(u64);

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

impl CorrelationId {
    /// Creates a new unique correlation ID.
    #[must_use]
    pub fn new() -> Self {
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }

    /// Creates a correlation ID from a raw value.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// Returns the raw ID value.
    #[must_use]
    pub const fn as_raw(self) -> u64 {
        self.0
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "op-{:08x}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_id_unique() {
        let id1 = CorrelationId::new();
        let id2 = CorrelationId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn correlation_id_display_format() {
        let id = CorrelationId::from_raw(0x12345);
        assert_eq!(format!("{}", id), "op-00012345");
    }

    #[test]
    fn correlation_id_from_raw_roundtrip() {
        let id = CorrelationId::from_raw(42);
        assert_eq!(id.as_raw(), 42);
    }

    #[test]
    fn correlation_id_from_raw_zero() {
        let id = CorrelationId::from_raw(0);
        assert_eq!(id.as_raw(), 0);
        assert_eq!(format!("{}", id), "op-00000000");
    }

    #[test]
    fn correlation_id_from_raw_max() {
        let id = CorrelationId::from_raw(u64::MAX);
        assert_eq!(id.as_raw(), u64::MAX);
    }

    #[test]
    fn correlation_id_default_uses_new() {
        let id1 = CorrelationId::default();
        let id2 = CorrelationId::default();
        assert_ne!(id1, id2); // should be unique like new()
    }

    #[test]
    fn correlation_id_clone_preserves_value() {
        let id = CorrelationId::from_raw(123);
        let cloned = id;
        assert_eq!(id, cloned);
        assert_eq!(id.as_raw(), cloned.as_raw());
    }

    #[test]
    fn correlation_id_hash_and_eq() {
        use std::collections::HashSet;
        let id1 = CorrelationId::from_raw(1);
        let id2 = CorrelationId::from_raw(1);
        let id3 = CorrelationId::from_raw(2);

        let mut set = HashSet::new();
        set.insert(id1);
        assert!(set.contains(&id2));
        assert!(!set.contains(&id3));
    }
}
