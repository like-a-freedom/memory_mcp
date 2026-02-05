use std::collections::HashMap;
use std::io::{self, Write};

use chrono::Utc;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct StdoutLogger {
    level: LogLevel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl StdoutLogger {
    pub fn new(level: &str) -> Self {
        Self {
            level: LogLevel::parse(level),
        }
    }

    pub fn log(&self, event: HashMap<String, Value>, level: LogLevel) {
        // In production we suppress debug and trace level logs to avoid noisy output
        if matches!(level, LogLevel::Debug | LogLevel::Trace) {
            return;
        }
        if level < self.level {
            return;
        }
        let ts = Utc::now().to_rfc3339();
        let line = Self::format_event_line_with_ts(&event, level, &ts);
        let mut stderr = io::stderr();
        let _ = stderr.write_all(line.as_bytes());
        let _ = stderr.write_all(b"\n");
        let _ = stderr.flush();
    }

    /// Format an event into a single human-readable line using a provided timestamp.
    pub(crate) fn format_event_line_with_ts(
        event: &HashMap<String, Value>,
        level: LogLevel,
        ts: &str,
    ) -> String {
        fn value_to_string(value: &Value) -> String {
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
                    let mut pairs: Vec<String> = Vec::new();
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
            const MAX_LEN: usize = 200;
            if s.len() > MAX_LEN {
                format!("{}...", &s[..MAX_LEN - 3])
            } else {
                s
            }
        }

        fn quote_if_needed(s: &str) -> String {
            if s.contains(char::is_whitespace) || s.contains('=') || s.contains('"') {
                format!("\"{}\"", s.replace('"', "'"))
            } else {
                s.to_string()
            }
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("[{}] {}", ts, level.as_str().to_uppercase()));

        let mut keys: Vec<_> = event.keys().cloned().collect();
        keys.sort();
        for k in keys {
            if let Some(v) = event.get(&k) {
                let vstr = value_to_string(v);
                parts.push(format!("{}={}", k, quote_if_needed(&vstr)));
            }
        }
        parts.join(" ")
    }
}

impl LogLevel {
    pub fn parse(level: &str) -> Self {
        match level.trim().to_lowercase().as_str() {
            "trace" => LogLevel::Trace,
            "debug" => LogLevel::Debug,
            "warn" | "warning" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            LogLevel::Trace => "trace",
            LogLevel::Debug => "debug",
            LogLevel::Info => "info",
            LogLevel::Warn => "warn",
            LogLevel::Error => "error",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
        event.insert("args".to_string(), json!({"scope":"org", "query":"ARR"}));
        let line = StdoutLogger::format_event_line_with_ts(
            &event,
            LogLevel::Info,
            "2026-01-01T00:00:00+00:00",
        );
        // name should be quoted because it contains space
        assert!(line.contains("name=\"Dmitry Ivanov\""));
        // list should be flattened
        assert!(line.contains("list=[a,b,c]"));
        // args object should be flattened and contain both key/value entries (possibly quoted)
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
        // the value should be truncated and contain ellipsis
        assert!(line.contains("..."));
        // ensure truncated len for value equals MAX_LEN (200)
        if let Some(pos) = line.find("long=") {
            let rest = &line[pos + 5..];
            let value = rest.split_whitespace().next().unwrap_or("");
            assert_eq!(value.len(), 200);
        } else {
            panic!("missing long=");
        }
    }
}
