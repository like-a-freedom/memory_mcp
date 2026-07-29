//! Error conversion utilities for MCP protocol.
//!
//! All MCP errors carry structured `data` with `guidance` (what to do next)
//! so that AI callers can respond intelligently.  Where the error message
//! alone is insufficient, `data` also includes `explanation`.
//!
//! `NotFound` errors additionally include `resource_type` and `missing_id`.
//! `Storage` / `Transient` errors include `retryable: true`.

use rmcp::ErrorData;
#[cfg(test)]
use rmcp::model::ErrorCode;
use serde_json::{Value, json};

use crate::service::MemoryError;

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Build a structured tool error with contextual guidance and explanation.
///
/// Unlike `mcp_error`, this factory is used for _tool-specific_ input
/// validation where the caller needs to know **what** was wrong and
/// **how to fix it**.
#[cfg(test)]
pub(crate) fn tool_error(
    code: ErrorCode,
    message: impl Into<String>,
    guidance: impl Into<String>,
    explanation: impl Into<String>,
) -> ErrorData {
    let data = json!({
        "guidance": guidance.into(),
        "explanation": explanation.into(),
    });
    ErrorData::new(code, message.into(), Some(data))
}

/// Converts a `MemoryError` into a structured MCP `ErrorData` response.
///
/// Guidance and explanation are placed in `data` rather than concatenated
/// into the message string.
pub fn mcp_error(err: MemoryError) -> ErrorData {
    match &err {
        MemoryError::Validation(msg) => {
            let data = json!({
                "guidance": "Review the input arguments, fix any invalid values, and retry.",
                "explanation": msg,
            });
            ErrorData::invalid_params(err.to_string(), Some(data))
        }
        MemoryError::NotFound(msg) => {
            let parsed = ParsedNotFound::from_msg(msg);
            let data = parsed.to_data();
            ErrorData::invalid_params(parsed.clean_msg, Some(data))
        }
        MemoryError::ConfigMissing(msg) => {
            let data = json!({
                "guidance": "The server is missing a required configuration value. Update the configuration and restart.",
                "explanation": msg,
            });
            ErrorData::invalid_request(err.to_string(), Some(data))
        }
        MemoryError::ConfigInvalid(msg) => {
            let data = json!({
                "guidance": "The server has an invalid configuration value. Fix it and restart.",
                "explanation": msg,
            });
            ErrorData::invalid_request(err.to_string(), Some(data))
        }
        MemoryError::Storage(msg) => {
            let data = json!({
                "guidance": "A server-side storage error occurred. Retry the request; if it persists, inspect server logs.",
                "explanation": msg,
                "retryable": true,
            });
            ErrorData::internal_error(err.to_string(), Some(data))
        }
        MemoryError::Transient(msg) => {
            let data = json!({
                "guidance": "A temporary error occurred — typically a rate limit or resource contention. Retry after a brief delay.",
                "explanation": msg,
                "retryable": true,
            });
            ErrorData::internal_error(err.to_string(), Some(data))
        }
        MemoryError::Conflict(msg) => {
            let data = json!({
                "guidance": "A capture identity conflict occurred. The event exists with a different identity.",
                "explanation": msg,
            });
            ErrorData::invalid_request(err.to_string(), Some(data))
        }
        MemoryError::BudgetExhausted(msg) => {
            let data = json!({
                "guidance": "The capture budget is exhausted before episode preparation.",
                "explanation": msg,
            });
            ErrorData::invalid_request(err.to_string(), Some(data))
        }
    }
}

// ---------------------------------------------------------------------------
// ParsedNotFound — single source of truth for not-found message parsing
// ---------------------------------------------------------------------------

/// Structured representation of a `NotFound` error message.
///
/// Parsed once, used for both the clean user-visible message and the
/// enriched `data` payload.  Adding a new resource type requires changing
/// **one** method rather than two.
struct ParsedNotFound {
    resource_type: &'static str,
    missing_id: Option<String>,
    clean_msg: String,
}

