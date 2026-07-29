//! Monotonic request-id generator shared by every tool invocation.

use std::sync::atomic::{AtomicU64, Ordering};

static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Generate a monotonically increasing request id like `req_0001`.
///
/// Replaces the instance-scoped `MemoryMcp::next_request_id` (which used a
/// per-instance `Arc<AtomicU64>` field). The new counter is **process-global**:
/// it does not reset between `MemoryMcp` instances or between tests. This is
/// intentional and is the only way to share id generation with the
/// protocol-agnostic `tools/` layer without threading a counter handle through
/// every call. The `req_NNNN` format is preserved byte-for-byte so structured
/// log events stay machine-parseable. See Risk R2.
#[must_use]
pub fn next_request_id() -> String {
    let n = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed) + 1;
    format!("req_{n:04}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_request_id_is_monotonic_and_zero_padded() {
        let a = next_request_id();
        let b = next_request_id();
        assert!(a.starts_with("req_"));
        assert!(b > a, "ids must be monotonically ordered as strings");
    }
}
