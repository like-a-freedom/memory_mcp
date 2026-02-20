//! Structured logging utilities.
//!
//! This module provides a simple stdout logger with structured event formatting
//! and configurable log levels.

use std::collections::HashMap;
use std::io::{self, Write};

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

/// Logger that writes structured events to stderr.
///
/// Events are formatted as key-value pairs on a single line.
/// Long values are truncated to avoid excessive output.
///
/// # Examples
///
/// ```rust
/// use memory_mcp::logging::{StdoutLogger, LogLevel};
/// use std::collections::HashMap;
/// use serde_json::json;
///
/// let logger = StdoutLogger::new("info");
/// let mut event = HashMap::new();
/// event.insert("op".to_string(), json!("test"));
/// logger.log(event, LogLevel::Info);
/// ```
#[derive(Clone)]
pub struct StdoutLogger {
    level: LogLevel,
}

impl StdoutLogger {
    /// Creates a new logger with the specified minimum log level.
    #[must_use]
    pub fn new(level: &str) -> Self {
        Self {
            level: LogLevel::parse(level),
        }
    }

    /// Returns true if the provided `level` should be emitted given the
    /// currently configured minimum level.
    #[must_use]
    pub fn is_enabled(&self, level: LogLevel) -> bool {
        !(level < self.level)
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
        let mut parts = Vec::with_capacity(event.len() + 2);
        parts.push(format!("[{}] {}", ts, level.as_str().to_uppercase()));

        // Sort keys for deterministic output
        let mut keys: Vec<_> = event.keys().cloned().collect();
        keys.sort();

        for key in keys {
            if let Some(value) = event.get(&key) {
                let value_str = value_to_string(value);
                parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
            }
        }

        parts.join(" ")
    }
}

/// Converts a JSON value to a string representation.
///
/// Objects are flattened to key=value pairs, arrays to comma-separated lists.
/// Long values are truncated to MAX_LEN characters.
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
            "2026-01-01T00:00:00+00:00",
        );

        assert!(line.contains("[2026-01-01T00:00:00+00:00] INFO"));
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

        // Name should be quoted because it contains space
        assert!(line.contains("name=\"Dmitry Ivanov\""));
        // List should be flattened
        assert!(line.contains("list=[a,b,c]"));
        // Args object should be flattened
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

        // Value should be truncated with ellipsis
        assert!(line.contains("..."));

        // Verify truncated value length
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
}
