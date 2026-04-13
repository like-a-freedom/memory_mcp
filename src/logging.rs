//! Structured logging utilities.
//!
//! This module provides a simple stdout logger with structured event formatting
//! and configurable log levels.

use std::collections::HashMap;
use std::io::{self, Write};
use std::sync::Mutex;

use chrono::Utc;
use serde_json::Value;

/// Log level for filtering log output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl LogLevel {
    /// Parses a log level from a string.
    ///
    /// Case-insensitive. Defaults to `Info` for unknown values.
    #[must_use]
    pub fn parse(level: &str) -> Self {
        match level.trim().to_lowercase().as_str() {
            "trace" => Self::Trace,
            "debug" => Self::Debug,
            "warn" | "warning" => Self::Warn,
            "error" => Self::Error,
            _ => Self::Info,
        }
    }

    /// Returns the string representation of the log level.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Tracks repeated warning occurrences for deduplication.
#[derive(Default)]
struct WarnTracker {
    counts: Mutex<HashMap<String, u64>>,
}

/// Logger that writes structured events to stderr.
#[derive(Clone)]
pub struct StdoutLogger {
    level: LogLevel,
    warn_tracker: std::sync::Arc<WarnTracker>,
}

impl StdoutLogger {
    /// Creates a new logger with the specified minimum log level.
    #[must_use]
    pub fn new(level: &str) -> Self {
        Self {
            level: LogLevel::parse(level),
            warn_tracker: std::sync::Arc::new(WarnTracker::default()),
        }
    }

    /// Logs a warning with deduplication. The `dedup_key` identifies
    /// repeated occurrences. The first occurrence is always logged.
    /// Subsequent occurrences are logged only at every Nth repetition
    /// (controlled by `every_nth`, default 10).
    pub fn log_warn_dedup(
        &self,
        event: HashMap<String, Value>,
        dedup_key: &str,
        every_nth: u64,
    ) {
        let count = {
            let mut counts = self.warn_tracker.counts.lock().expect("warn tracker lock");
            let c = counts.entry(dedup_key.to_string()).or_insert(0);
            *c += 1;
            *c
        };

        if count == 1 || count % every_nth == 0 {
            let mut event = event;
            if count > 1 {
                event.insert(
                    "repeat_count".to_string(),
                    Value::Number(count.into()),
                );
            }
            self.log(event, LogLevel::Warn);
        }
    }

    /// Returns true if the provided `level` should be emitted given the
    /// currently configured minimum level.
    #[must_use]
    pub fn is_enabled(&self, level: LogLevel) -> bool {
        level >= self.level
    }

    /// Logs an event if the level is enabled.
    ///
    /// The logger respects the configured minimum `level`. Messages with a
    /// severity lower than the configured level are dropped. `debug` and
    /// `trace` messages are emitted only when the logger is configured to
    /// `debug`/`trace` respectively (no global unconditional suppression).
    pub fn log(&self, event: HashMap<String, Value>, level: LogLevel) {
        if level < self.level {
            return;
        }

        let line = Self::format_event_line(&event, level);

        let mut stderr = io::stderr();
        let _ = stderr.write_all(line.as_bytes());
        let _ = stderr.write_all(b"\n");
        let _ = stderr.flush();
    }

    /// Formats an event into a single human-readable line.
    #[must_use]
    pub fn format_event_line(event: &HashMap<String, Value>, level: LogLevel) -> String {
        let ts = Utc::now().to_rfc3339();
        Self::format_event_line_with_ts(event, level, &ts)
    }

