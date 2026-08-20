//! Parameter structures for protocol-agnostic tool calls.
//!
//! All parameter structs use flat, primitive types only (no nested structs)
//! for OpenAI schema compatibility.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for the `ingest` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct IngestParams {
    /// Type of source (e.g., "email", "tfs_work_item", "document")
    pub source_type: String,
    /// Unique identifier for the source
    pub source_id: String,
    /// Content to ingest
    pub content: String,
    /// Reference timestamp (ISO 8601 format)
    pub t_ref: String,
    /// Ingestion timestamp (ISO 8601 format, optional)
    pub t_ingested: Option<String>,
    /// Policy tags (optional)
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Parameters for the `explain` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ExplainParams {
    /// JSON array string of context items to explain.
    ///
    /// Accepted forms inside the JSON array:
    /// - objects with snake_case keys such as `content`, `quote`, `fact_id`, and `source_episode`
    /// - plain source ID strings such as `episode:abc123`
    pub context_items: String,
    /// Request compact (token-efficient) response. Defaults to true.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
}

/// Parameters for the `extract` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ExtractParams {
    /// Episode ID to extract from (optional if content provided).
    /// Use the canonical `episode:<id>` form exactly as returned by `ingest`;
    /// a bare hex ID without the `episode:` prefix is rejected.
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

    /// Custom zero-shot entity labels for GLiNER (opt-in, overrides default labels)
    pub zero_shot_labels: Option<Vec<String>>,
}

/// Parameters for the `resolve` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct InvalidateParams {
    /// ID of the fact to invalidate.
    /// Use the canonical `fact:<id>` form exactly as returned by `extract`;
    /// a bare hex ID without the `fact:` prefix is rejected.
    pub fact_id: String,
    /// Reason for invalidation
    pub reason: String,
    /// Timestamp when fact became invalid (ISO 8601 format)
    pub t_invalid: String,
}