impl ParsedNotFound {
    /// Parse a raw `MemoryError::NotFound` inner message into its components.
    ///
    /// Expected input patterns (produced by the service layer):
    /// - `"episode_id not found"`  /  `"episode_id not found: <id>"`
    /// - `"fact_id not found"`  /  `"fact_id not found: <id>"`
    /// - `"fact_id not found for background embedding: <id>"`
    /// - `"entity not found"`  /  `"entity not found: <name>"`
    ///   /  `"entity not found for name: <name>"`
    /// - anything else → generic fallback.
    ///
    /// Longer (more specific) prefixes are listed first so they win over
    /// shorter ones in the same resource type group.
    fn from_msg(msg: &str) -> Self {
        // (canonical resource_type, [prefixes ordered longest-first])
        const PREFIXES: &[(&str, &[&str])] = &[
            (
                "episode",
                &[
                    "episode_id not found: ", // "episode_id not found: 123"
                    "episode_id not found",   // "episode_id not found" (exact)
                ],
            ),
            (
                "fact",
                &[
                    "fact_id not found for background embedding: ", // "fact_id not found for background embedding: 123"
                    "fact_id not found: ",                          // "fact_id not found: 123"
                    "fact_id not found",                            // "fact_id not found" (exact)
                ],
            ),
            (
                "entity",
                &[
                    "entity not found for name: ", // "entity not found for name: John"
                    "entity not found: ",          // "entity not found: Acme"
                    "entity not found",            // "entity not found" (exact)
                ],
            ),
        ];

        for &(resource_type, prefixes) in PREFIXES {
            for prefix in prefixes {
                if let Some(rest) = msg.strip_prefix(prefix) {
                    if rest.is_empty() {
                        return Self {
                            resource_type,
                            missing_id: None,
                            clean_msg: format!("{} not found", title(resource_type)),
                        };
                    }
                    return Self {
                        resource_type,
                        missing_id: Some(rest.to_string()),
                        clean_msg: format!("{} not found: {rest}", title(resource_type)),
                    };
                }
            }
        }
        Self {
            resource_type: "resource",
            missing_id: None,
            clean_msg: format!("Not found: {msg}"),
        }
    }

    /// Build the enriched `data` JSON value.
    fn to_data(&self) -> Value {
        let explanation = format!(
            "The {} was not found in the database. \
             It may have been deleted, archived, or the identifier may be incorrect.",
            self.resource_type,
        );

        let mut map = serde_json::Map::new();
        map.insert(
            "guidance".into(),
            Value::String(
                "Verify the identifier is correct. If the record was recently archived, \
                 try assemble_context instead to find relevant information."
                    .into(),
            ),
        );
        map.insert("explanation".into(), Value::String(explanation));
        map.insert(
            "resource_type".into(),
            Value::String(self.resource_type.into()),
        );
        if let Some(ref id) = self.missing_id {
            map.insert("missing_id".into(), Value::String(id.clone()));
        }
        Value::Object(map)
    }
}

