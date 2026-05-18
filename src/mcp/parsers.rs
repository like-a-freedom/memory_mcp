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
/// 1. **Strict snake_case objects**: `[{"content":"…","quote":"…","source_episode":"episode:xxx"}]`
/// 2. **Array of ID strings**: `["episode:xxx","task:yyy"]`
/// 3. **Mixed**: Any combination of strings and strict snake_case objects in one array
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
        .map(|v| -> Result<ExplainItem, String> {
            Ok(match v {
                Value::String(s) => ExplainItem {
                    content: String::new(),
                    quote: String::new(),
                    source_episode: s.trim().to_string(),
                    scope: None,
                    t_ref: None,
                    t_ingested: None,
                    provenance: Value::Null,
                    citation_context: None,
                    ..Default::default()
                },
                Value::Object(ref map) => {
                    reject_legacy_context_item_aliases(map)?;

                    let fact_id = map.get("fact_id").and_then(Value::as_str).map(String::from);
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
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    ExplainItem {
                        fact_id,
                        content,
                        quote,
                        source_episode,
                        scope: None,
                        t_ref: None,
                        t_ingested: None,
                        provenance: Value::Null,
                        citation_context: None,
                        ..Default::default()
                    }
                }
                _ => {
                    return Err(
                        "context_items must be a JSON array of strings or snake_case objects"
                            .to_string(),
                    );
                }
            })
        })
        .collect::<Result<Vec<_>, _>>()?;

    for item in &items {
        if item.source_episode.trim().is_empty() {
            return Err(
                "context_items entries must include a non-empty `source_episode`".to_string(),
            );
        }
    }

    Ok(items)
}

fn reject_legacy_context_item_aliases(map: &serde_json::Map<String, Value>) -> Result<(), String> {
    for (legacy_key, canonical_key) in [
        ("factId", "fact_id"),
        ("sourceEpisode", "source_episode"),
        ("citationContext", "citation_context"),
        ("allSources", "all_sources"),
        ("graphInsights", "graph_insights"),
        ("tRef", "t_ref"),
        ("tIngested", "t_ingested"),
    ] {
        if map.contains_key(legacy_key) {
            return Err(format!(
                "context_items objects must use snake_case keys; use `{canonical_key}` instead of `{legacy_key}`"
            ));
        }
    }

    if map.contains_key("id") {
        return Err(
            "context_items objects must use `source_episode` instead of legacy `id`.".to_string(),
        );
    }

    if map.contains_key("sourceType") {
        return Err(
            "context_items objects must use snake_case keys; `sourceType` is not part of the strict explain-item contract."
                .to_string(),
        );
    }

    Ok(())
}

/// Parse an ISO 8601 datetime string into `DateTime<Utc>`.
///
/// Returns `None` if the input is not a valid ISO 8601 datetime.
///
/// Accepts RFC 3339 timestamps, including the common ISO 8601 variant
/// where seconds are omitted (e.g. `2026-05-11T17:34Z`).
#[must_use]
pub fn parse_datetime(value: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = DateTime::parse_from_rfc3339(value) {
        return Some(dt.with_timezone(&Utc));
    }

    // Retry with seconds inserted when they're omitted after minutes.
    // ISO 8601 allows "HH:MM" instead of "HH:MM:SS"; RFC 3339 doesn't.
    // Transform "T17:34Z" -> "T17:34:00Z" and "T17:34+05:00" -> "T17:34:00+05:00"
    if let Some(patched) = try_insert_seconds(value)
        && let Ok(dt) = DateTime::parse_from_rfc3339(&patched)
    {
        return Some(dt.with_timezone(&Utc));
    }

    None
}