    /// Formats an event with a provided timestamp.
    pub(crate) fn format_event_line_with_ts(
        event: &HashMap<String, Value>,
        level: LogLevel,
        ts: &str,
    ) -> String {
        // Truncate timestamp to milliseconds for readability: ...608.123456Z -> ...608Z
        let ts_short = if ts.len() > 23 {
            // RFC3339: "2026-04-12T20:03:59.608616+00:00" → find '.' then keep 3 digits then 'Z'
            if let Some(dot) = ts.find('.') {
                format!("{}Z", &ts[..dot + 4])
            } else {
                ts.to_string()
            }
        } else {
            ts.to_string()
        };

        // Extract special fields for prominent placement
        let request_id = event
            .get("request_id")
            .and_then(|v| v.as_str())
            .unwrap_or("-");
        let duration_ms = event.get("duration_ms").and_then(|v| v.as_u64());

        let mut parts = Vec::with_capacity(event.len() + 4);
        // Header: [ts] LEVEL  req=XXXX
        parts.push(format!("[{}] {:<5} req={:<6}", ts_short, level.as_str().to_uppercase(), request_id));

        // Build remaining keys, excluding special fields we already rendered
        let special_keys = ["request_id"];
        let mut keys: Vec<_> = event
            .keys()
            .filter(|k| !special_keys.contains(&k.as_str()))
            .cloned()
            .collect();
        keys.sort();

        // Render: op first, then duration_ms (if present), then the rest
        if let Some(pos) = keys.iter().position(|k| k == "op") {
            let op = keys.remove(pos);
            if let Some(value) = event.get(&op) {
                parts.push(format!("{}={}", op, value_to_string(value)));
            }
        }

        if let Some(ms) = duration_ms {
            parts.push(format!("duration_ms={}", ms));
        }

        for key in keys {
            if let Some(value) = event.get(&key) {
                let value_str = value_to_string(value);
                parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
            }
        }

        parts.join("  ")
    }
}

/// Converts a JSON value to a string representation.
///
/// Objects are flattened to key=value pairs, arrays to comma-separated lists.
/// Long values are truncated to MAX_LEN characters.
///
/// Special handling for Rust artifacts:
/// - `Some(value)` → extracts inner value
/// - `None` → "null"
/// - `Ok(value)` → extracts inner value
/// - `Err(value)` → formats as "Err(error_msg)"
fn value_to_string(value: &Value) -> String {
    const MAX_LEN: usize = 200;

    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(arr) => {
            let elems: Vec<String> = arr.iter().map(value_to_string).collect();
            format!("[{}]", elems.join(","))
        }
        Value::Object(map) => {
            // Handle Rust Option/Result artifacts from serde
            if let Some(Value::String(inner)) = map.get("Some") {
                return inner.clone();
            }
            if let Some(inner) = map.get("Some") {
                return value_to_string(inner);
            }
            if map.contains_key("None") {
                return "null".to_string();
            }
            if let Some(Value::String(inner)) = map.get("Ok") {
                return inner.clone();
            }
            if let Some(inner) = map.get("Ok") {
                return value_to_string(inner);
            }
            if let Some(Value::String(err)) = map.get("Err") {
                return format!("Err({})", err);
            }
            if let Some(inner) = map.get("Err") {
                return format!("Err({})", value_to_string(inner));
            }

            let mut pairs = Vec::with_capacity(map.len());
            let mut keys: Vec<_> = map.keys().cloned().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = map.get(&k) {
                    pairs.push(format!("{}={}", k, value_to_string(v)));
                }
            }
            format!("{{{}}}", pairs.join(","))
        }
    };

    if s.len() > MAX_LEN {
        format!("{}...", &s[..MAX_LEN - 3])
    } else {
        s
    }
}

