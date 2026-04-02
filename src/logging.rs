//! Structured logging utilities.
//!
//! This module provides a simple stdout logger with structured event formatting
//! and configurable log levels.

use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;

use chrono::Utc;
use serde_json::Value;

use crate::correlation::CorrelationId;

/// Macro for building operation log events with fixed field order.
///
/// # Example
///
/// ```rust
/// use memory_mcp::operation_event;
/// use memory_mcp::timing::OperationTimer;
///
/// let timer = OperationTimer::new("embed");
/// let event = operation_event!(
///     "embed",
///     timer,
///     "success",
///     None::<String>,
///     "provider" => "ollama",
///     "count" => 5
/// );
/// assert_eq!(
///     event.get("op").and_then(serde_json::Value::as_str),
///     Some("embed")
/// );
/// ```
#[macro_export]
macro_rules! operation_event {
    ($op:expr, $timer:expr, $status:expr, $error:expr, $($key:expr => $value:expr),* $(,)?) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("duration_ms".to_string(), serde_json::json!($timer.elapsed_ms()));
        event.insert("status".to_string(), serde_json::json!($status));
        if let Some(err) = $error {
            event.insert("error".to_string(), serde_json::json!(err));
        }
        // Add custom fields
        $(event.insert($key.to_string(), serde_json::json!($value));)*
        event
    }};
}

/// Macro for building simple log events without timer.
/// Automatically adds timestamp.
///
/// # Example
///
/// ```rust
/// use memory_mcp::log_event;
///
/// let event = log_event!(
///     "ingest.start",
///     "success",
///     "source_id" => "email-123",
///     "scope" => "org"
/// );
/// assert!(event.contains_key("op"));
/// assert!(event.contains_key("status"));
/// ```
#[macro_export]
macro_rules! log_event {
    ($op:expr, $status:expr $(, $($key:expr => $value:expr),*)?) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("status".to_string(), serde_json::json!($status));
        event.insert("ts".to_string(), serde_json::json!(chrono::Utc::now().to_rfc3339()));
        $(
            $(event.insert($key.to_string(), serde_json::json!($value));)*
        )?
        event
    }};
}

/// Macro for building error log events with context.
///
/// # Example
///
/// ```rust
/// use memory_mcp::log_error;
///
/// let err = std::io::Error::new(std::io::ErrorKind::NotFound, "not found");
/// let event = log_error!(
///     "db.select_one",
///     &err,
///     "record_id" => "episode:123",
///     "namespace" => "org"
/// );
/// assert!(event.contains_key("op"));
/// assert!(event.contains_key("error"));
/// ```
#[macro_export]
macro_rules! log_error {
    ($op:expr, $err:expr $(, $($key:expr => $value:expr),*)?) => {{
        let mut event = std::collections::HashMap::new();
        event.insert("op".to_string(), serde_json::json!($op));
        event.insert("status".to_string(), serde_json::json!("error"));
        event.insert("error".to_string(), serde_json::json!($err.to_string()));
        event.insert("ts".to_string(), serde_json::json!(chrono::Utc::now().to_rfc3339()));
        $(
            $(event.insert($key.to_string(), serde_json::json!($value));)*
        )?
        event
    }};
}

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

/// Fixed field order for consistent log output.
/// Order: op, status, duration_ms, provider, count, error (then alphabetically sorted rest)
pub(crate) const LOG_FIELD_ORDER: &[&str] =
    &["op", "status", "duration_ms", "provider", "count", "error"];

/// Context for log events, used to enrich logs with correlation and session information.
#[derive(Debug, Clone, Default)]
pub struct LogContext {
    /// Correlation ID for request tracing
    pub correlation_id: Option<CorrelationId>,
    /// Session ID if available
    pub session_id: Option<String>,
    /// User ID if available
    pub user_id: Option<String>,
    /// Tool/operation name
    pub tool_name: Option<String>,
}

impl LogContext {
    /// Creates a new empty log context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Creates a log context with correlation ID.
    #[must_use]
    pub fn with_correlation_id(correlation_id: CorrelationId) -> Self {
        Self {
            correlation_id: Some(correlation_id),
            ..Default::default()
        }
    }

    /// Creates a log context with session ID.
    #[must_use]
    pub fn with_session_id(session_id: String) -> Self {
        Self {
            session_id: Some(session_id),
            ..Default::default()
        }
    }

    /// Adds session ID to the context (for chaining).
    #[must_use]
    pub fn with_session_id_opt(mut self, session_id: Option<String>) -> Self {
        if session_id.is_some() {
            self.session_id = session_id;
        }
        self
    }

