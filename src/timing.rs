//! Operation timing utilities for logging.

use std::time::Duration;

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
