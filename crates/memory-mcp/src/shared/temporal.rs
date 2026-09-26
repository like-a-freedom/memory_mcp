//! Temporal value semantics.
//!
//! Formatting, parsing and cache bucketing are pure: they read no
//! clock, no environment and no storage. Every context writes the
//! same instant in the same form and reads it back the same way, so
//! this is a shared *meaning* rather than a convenience that happens
//! to be used in several places.
//!
//! `now()` is included because it is the same vocabulary — a caller
//! that needs the current instant should not have to reach for a
//! storage module to get it.

use chrono::{DateTime, Timelike, Utc};

/// Normalize a datetime to RFC3339 string.
pub fn normalize_dt(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339()
}

/// Parse an ISO 8601 datetime string.
pub fn parse_iso(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Get current UTC time.
pub fn now() -> DateTime<Utc> {
    Utc::now()
}

/// Bucket cutoff to the start of the hour for better cache hit rate.
pub fn bucket_to_hour(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%dT%H:00:00Z").to_string()
}

/// Bucket cutoff to the nearest lower five-minute boundary for cache freshness.
pub fn bucket_to_five_minutes(dt: DateTime<Utc>) -> String {
    let minute = (dt.minute() / 5) * 5;
    format!("{}:{minute:02}:00Z", dt.format("%Y-%m-%dT%H"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Datelike, TimeZone};

    #[test]
    fn normalize_dt_formats_as_rfc3339() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        let result = normalize_dt(dt);
        assert!(result.starts_with("2024-01-15T10:30:00"));
    }

    #[test]
    fn parse_iso_parses_valid_datetime() {
        let result = parse_iso("2024-01-15T10:30:00Z");
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn parse_iso_returns_none_for_invalid_datetime() {
        assert!(parse_iso("invalid").is_none());
        assert!(parse_iso("").is_none());
        assert!(parse_iso("2024-13-45").is_none());
    }

    #[test]
    fn bucket_to_hour_rounds_down_to_hour() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 45, 30).unwrap();
        let result = bucket_to_hour(dt);
        assert_eq!(result, "2024-01-15T10:00:00Z");
    }

    #[test]
    fn bucket_to_five_minutes_rounds_down_to_five_minute_boundary() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 47, 30).unwrap();
        let result = bucket_to_five_minutes(dt);
        assert_eq!(result, "2024-01-15T10:45:00Z");
    }

    #[test]
    fn a_normalized_instant_round_trips() {
        let dt = Utc.with_ymd_and_hms(2024, 1, 15, 10, 30, 0).unwrap();
        let round_tripped = parse_iso(&normalize_dt(dt)).expect("round trip");
        assert_eq!(round_tripped.timestamp(), dt.timestamp());
    }
}