fn title(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- mcp_error mapping ------------------------------------------------

    #[test]
    fn maps_validation_to_invalid_params() {
        let err = MemoryError::Validation("field is required".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp_err.message.contains("field is required"));
    }

    #[test]
    fn maps_not_found_to_invalid_params() {
        let err = MemoryError::NotFound("episode_id not found: abc123".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn maps_config_missing_to_invalid_request() {
        let err = MemoryError::ConfigMissing("SURREALDB_URL".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_REQUEST);
    }

    #[test]
    fn maps_config_invalid_to_invalid_request() {
        let err = MemoryError::ConfigInvalid("invalid value".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_REQUEST);
    }

    #[test]
    fn maps_storage_to_internal_error() {
        let err = MemoryError::Storage("database error".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INTERNAL_ERROR);
    }

    #[test]
    fn maps_transient_to_internal_error() {
        let err = MemoryError::Transient("provider rate limited".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INTERNAL_ERROR);
    }

    // -- message cleanliness ----------------------------------------------

    #[test]
    fn message_no_longer_contains_guidance_text() {
        let err = MemoryError::NotFound("episode_id not found: abc123".to_string());
        let mcp_err = mcp_error(err);
        assert!(
            !mcp_err.message.contains("Guidance"),
            "guidance should not be concatenated into message: {}",
            mcp_err.message
        );
    }

    #[test]
    fn cleans_not_found_episode_id() {
        let err = MemoryError::NotFound("episode_id not found: abc123".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.message, "Episode not found: abc123");
    }

    #[test]
    fn cleans_not_found_fact_id() {
        let err = MemoryError::NotFound("fact_id not found: fact:xyz".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.message, "Fact not found: fact:xyz");
    }

    #[test]
    fn cleans_not_found_entity() {
        let err = MemoryError::NotFound("entity not found: Acme Corp".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.message, "Entity not found: Acme Corp");
    }

    #[test]
    fn cleans_not_found_entity_for_name() {
        let err = MemoryError::NotFound("entity not found for name: John".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.message, "Entity not found: John");
    }

    #[test]
    fn falls_through_to_generic_for_unknown_not_found() {
        let err = MemoryError::NotFound("what even is this".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.message, "Not found: what even is this");
    }

    // -- structured data --------------------------------------------------

    #[test]
    fn data_contains_guidance() {
        let err = MemoryError::Validation("bad input".to_string());
        let mcp_err = mcp_error(err);
        let data = mcp_err.data.expect("data should be Some");
        assert!(data.get("guidance").and_then(|v| v.as_str()).is_some(),);
    }

    #[test]
    fn data_contains_explanation() {
        let err = MemoryError::NotFound("episode_id not found: abc123".to_string());
        let mcp_err = mcp_error(err);
        let data = mcp_err.data.expect("data should be Some");
        assert!(data.get("explanation").and_then(|v| v.as_str()).is_some(),);
    }

    #[test]
    fn not_found_includes_resource_type_and_missing_id() {
        let err = MemoryError::NotFound("episode_id not found: abc123".to_string());
        let mcp_err = mcp_error(err);
        let data = mcp_err.data.expect("data should be Some");
        assert_eq!(data["resource_type"], "episode");
        assert_eq!(data["missing_id"], "abc123");
    }

    #[test]
    fn storage_includes_retryable() {
        let err = MemoryError::Storage("db down".to_string());
        let mcp_err = mcp_error(err);
        let data = mcp_err.data.expect("data should be Some");
        assert_eq!(data["retryable"], true);
    }

    #[test]
    fn transient_includes_retryable() {
        let err = MemoryError::Transient("rate limited".to_string());
        let mcp_err = mcp_error(err);
        let data = mcp_err.data.expect("data should be Some");
        assert_eq!(data["retryable"], true);
    }

    // -- tool_error builder -----------------------------------------------

    #[test]
    fn tool_error_includes_message_and_data() {
        let err = tool_error(
            ErrorCode::INVALID_PARAMS,
            "Bad request",
            "Fix the input",
            "The input was malformed",
        );
        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert_eq!(err.message, "Bad request");
        let data = err.data.expect("data should be Some");
        assert_eq!(data["guidance"], "Fix the input");
        assert_eq!(data["explanation"], "The input was malformed");
    }

    // -- ParsedNotFound ---------------------------------------------------

    #[test]
    fn parsed_not_found_episode() {
        let p = ParsedNotFound::from_msg("episode_id not found: abc123");
        assert_eq!(p.resource_type, "episode");
        assert_eq!(p.missing_id.as_deref(), Some("abc123"));
        assert_eq!(p.clean_msg, "Episode not found: abc123");
    }

    #[test]
    fn parsed_not_found_fact() {
        let p = ParsedNotFound::from_msg("fact_id not found: fact:xyz");
        assert_eq!(p.resource_type, "fact");
        assert_eq!(p.missing_id.as_deref(), Some("fact:xyz"));
        assert_eq!(p.clean_msg, "Fact not found: fact:xyz");
    }

    #[test]
    fn parsed_not_found_entity() {
        let p = ParsedNotFound::from_msg("entity not found: Acme");
        assert_eq!(p.resource_type, "entity");
        assert_eq!(p.missing_id.as_deref(), Some("Acme"));
    }

    #[test]
    fn parsed_not_found_entity_for_name() {
        let p = ParsedNotFound::from_msg("entity not found for name: Jane");
        assert_eq!(p.resource_type, "entity");
        assert_eq!(p.missing_id.as_deref(), Some("Jane"));
    }

    #[test]
    fn parsed_not_found_episode_without_id() {
        let p = ParsedNotFound::from_msg("episode_id not found");
        assert_eq!(p.resource_type, "episode");
        assert_eq!(p.missing_id, None);
        assert_eq!(p.clean_msg, "Episode not found");
    }

    #[test]
    fn parsed_not_found_fact_without_id() {
        let p = ParsedNotFound::from_msg("fact_id not found");
        assert_eq!(p.resource_type, "fact");
        assert_eq!(p.missing_id, None);
        assert_eq!(p.clean_msg, "Fact not found");
    }

    #[test]
    fn parsed_not_found_fact_for_background_embedding() {
        let p = ParsedNotFound::from_msg("fact_id not found for background embedding: fact:abc");
        assert_eq!(p.resource_type, "fact");
        assert_eq!(p.missing_id.as_deref(), Some("fact:abc"));
        assert_eq!(p.clean_msg, "Fact not found: fact:abc");
    }

    #[test]
    fn parsed_not_found_unknown() {
        let p = ParsedNotFound::from_msg("something went wrong");
        assert_eq!(p.resource_type, "resource");
        assert_eq!(p.missing_id, None);
        assert_eq!(p.clean_msg, "Not found: something went wrong");
    }

    #[test]
    fn parsed_not_found_data_contains_resource_type() {
        let p = ParsedNotFound::from_msg("episode_id not found: x");
        let data = p.to_data();
        assert_eq!(data["resource_type"], "episode");
        assert_eq!(data["missing_id"], "x");
        assert!(data.get("guidance").is_some());
        assert!(data.get("explanation").is_some());
    }
}