fn try_insert_seconds(value: &str) -> Option<String> {
    // Locate the 'T' separator between date and time.
    let t_pos = value.find('T')?;
    // Find timezone marker (Z, +, -) that appears after the 'T'.
    let tz_pos = value[t_pos + 1..]
        .find(['Z', '+', '-'])
        .map(|p| t_pos + 1 + p)?;
    // Length of the time portion between T and the timezone marker.
    let time_len = tz_pos - t_pos - 1;
    // If it's just "HH:MM" (5 chars), seconds are missing.
    if time_len == 5 {
        let (pre, post) = value.split_at(tz_pos);
        Some(format!("{pre}:00{post}"))
    } else {
        None
    }
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
    fn parse_context_items_rejects_legacy_id_alias() {
        let raw = r#"[{"content":"Follow up on ARR deal","id":"task:e8g"}]"#;
        let err = parse_context_items(raw).expect_err("legacy id alias should be rejected");
        assert!(err.contains("source_episode"), "unexpected error: {err}");
    }

    #[test]
    fn parse_context_items_rejects_camel_case_source_episode_alias() {
        let raw = r#"[{"content":"x","quote":"y","sourceEpisode":"episode:real"}]"#;
        let err =
            parse_context_items(raw).expect_err("camelCase sourceEpisode alias should be rejected");
        assert!(err.contains("source_episode"), "unexpected error: {err}");
    }

    #[test]
    fn parse_mixed_array() {
        let raw = r#"["episode:aaa",{"content":"c","source_episode":"task:bbb"}]"#;
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
        let raw = r#"[{"content":"Follow up on ARR deal","source_episode":"task:e8gsmlprfchnktf6js0p","source_type":"task"},{"content":"ASSIGNEE: Anton Solovey","source_episode":"task:ha8caz3sb2fxr9ju2sbc","source_type":"task"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].content, "Follow up on ARR deal");
        assert_eq!(items[0].source_episode, "task:e8gsmlprfchnktf6js0p");
        assert_eq!(items[1].source_episode, "task:ha8caz3sb2fxr9ju2sbc");
    }

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
    fn parse_datetime_parses_without_seconds_zulu() {
        assert!(parse_datetime("2026-05-11T17:34Z").is_some());
    }

    #[test]
    fn parse_datetime_parses_without_seconds_offset() {
        assert!(parse_datetime("2026-05-11T17:34+05:00").is_some());
    }

    #[test]
    fn parse_datetime_returns_none_for_garbage() {
        assert!(parse_datetime("not-a-date").is_none());
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
        assert_eq!(hash.len(), 16);
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
        let raw = r#"[{"content":"Test","source_episode":"episode:456"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].source_episode, "episode:456");
    }

    #[test]
    fn parse_context_items_rejects_camel_case_fact_id_alias() {
        let raw = r#"[{"content":"Test","quote":"Q","source_episode":"episode:123","factId":"fact:123"}]"#;
        let err = parse_context_items(raw).expect_err("camelCase factId alias should be rejected");
        assert!(err.contains("fact_id"), "unexpected error: {err}");
    }

    #[test]
    fn parse_context_items_handles_empty_quote() {
        let raw = r#"[{"content":"Test","source_episode":"episode:123","quote":""}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].quote, "");
    }

    #[test]
    fn parse_context_items_handles_missing_fields() {
        let raw = r#"[{"content":"Test"}]"#;
        let err = parse_context_items(raw)
            .expect_err("context items without a source_episode should be rejected");
        assert!(err.contains("source_episode"), "unexpected error: {err}");
    }

    #[test]
    fn parse_context_items_preserves_unicode() {
        let raw = r#"[{"content":"Hello world ✓","source_episode":"episode:123"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items[0].content, "Hello world ✓");
    }

    #[test]
    fn parse_context_items_rejects_empty_string_element() {
        let raw = r#"[""]"#;
        let err =
            parse_context_items(raw).expect_err("empty string context item should be rejected");
        assert!(err.contains("source_episode"), "unexpected error: {err}");
    }

    #[test]
    fn parse_context_items_object_with_only_source_episode() {
        let raw = r#"[{"source_episode":"episode:xyz"}]"#;
        let items = parse_context_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].source_episode, "episode:xyz");
        assert_eq!(items[0].content, "");
        assert_eq!(items[0].quote, "");
    }

    #[test]
    fn parse_context_items_rejects_fact_id_with_empty_source_episode() {
        let raw = r#"[{"fact_id":"fact:abc","source_episode":""}]"#;
        let err = parse_context_items(raw)
            .expect_err("empty source_episode should be rejected even when fact_id is present");
        assert!(err.contains("source_episode"), "unexpected error: {err}");
    }
}