/// Quotes a string if it contains special characters.
fn quote_if_needed(s: &str) -> String {
    if s.contains(char::is_whitespace) || s.contains('=') || s.contains('"') {
        format!("\"{}\"", s.replace('"', "'"))
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn log_level_parse_recognizes_valid_levels() {
        assert_eq!(LogLevel::parse("trace"), LogLevel::Trace);
        assert_eq!(LogLevel::parse("DEBUG"), LogLevel::Debug);
        assert_eq!(LogLevel::parse("info"), LogLevel::Info);
        assert_eq!(LogLevel::parse("WARN"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("warning"), LogLevel::Warn);
        assert_eq!(LogLevel::parse("error"), LogLevel::Error);
    }

    #[test]
    fn log_level_parse_defaults_to_info() {
        assert_eq!(LogLevel::parse("unknown"), LogLevel::Info);
        assert_eq!(LogLevel::parse(""), LogLevel::Info);
    }

    #[test]
    fn log_level_as_str() {
        assert_eq!(LogLevel::Trace.as_str(), "trace");
        assert_eq!(LogLevel::Debug.as_str(), "debug");
        assert_eq!(LogLevel::Info.as_str(), "info");
        assert_eq!(LogLevel::Warn.as_str(), "warn");
        assert_eq!(LogLevel::Error.as_str(), "error");
    }

    #[test]
    fn format_simple_event_contains_keys() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("migrations"));
        event.insert("stage".to_string(), json!("start"));
        event.insert("source".to_string(), json!("filesystem"));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00.000+00:00",
        );

        assert!(line.contains("[2026-01-01T00:00:00.000Z] INFO "));
        assert!(line.contains("req=-"));
        assert!(line.contains("op=migrations"));
        assert!(line.contains("stage=start"));
        assert!(line.contains("source=filesystem"));
    }

    #[test]
    fn format_object_and_array_and_quoting() {
        let mut event = HashMap::new();
        event.insert("name".to_string(), json!("Dmitry Ivanov"));
        event.insert("list".to_string(), json!(["a", "b", "c"]));
        event.insert("args".to_string(), json!({"scope": "org", "query": "ARR"}));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00+00:00",
        );

        assert!(line.contains("name=\"Dmitry Ivanov\""));
        assert!(line.contains("list=[a,b,c]"));
        assert!(line.contains("args="));
        assert!(line.contains("query=ARR"));
        assert!(line.contains("scope=org"));
    }

    #[test]
    fn format_truncates_long_values() {
        let long = "x".repeat(300);
        let mut event = HashMap::new();
        event.insert("long".to_string(), json!(long));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00+00:00",
        );

        assert!(line.contains("..."));

        if let Some(pos) = line.find("long=") {
            let rest = &line[pos + 5..];
            let value = rest.split_whitespace().next().unwrap_or("");
            assert_eq!(value.len(), 200);
        } else {
            panic!("missing long=");
        }
    }

    #[test]
    fn format_event_line_uses_current_timestamp() {
        let event = HashMap::new();
        let line = StdoutLogger::format_event_line(&event, LogLevel::Info);
        assert!(line.contains("] INFO"));
    }

    #[test]
    fn format_with_request_id_and_duration() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("extract.done"));
        event.insert("request_id".to_string(), json!("req_0042"));
        event.insert("duration_ms".to_string(), json!(152u64));
        event.insert("entities".to_string(), json!(3u64));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-04-12T20:03:59.608616+00:00",
        );

        assert!(line.contains("[2026-04-12T20:03:59.608Z]"));
        assert!(line.contains("INFO "));
        assert!(line.contains("req=req_0042"));
        assert!(line.contains("op=extract.done"));
        assert!(line.contains("duration_ms=152"));
        assert!(line.contains("entities=3"));
        // request_id should NOT appear again in the key-value section
        let after_op = line.split("op=extract.done").nth(1).unwrap_or("");
        assert!(!after_op.contains("request_id="));
    }

    #[test]
    fn format_without_request_id_shows_dash() {
        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("main.startup"));

        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-04-12T20:03:59.608616+00:00",
        );

        assert!(line.contains("req=-"));
    }

    #[test]
    fn is_enabled_respects_configured_level() {
        let info_logger = StdoutLogger::new("info");
        assert!(info_logger.is_enabled(LogLevel::Info));
        assert!(!info_logger.is_enabled(LogLevel::Debug));
        assert!(!info_logger.is_enabled(LogLevel::Trace));

        let debug_logger = StdoutLogger::new("debug");
        assert!(debug_logger.is_enabled(LogLevel::Debug));
        assert!(!debug_logger.is_enabled(LogLevel::Trace));
        assert!(debug_logger.is_enabled(LogLevel::Info));

        let trace_logger = StdoutLogger::new("trace");
        assert!(trace_logger.is_enabled(LogLevel::Trace));
        assert!(trace_logger.is_enabled(LogLevel::Debug));
        assert!(trace_logger.is_enabled(LogLevel::Info));
    }

    #[test]
    fn value_to_string_handles_option_some() {
        let some_value = json!({"Some": "hello"});
        assert_eq!(value_to_string(&some_value), "hello");

        // Nested SurrealDB-style Some with String wrapper
        let some_nested = json!({"Some": {"String": "world"}});
        // This extracts the inner object which is {String=world}
        assert_eq!(value_to_string(&some_nested), "{String=world}");
    }

    #[test]
    fn value_to_string_handles_option_none() {
        let none_value = json!({"None": null});
        assert_eq!(value_to_string(&none_value), "null");
    }

    #[test]
    fn value_to_string_handles_result_ok() {
        let ok_value = json!({"Ok": "success"});
        assert_eq!(value_to_string(&ok_value), "success");

        let ok_nested = json!({"Ok": {"value": 42}});
        assert_eq!(value_to_string(&ok_nested), "{value=42}");
    }

    #[test]
    fn value_to_string_handles_result_err() {
        let err_value = json!({"Err": "not found"});
        assert_eq!(value_to_string(&err_value), "Err(not found)");

        let err_nested = json!({"Err": {"code": 404}});
        assert_eq!(value_to_string(&err_nested), "Err({code=404})");
    }
}