    /// Adds tool name to the context (for chaining).
    #[must_use]
    pub fn with_tool_name_opt(mut self, tool_name: Option<String>) -> Self {
        if tool_name.is_some() {
            self.tool_name = tool_name;
        }
        self
    }

    /// Adds correlation ID to the context (for chaining).
    #[must_use]
    pub fn with_correlation_id_opt(mut self, correlation_id: Option<CorrelationId>) -> Self {
        if correlation_id.is_some() {
            self.correlation_id = correlation_id;
        }
        self
    }

    /// Adds tool name to the context.
    #[must_use]
    pub fn with_tool_name(mut self, tool_name: String) -> Self {
        self.tool_name = Some(tool_name);
        self
    }

    /// Converts context to a HashMap for logging.
    #[must_use]
    pub fn to_event_fields(&self) -> HashMap<String, Value> {
        let mut fields = HashMap::new();
        if let Some(cid) = self.correlation_id {
            fields.insert("correlation_id".to_string(), Value::String(cid.to_string()));
        }
        if let Some(sid) = &self.session_id {
            fields.insert("session_id".to_string(), Value::String(sid.clone()));
        }
        if let Some(uid) = &self.user_id {
            fields.insert("user_id".to_string(), Value::String(uid.clone()));
        }
        if let Some(tool) = &self.tool_name {
            fields.insert("tool".to_string(), Value::String(tool.clone()));
        }
        fields
    }
}

