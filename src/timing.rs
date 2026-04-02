//! Operation timing utilities for logging.

use std::time::{Duration, Instant};

/// Tracks timing for an operation.
#[derive(Debug, Clone)]
pub struct OperationTimer {
    start: Instant,
    op: String,
}

impl OperationTimer {
    /// Creates a new timer for an operation.
    #[must_use]
    pub fn new(op: impl Into<String>) -> Self {
        Self {
            start: Instant::now(),
            op: op.into(),
        }
    }

    /// Returns elapsed time and continues timing.
    #[must_use]
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Returns elapsed time in milliseconds.
    #[must_use]
    pub fn elapsed_ms(&self) -> u128 {
        self.elapsed().as_millis()
    }

    /// Finishes timing and returns the operation name and duration.
    #[must_use]
    pub fn finish(self) -> (String, Duration) {
        (self.op, self.start.elapsed())
    }
}

/// Formats a duration in human-readable form for logging.
#[must_use]
pub fn format_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    let millis = duration.subsec_millis();

    if secs >= 1 {
        format!("{}.{:03}s", secs, millis)
    } else if millis >= 1 {
        format!("{}ms", millis)
    } else {
        format!("{}µs", duration.as_micros())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_timer_measures_elapsed() {
        let timer = OperationTimer::new("test_op");
        std::thread::sleep(Duration::from_millis(10));
        assert!(timer.elapsed_ms() >= 10);
    }

    #[test]
    fn operation_timer_finish_returns_duration() {
        let timer = OperationTimer::new("test_op");
        std::thread::sleep(Duration::from_millis(5));
        let (op, duration) = timer.finish();
        assert_eq!(op, "test_op");
        assert!(duration.as_millis() >= 5);
    }

    #[test]
    fn format_duration_micros() {
        let d = Duration::from_micros(500);
        let s = format_duration(d);
        assert!(s.contains("µs"));
    }

    #[test]
    fn format_duration_millis() {
        let d = Duration::from_millis(150);
        let s = format_duration(d);
        assert!(s.contains("ms"));
    }

    #[test]
    fn format_duration_seconds() {
        let d = Duration::from_millis(1500);
        let s = format_duration(d);
        assert!(s.contains('s'));
        assert!(s.starts_with("1."));
    }
}
