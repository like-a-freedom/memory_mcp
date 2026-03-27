//! Parameter structures for MCP tool calls.
//!
//! All parameter structs use flat, primitive types only (no nested structs)
//! for OpenAI schema compatibility.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for the `ingest` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct IngestParams {
    /// Type of source (e.g., "email", "tfs_work_item", "document")
    pub source_type: String,
    /// Unique identifier for the source
    pub source_id: String,
    /// Content to ingest
    pub content: String,
    /// Reference timestamp (ISO 8601 format)
    pub t_ref: String,
    /// Scope (default: "org")
    #[serde(default = "super::default_scope")]
    pub scope: String,
    /// Ingestion timestamp (ISO 8601 format, optional)
    pub t_ingested: Option<String>,
    /// Visibility scope (optional)
    pub visibility_scope: Option<String>,
    /// Policy tags (optional)
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Parameters for the `explain` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExplainParams {
    /// JSON array string of context items to explain.
    ///
    /// Accepted forms inside the JSON array:
    /// - objects with `content`, `quote`, `source_episode`
    /// - objects with `id` instead of `source_episode`
    /// - plain source ID strings such as `episode:abc123`
    pub context_items: String,
}

/// Parameters for the `extract` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ExtractParams {
    /// Episode ID to extract from (optional if content provided)
    pub episode_id: Option<String>,
    /// Content to analyze (optional if episode_id provided)
    pub content: Option<String>,
    /// Alternative content field
    pub text: Option<String>,
    /// Source type (default: "ad-hoc")
    pub source_type: Option<String>,
    /// Source ID (optional)
    pub source_id: Option<String>,
    /// Reference timestamp (ISO 8601 format, optional)
    pub t_ref: Option<String>,
    /// Scope (default: "org")
    pub scope: Option<String>,
}

/// Parameters for the `resolve` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResolveParams {
    /// Type of entity (e.g., "person", "project", "company")
    pub entity_type: String,
    /// Canonical name for the entity
    pub canonical_name: String,
    /// Known aliases for the entity
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Parameters for the `invalidate` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct InvalidateParams {
    /// ID of the fact to invalidate
    pub fact_id: String,
    /// Reason for invalidation
    pub reason: String,
    /// Timestamp when fact became invalid (ISO 8601 format)
    pub t_invalid: String,
}

/// Parameters for the `assemble_context` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssembleContextParams {
    /// The query to assemble context for
    pub query: String,
    /// The scope to search within
    pub scope: String,
    /// The timestamp to assemble context as-of (ISO 8601 format, default: now)
    #[serde(default)]
    pub as_of: String,
    /// Maximum number of facts to return (default: 5)
    #[serde(default = "super::default_budget")]
    pub budget: i32,
    /// Optional retrieval view mode (for example, "timeline")
    pub view_mode: Option<String>,
    /// Optional lower bound for result timestamps (ISO 8601 format)
    pub window_start: Option<String>,
    /// Optional upper bound for result timestamps (ISO 8601 format)
    pub window_end: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_json<T: JsonSchema>() -> serde_json::Value {
        serde_json::to_value(schemars::schema_for!(T)).expect("schema json")
    }

    #[test]
    fn ingest_params_schema_exposes_expected_fields() {
        let schema = schema_json::<IngestParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields are renamed to camelCase for MCP/JSON compatibility
        for key in [
            "sourceType",
            "sourceId",
            "content",
            "tRef",
            "scope",
            "tIngested",
            "visibilityScope",
            "policyTags",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
    }

    #[test]
    fn resolve_params_schema_models_aliases_as_string_array() {
        let schema = schema_json::<ResolveParams>();
        let aliases = &schema["properties"]["aliases"];

        assert_eq!(aliases["type"], "array");
        assert_eq!(aliases["items"]["type"], "string");
    }

    #[test]
    fn explain_params_schema_requires_json_array_string() {
        let schema = schema_json::<ExplainParams>();
        // Field is renamed to camelCase for MCP/JSON compatibility
        assert_eq!(schema["properties"]["contextItems"]["type"], "string");
    }

    #[test]
    fn extract_params_schema_exposes_both_episode_and_inline_content_entry_points() {
        let schema = schema_json::<ExtractParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields are renamed to camelCase for MCP/JSON compatibility
        for key in [
            "episodeId",
            "content",
            "text",
            "sourceType",
            "sourceId",
            "tRef",
            "scope",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
    }

    #[test]
    fn assemble_context_params_schema_keeps_flat_primitives() {
        let schema = schema_json::<AssembleContextParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields are renamed to camelCase for MCP/JSON compatibility
        assert_eq!(properties["query"]["type"], "string");
        assert_eq!(properties["scope"]["type"], "string");
        assert_eq!(properties["asOf"]["type"], "string");
        assert_eq!(properties["budget"]["type"], "integer");
        assert_eq!(
            properties["viewMode"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["windowStart"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["windowEnd"]["type"],
            serde_json::json!(["string", "null"])
        );
    }
}