/// Returns appropriate log level based on operation duration.
/// For successful operations only - errors always use Error level.
pub fn level_for_duration(duration_ms: u128) -> LogLevel {
    match duration_ms {
        0..=99 => LogLevel::Debug,
        100..=999 => LogLevel::Info,
        _ => LogLevel::Warn,
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
/// use memory_mcp::logging::{StdoutLogger, LogLevel, LogContext};
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
    context: LogContext,
}

impl StdoutLogger {
    /// Creates a new logger with the specified minimum log level.
    #[must_use]
    pub fn new(level: &str) -> Self {
        Self {
            level: LogLevel::parse(level),
            context: LogContext::default(),
        }
    }

    /// Creates a new logger with a correlation ID for tracing.
    #[must_use]
    pub fn with_correlation(level: &str, correlation_id: CorrelationId) -> Self {
        Self {
            level: LogLevel::parse(level),
            context: LogContext::with_correlation_id(correlation_id),
        }
    }

    /// Creates a new logger with a log context.
    #[must_use]
    pub fn with_context(level: &str, context: LogContext) -> Self {
        Self {
            level: LogLevel::parse(level),
            context,
        }
    }

    /// Returns a new logger with the specified correlation ID.
    #[must_use]
    pub fn with_correlation_id(&self, correlation_id: CorrelationId) -> Self {
        Self {
            level: self.level,
            context: LogContext::with_correlation_id(correlation_id)
                .with_session_id_opt(self.context.session_id.clone())
                .with_tool_name_opt(self.context.tool_name.clone()),
        }
    }

    /// Returns a new logger with the specified session ID.
    #[must_use]
    pub fn with_session_id(&self, session_id: String) -> Self {
        Self {
            level: self.level,
            context: LogContext::with_session_id(session_id)
                .with_correlation_id_opt(self.context.correlation_id)
                .with_tool_name_opt(self.context.tool_name.clone()),
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
    /// `debug`/trace` respectively (no global unconditional suppression).
    pub fn log(&self, event: HashMap<String, Value>, level: LogLevel) {
        if level < self.level {
            return;
        }

        let line = self.format_event_line_with_context(&event, level);

        let mut stderr = io::stderr();
        let _ = stderr.write_all(line.as_bytes());
        let _ = stderr.write_all(b"\n");
        let _ = stderr.flush();
    }

    /// Logs an event with additional context fields.
    pub fn log_with_context(
        &self,
        event: HashMap<String, Value>,
        level: LogLevel,
        context: &LogContext,
    ) {
        if level < self.level {
            return;
        }

        let mut enriched_event = context.to_event_fields();
        for (k, v) in event {
            enriched_event.insert(k, v);
        }

        let line = self.format_event_line_with_context(&enriched_event, level);

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

    /// Formats an event with context (correlation ID, session ID, etc.).
    #[must_use]
    fn format_event_line_with_context(
        &self,
        event: &HashMap<String, Value>,
        level: LogLevel,
    ) -> String {
        let ts = Utc::now().to_rfc3339();
        let mut parts = Vec::with_capacity(event.len() + 3);

        // Build correlation/session prefix
        let mut prefix_parts = Vec::new();
        if let Some(cid) = self.context.correlation_id {
            prefix_parts.push(cid.to_string());
        }
        if let Some(sid) = &self.context.session_id {
            prefix_parts.push(format!("session={}", sid));
        }

        if prefix_parts.is_empty() {
            parts.push(format!("[{}] {}", ts, level.as_str().to_uppercase()));
        } else {
            parts.push(format!(
                "[{}] {} {}",
                ts,
                level.as_str().to_uppercase(),
                prefix_parts.join(" ")
            ));
        }

        let mut keys: Vec<_> = event.keys().cloned().collect();
        keys.sort();

        // First emit fields in fixed order
        for key in LOG_FIELD_ORDER {
            if let Some(value) = event.get(*key) {
                let value_str = value_to_string(value);
                parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
            }
        }

        // Then emit remaining fields alphabetically
        let remaining: Vec<_> = keys
            .into_iter()
            .filter(|k| !LOG_FIELD_ORDER.contains(&k.as_str()))
            .collect();

        for key in remaining {
            if let Some(value) = event.get(&key) {
                let value_str = value_to_string(value);
                parts.push(format!("{}={}", key, quote_if_needed(&value_str)));
            }
        }

        parts.join(" ")
    }

    /// Formats an event with a provided timestamp (static method for tests).
    pub(crate) fn format_event_line_with_ts(
        event: &HashMap<String, Value>,
        level: LogLevel,
        ts: &str,
    ) -> String {
        let mut parts = Vec::with_capacity(event.len() + 2);
        parts.push(format!("[{}] {}", ts, level.as_str().to_uppercase()));

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

    /// Formats a duration in human-readable form.
    #[must_use]
    pub fn format_duration(duration: Duration) -> String {
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();

        if secs >= 1 {
            format!("{}.{:03}s", secs, millis)
        } else {
            format!("{}ms", millis)
        }
    }
}

/// Converts a JSON value to a string representation.
///
/// Objects are flattened to key=value pairs, arrays to comma-separated lists.
/// Long values are truncated to MAX_LEN characters.
/// Large collections are summarized to avoid excessive output.
///
/// Special handling for Rust artifacts:
/// - `Some(value)` → extracts inner value
/// - `None` → "null"
/// - `Ok(value)` → extracts inner value
/// - `Err(value)` → formats as "Err(error_msg)"
fn value_to_string(value: &Value) -> String {
    const MAX_LEN: usize = 200;
    const MAX_ARRAY_ITEMS: usize = 5;
    const MAX_OBJECT_KEYS: usize = 3;

    let s = match value {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Null => "null".to_string(),
        Value::Array(arr) => {
            if arr.len() > MAX_ARRAY_ITEMS {
                // Summarize large arrays
                let sample: Vec<String> = arr
                    .iter()
                    .take(MAX_ARRAY_ITEMS)
                    .map(value_to_string)
                    .collect();
                return format!(
                    "[{}... +{} more]",
                    sample.join(","),
                    arr.len() - MAX_ARRAY_ITEMS
                );
            }
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

            if map.len() > MAX_OBJECT_KEYS {
                // Summarize large objects
                let mut keys: Vec<_> = map.keys().cloned().collect();
                keys.sort();
                let sample: Vec<String> = keys
                    .iter()
                    .take(MAX_OBJECT_KEYS)
                    .filter_map(|k| map.get(k).map(|v| format!("{}={}", k, value_to_string(v))))
                    .collect();
                return format!(
                    "{{{}}}, +{} more",
                    sample.join(","),
                    map.len() - MAX_OBJECT_KEYS
                );
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

    #[test]
    fn level_for_duration_debug_for_fast_operations() {
        assert_eq!(level_for_duration(50), LogLevel::Debug);
        assert_eq!(level_for_duration(99), LogLevel::Debug);
    }

    #[test]
    fn level_for_duration_info_for_normal_operations() {
        assert_eq!(level_for_duration(100), LogLevel::Info);
        assert_eq!(level_for_duration(500), LogLevel::Info);
        assert_eq!(level_for_duration(999), LogLevel::Info);
    }

    #[test]
    fn level_for_duration_warn_for_slow_operations() {
        assert_eq!(level_for_duration(1000), LogLevel::Warn);
        assert_eq!(level_for_duration(5000), LogLevel::Warn);
    }

    #[test]
    fn log_context_default_is_empty() {
        let ctx = LogContext::new();
        assert!(ctx.correlation_id.is_none());
        assert!(ctx.session_id.is_none());
        assert!(ctx.user_id.is_none());
        assert!(ctx.tool_name.is_none());
    }

    #[test]
    fn log_context_with_correlation_id() {
        use crate::correlation::CorrelationId;
        let cid = CorrelationId::new();
        let ctx = LogContext::with_correlation_id(cid);
        assert_eq!(ctx.correlation_id, Some(cid));
        assert!(ctx.session_id.is_none());
    }

    #[test]
    fn log_context_with_session_id() {
        let ctx = LogContext::with_session_id("test-session-123".to_string());
        assert_eq!(ctx.session_id, Some("test-session-123".to_string()));
        assert!(ctx.correlation_id.is_none());
    }

    #[test]
    fn log_context_chaining() {
        use crate::correlation::CorrelationId;
        let cid = CorrelationId::new();
        let ctx = LogContext::with_correlation_id(cid)
            .with_session_id_opt(Some("session-456".to_string()))
            .with_tool_name_opt(Some("test_tool".to_string()));

        assert_eq!(ctx.correlation_id, Some(cid));
        assert_eq!(ctx.session_id, Some("session-456".to_string()));
        assert_eq!(ctx.tool_name, Some("test_tool".to_string()));
    }

    #[test]
    fn log_context_to_event_fields() {
        use crate::correlation::CorrelationId;
        let cid = CorrelationId::new();
        let ctx = LogContext::with_correlation_id(cid)
            .with_session_id_opt(Some("session-789".to_string()));

        let fields = ctx.to_event_fields();
        assert!(fields.contains_key("correlation_id"));
        assert!(fields.contains_key("session_id"));
        assert_eq!(
            fields.get("session_id").unwrap().as_str(),
            Some("session-789")
        );
    }

    #[test]
    fn log_simple_macro_creates_event() {
        let event = log_event!("test_op", "success", "key1" => "value1", "key2" => 42);
        assert_eq!(
            event.get("op").and_then(|v: &serde_json::Value| v.as_str()),
            Some("test_op")
        );
        assert_eq!(
            event
                .get("status")
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("success")
        );
        assert!(event.contains_key("ts"));
        assert_eq!(
            event
                .get("key1")
                .and_then(|v: &serde_json::Value| v.as_str()),
            Some("value1")
        );
        assert_eq!(
            event
                .get("key2")
                .and_then(|v: &serde_json::Value| v.as_i64()),
            Some(42)
        );
    }

    #[test]
    fn log_error_macro_creates_event() {
        let err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let event = log_error!("db.query", &err, "table" => "facts");
        assert_eq!(event.get("op").and_then(|v| v.as_str()), Some("db.query"));
        assert_eq!(event.get("status").and_then(|v| v.as_str()), Some("error"));
        assert!(
            event
                .get("error")
                .unwrap()
                .as_str()
                .unwrap()
                .contains("file not found")
        );
        assert!(event.contains_key("ts"));
        assert_eq!(event.get("table").and_then(|v| v.as_str()), Some("facts"));
    }

    #[test]
    fn value_to_string_truncates_large_arrays() {
        let arr = json!([1, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
        let result = value_to_string(&arr);
        assert!(result.contains("+5 more"));
        assert!(result.starts_with("["));
    }

    #[test]
    fn value_to_string_truncates_large_objects() {
        let obj = json!({"a": 1, "b": 2, "c": 3, "d": 4, "e": 5});
        let result = value_to_string(&obj);
        assert!(result.contains("+2 more"));
        assert!(result.starts_with("{"));
    }

    #[test]
    fn value_to_string_small_collections_not_truncated() {
        let arr = json!([1, 2, 3]);
        let result = value_to_string(&arr);
        assert!(!result.contains("more"));
        assert_eq!(result, "[1,2,3]");

        let obj = json!({"a": 1, "b": 2});
        let result2 = value_to_string(&obj);
        assert!(!result2.contains("more"));
        assert_eq!(result2, "{a=1,b=2}");
    }

    #[test]
    fn stdout_logger_with_session_id() {
        let logger = StdoutLogger::new("debug").with_session_id("test-session".to_string());

        let mut event = HashMap::new();
        event.insert("op".to_string(), json!("test"));

        // Just verify it doesn't panic
        logger.log(event, LogLevel::Debug);
    }

    #[test]
    fn stdout_logger_with_context() {
        let ctx = LogContext::with_session_id("ctx-session".to_string())
            .with_tool_name_opt(Some("test_tool".to_string()));

        let logger = StdoutLogger::with_context("debug", ctx);
        assert!(logger.is_enabled(LogLevel::Debug));
        assert!(logger.is_enabled(LogLevel::Info)); // Info is higher than Debug
    }
}