/// Parameters for the `assemble_context` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct AssembleContextParams {
    /// The query to assemble context for
    pub query: String,

    /// Optional fact types to include in the response
    #[serde(default)]
    pub fact_types: Vec<String>,
    /// The timestamp to assemble context as-of (ISO 8601 format, default: now)
    #[serde(default)]
    pub as_of: String,
    /// Maximum number of facts to return (default: 5)
    #[serde(default = "crate::tools::parsers::default_budget")]
    pub budget: i32,
    /// Optional retrieval view mode (for example, "timeline")
    pub view_mode: Option<String>,
    /// Optional lower bound for result timestamps (ISO 8601 format)
    pub window_start: Option<String>,
    /// Optional upper bound for result timestamps (ISO 8601 format)
    pub window_end: Option<String>,
    /// Request compact (token-efficient) response. Defaults to true.
    /// Set to false for verbose debug output including full rationale strings.
    #[serde(default = "crate::tools::parsers::default_compact")]
    pub compact: bool,
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

        // Public MCP tool parameters use snake_case keys only.
        for key in [
            "source_type",
            "source_id",
            "content",
            "t_ref",
            "t_ingested",
            "policy_tags",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        for key in ["sourceType", "sourceId", "tRef", "tIngested", "policyTags"] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
        }
    }

    #[test]
    fn ingest_params_reject_legacy_partition_fields() {
        let err = serde_json::from_value::<IngestParams>(serde_json::json!({
            "source_type": "email",
            "source_id": "msg-1",
            "content": "hello",
            "t_ref": "2026-01-01T00:00:00Z",
            "scope": "org"
        }))
        .expect_err("legacy scope must be rejected");
        assert!(err.to_string().contains("scope"));

        let err = serde_json::from_value::<IngestParams>(serde_json::json!({
            "source_type": "email",
            "source_id": "msg-1",
            "content": "hello",
            "t_ref": "2026-01-01T00:00:00Z",
            "project": "atlas"
        }))
        .expect_err("legacy project must be rejected");
        assert!(err.to_string().contains("project"));
    }

    #[test]
    fn ingest_params_reject_camel_case_fields() {
        let err = serde_json::from_value::<IngestParams>(serde_json::json!({
            "sourceType": "email",
            "sourceId": "msg-1",
            "content": "hello",
            "tRef": "2026-01-01T00:00:00Z"
        }))
        .expect_err("camelCase ingest params should be rejected");

        assert!(
            err.to_string().contains("sourceType") || err.to_string().contains("sourceId"),
            "unexpected error: {err}"
        );
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
        assert_eq!(schema["properties"]["context_items"]["type"], "string");
        assert!(schema["properties"].get("contextItems").is_none());
        assert_eq!(schema["properties"]["compact"]["type"], "boolean");
    }

    #[test]
    fn explain_params_compact_defaults_true_when_omitted() {
        let params: ExplainParams =
            serde_json::from_value(serde_json::json!({"context_items": "[]"})).unwrap();
        assert!(params.compact, "compact must default to true");
    }

    #[test]
    fn explain_params_compact_explicit_false() {
        let params: ExplainParams =
            serde_json::from_value(serde_json::json!({"context_items": "[]", "compact": false}))
                .unwrap();
        assert!(!params.compact);
    }

    #[test]
    fn extract_params_schema_exposes_both_episode_and_inline_content_entry_points() {
        let schema = schema_json::<ExtractParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Public MCP tool parameters use snake_case keys only.
        for key in [
            "episode_id",
            "content",
            "text",
            "source_type",
            "source_id",
            "t_ref",
            "zero_shot_labels",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        for key in [
            "episodeId",
            "sourceType",
            "sourceId",
            "tRef",
            "zeroShotLabels",
        ] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
        }

        // Verify zero_shot_labels is an optional array of strings.
        assert_eq!(
            properties["zero_shot_labels"]["type"],
            serde_json::json!(["array", "null"])
        );

        // Verify the items in zero_shot_labels array are strings.
        let zero_shot_labels_schema = &properties["zero_shot_labels"];
        assert_eq!(
            zero_shot_labels_schema["items"]["type"], "string",
            "zero_shot_labels items should be strings"
        );
    }

    #[test]
    fn extract_params_reject_nested_payload_and_camel_case_fields() {
        let camel_case_err = serde_json::from_value::<ExtractParams>(serde_json::json!({
            "episodeId": "episode:123"
        }))
        .expect_err("camelCase extract params should be rejected");
        assert!(
            camel_case_err.to_string().contains("episodeId"),
            "unexpected error: {camel_case_err}"
        );

        let payload_err = serde_json::from_value::<ExtractParams>(serde_json::json!({
            "payload": {
                "episode_id": "episode:123"
            }
        }))
        .expect_err("nested payload wrapper should be rejected");
        assert!(
            payload_err.to_string().contains("payload"),
            "unexpected error: {payload_err}"
        );
    }

    #[test]
    fn record_id_fields_document_canonical_table_prefix_form() {
        let extract = schema_json::<ExtractParams>();
        let episode_description = extract["properties"]["episode_id"]["description"]
            .as_str()
            .expect("episode_id description");
        assert!(
            episode_description.contains("episode:<id>"),
            "episode_id description must name the canonical form: {episode_description}"
        );

        let invalidate = schema_json::<InvalidateParams>();
        let fact_description = invalidate["properties"]["fact_id"]["description"]
            .as_str()
            .expect("fact_id description");
        assert!(
            fact_description.contains("fact:<id>"),
            "fact_id description must name the canonical form: {fact_description}"
        );
    }

    #[test]
    fn assemble_context_params_schema_keeps_flat_primitives() {
        let schema = schema_json::<AssembleContextParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Public MCP tool parameters use snake_case keys only.
        assert_eq!(properties["query"]["type"], "string");
        assert!(!properties.contains_key("scope"));
        assert!(!properties.contains_key("project"));
        assert_eq!(properties["fact_types"]["type"], "array");
        assert_eq!(properties["as_of"]["type"], "string");
        assert_eq!(properties["budget"]["type"], "integer");
        assert_eq!(
            properties["view_mode"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["window_start"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["window_end"]["type"],
            serde_json::json!(["string", "null"])
        );

        for key in ["factTypes", "asOf", "viewMode", "windowStart", "windowEnd"] {
            assert!(
                !properties.contains_key(key),
                "unexpected camelCase property {key}"
            );
        }

        // compact is an optional boolean (not required).
        assert_eq!(properties["compact"]["type"], "boolean");
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            !required.iter().any(|v| v.as_str() == Some("compact")),
            "compact must not be a required property"
        );
    }

    #[test]
    fn assemble_context_params_compact_defaults_true_when_omitted() {
        let params: AssembleContextParams = serde_json::from_value(serde_json::json!({
            "query": "q"
        }))
        .unwrap();
        assert!(
            serde_json::from_value::<AssembleContextParams>(serde_json::json!({
                "query": "q",
                "scope": "org"
            }))
            .is_err()
        );
        assert!(params.compact, "compact must default to true");
    }
}
