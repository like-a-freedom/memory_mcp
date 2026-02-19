//! Utility functions for parsing and validation.

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sha2::Digest;

use crate::models::ExplainItem;

/// Parse `context_items` JSON string into `Vec<ExplainItem>`.
///
/// # Accepted Input Formats
///
/// All inputs must be a JSON array. Supported element types:
///
/// 1. **Strict ExplainItem objects**: `[{"content":"…","quote":"…","source_episode":"episode:xxx"}]`
/// 2. **Array of ID strings**: `["episode:xxx","task:yyy"]`
/// 3. **Loose objects**: `[{"content":"…","id":"task:xxx","source_type":"task"}]`
///    - `id` is used as `source_episode` when `source_episode` is absent
///    - `quote` and `content` default to `""` when absent
/// 4. **Mixed**: Any combination of strings and objects in one array
///
/// # Examples
///
/// ```rust
/// use memory_mcp::mcp::parse_context_items;
///
/// let raw = r#"[{"content":"alpha","quote":"beta","source_episode":"episode:abc"}]"#;
/// let items = parse_context_items(raw).unwrap();
/// assert_eq!(items.len(), 1);
/// ```
pub fn parse_context_items(raw: &str) -> Result<Vec<ExplainItem>, String> {
    let values: Vec<Value> =
        serde_json::from_str(raw).map_err(|e| format!("Invalid context_items JSON: {e}"))?;

    let items = values
        .into_iter()
        .map(|v| match v {
            Value::String(s) => ExplainItem {
                content: String::new(),
                quote: String::new(),
                source_episode: s,
            },
            Value::Object(ref map) => {
                let content = map
                    .get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let quote = map
                    .get("quote")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let source_episode = map
                    .get("source_episode")
                    .or_else(|| map.get("id"))
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                ExplainItem {
                    content,
                    quote,
                    source_episode,
                }
            }
            _ => ExplainItem {
                content: String::new(),
                quote: String::new(),
                source_episode: String::new(),
            },
        })
        .collect();

    Ok(items)
}

/// Parse an ISO 8601 datetime string into `DateTime<Utc>`.
///
/// Returns `None` if the input is not a valid ISO 8601 datetime.
#[must_use]
pub fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| dt.with_timezone(&Utc))
        .ok()
}

/// Normalize an optional string, returning `None` for empty or "null" values.
#[must_use]
pub fn normalize_optional_string(value: Option<String>) -> Option<String> {
    let value = value?;
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("null") {
        None
    } else {
        Some(trimmed.to_string())
    }
}

/// Create an empty extraction result with status and hint.
#[must_use]
pub fn empty_extract_result(status: &str, hint: &str) -> Value {
    json!({
        "status": status,
        "hint": hint,
        "entities": [],
        "facts": [],
        "links": [],
    })
}

/// Compute a 16-character hex hash of content.
#[must_use]
pub fn content_hash(content: &str) -> String {
    let digest = sha2::Sha256::digest(content.as_bytes());
    hex::encode(digest)[..16].to_string()
}

/// Default scope for operations.
#[must_use]
pub fn default_scope() -> String {
    "org".to_string()
}

/// Default budget for context assembly.
#[must_use]
pub fn default_budget() -> i32 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_strict_explain_items() {
        let raw = r#"[{"content":"alpha","quote":"beta","source_episode":"episode:abc"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "alpha");
        assert_eq!(items[0].quote, "beta");
        assert_eq!(items[0].source_episode, "episode:abc");
    }

    #[test]
    fn parse_array_of_id_strings() {
        let raw = r#"["episode:111","task:222"]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:111");
        assert_eq!(items[0].content, "");
        assert_eq!(items[1].source_episode, "task:222");
    }

    #[test]
    fn parse_loose_objects_id_no_quote() {
        let raw = r#"[{"content":"Follow up on ARR deal","id":"task:e8g","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].content, "Follow up on ARR deal");
        assert_eq!(items[0].quote, "");
        assert_eq!(items[0].source_episode, "task:e8g");
    }

    #[test]
    fn parse_source_episode_preferred_over_id() {
        let raw =
            r#"[{"content":"x","quote":"y","source_episode":"episode:real","id":"task:alt"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].source_episode, "episode:real");
    }

    #[test]
    fn parse_mixed_array() {
        let raw = r#"["episode:aaa",{"content":"c","id":"task:bbb"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_episode, "episode:aaa");
        assert_eq!(items[0].content, "");
        assert_eq!(items[1].source_episode, "task:bbb");
        assert_eq!(items[1].content, "c");
    }

    #[test]
    fn parse_empty_array() {
        let items = parse_context_items("[]").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn parse_invalid_json_errors() {
        assert!(parse_context_items("not json").is_err());
    }

    #[test]
    fn parse_non_array_errors() {
        assert!(parse_context_items(r#"{"content":"x"}"#).is_err());
    }

    #[test]
    fn parse_real_world_payload() {
        let raw = r#"[{"content":"Follow up on ARR deal","id":"task:e8gsmlprfchnktf6js0p","source_type":"task"},{"content":"ASSIGNEE: Anton Solovey","id":"task:ha8caz3sb2fxr9ju2sbc","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].content, "Follow up on ARR deal");
        assert_eq!(items[0].source_episode, "task:e8gsmlprfchnktf6js0p");
        assert_eq!(items[1].source_episode, "task:ha8caz3sb2fxr9ju2sbc");
    }
}
