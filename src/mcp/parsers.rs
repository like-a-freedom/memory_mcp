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

    // ==================== Additional Parser Tests ====================

    #[test]
    fn parse_datetime_parses_rfc3339() {
        use chrono::Datelike;
        let result = parse_datetime("2024-01-15T10:30:00Z");
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 2024);
        assert_eq!(dt.month(), 1);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn parse_datetime_parses_with_timezone() {
        let result = parse_datetime("2024-01-15T10:30:00+05:00");
        assert!(result.is_some());
    }

    #[test]
    fn parse_datetime_returns_none_for_invalid() {
        assert!(parse_datetime("invalid").is_none());
        assert!(parse_datetime("").is_none());
        assert!(parse_datetime("2024-13-45").is_none());
    }

    #[test]
    fn parse_datetime_returns_none_for_empty() {
        assert!(parse_datetime("").is_none());
    }

    #[test]
    fn default_scope_returns_org() {
        assert_eq!(default_scope(), "org");
    }

    #[test]
    fn content_hash_is_deterministic() {
        let hash1 = content_hash("test content");
        let hash2 = content_hash("test content");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn content_hash_differs_for_different_content() {
        let hash1 = content_hash("content A");
        let hash2 = content_hash("content B");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn content_hash_produces_hex_string() {
        let hash = content_hash("test");
        // First 16 chars of SHA-256 hex
        assert_eq!(hash.len(), 16);
        // All characters should be hex
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn normalize_optional_string_returns_content_for_some() {
        assert_eq!(
            normalize_optional_string(Some("test".to_string())),
            Some("test".to_string())
        );
    }

    #[test]
    fn normalize_optional_string_returns_none_for_none() {
        assert_eq!(normalize_optional_string(None), None);
    }

    #[test]
    fn normalize_optional_string_returns_none_for_empty() {
        assert_eq!(normalize_optional_string(Some("".to_string())), None);
    }

    #[test]
    fn normalize_optional_string_returns_none_for_null() {
        assert_eq!(normalize_optional_string(Some("null".to_string())), None);
    }

    #[test]
    fn normalize_optional_string_trims_whitespace() {
        assert_eq!(
            normalize_optional_string(Some("  test  ".to_string())),
            Some("test".to_string())
        );
    }

    #[test]
    fn normalize_optional_string_returns_none_for_none_input() {
        assert_eq!(normalize_optional_string(None), None::<String>);
    }

    #[test]
    fn empty_extract_result_creates_error_structure() {
        let result = empty_extract_result("no_content", "Content is required");
        assert_eq!(result["status"], "no_content");
        assert_eq!(result["hint"], "Content is required");
        assert_eq!(result["entities"], serde_json::json!([]));
        assert_eq!(result["facts"], serde_json::json!([]));
        assert_eq!(result["links"], serde_json::json!([]));
    }

    #[test]
    fn parse_context_items_prefers_source_episode_over_id() {
        let raw = r#"[{"content":"Test","id":"episode:123","source_episode":"episode:456"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].source_episode, "episode:456");
    }

    #[test]
    fn parse_context_items_uses_id_when_source_episode_missing() {
        let raw = r#"[{"content":"Test","id":"episode:123"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].source_episode, "episode:123");
    }

    #[test]
    fn parse_context_items_handles_empty_quote() {
        let raw = r#"[{"content":"Test","id":"episode:123","quote":""}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].quote, "");
    }

    #[test]
    fn parse_context_items_handles_missing_fields() {
        let raw = r#"[{"content":"Test"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].content, "Test");
        assert_eq!(items[0].source_episode, "");
        assert_eq!(items[0].quote, "");
    }

    #[test]
    fn parse_context_items_preserves_unicode() {
        let raw = r#"[{"content":"Привет мир","id":"episode:123"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].content, "Привет мир");
    }
}
