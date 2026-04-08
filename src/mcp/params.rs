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
    /// Optional project tag for project-scoped retrieval
    pub project: Option<String>,
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
    /// Optional project tag to restrict retrieval to one project
    pub project: Option<String>,
    /// Optional fact types to include in the response
    #[serde(default)]
    pub fact_types: Vec<String>,
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

/// Parameters for the public `open_app` launcher.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenAppParams {
    /// Public app identifier (for example: inspector, diff, ingestion_review, lifecycle, graph).
    pub app: String,
    /// Scope for the app session.
    pub scope: String,
    /// Target kind for entity/fact/episode-driven apps.
    pub target_type: Option<String>,
    /// Target identifier for entity/fact/episode-driven apps.
    pub target_id: Option<String>,
    /// Source entity for graph navigation.
    pub from_entity_id: Option<String>,
    /// Destination entity for graph navigation.
    pub to_entity_id: Option<String>,
    /// Inline source text for ingestion review.
    pub source_text: Option<String>,
    /// Existing draft episode identifier for ingestion review.
    pub draft_episode_id: Option<String>,
    /// Timestamp for single-timepoint views.
    pub as_of: Option<String>,
    /// Left boundary timestamp for temporal diff.
    pub as_of_left: Option<String>,
    /// Right boundary timestamp for temporal diff.
    pub as_of_right: Option<String>,
    /// Time axis for temporal diff.
    pub time_axis: Option<String>,
    /// Optional app view variant.
    pub view: Option<String>,
    /// Cursor for paginated app views.
    pub cursor: Option<String>,
    /// Page size for paginated app views.
    pub page_size: Option<i32>,
    /// Maximum path depth for graph navigation.
    pub max_depth: Option<i32>,
    /// Optional session TTL in seconds.
    pub ttl_seconds: Option<i64>,
}

/// Parameters for the public `app_command` bridge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AppCommandParams {
    /// Session identifier returned by `open_app`.
    pub session_id: String,
    /// Coarse-grained action name for the active app session.
    pub action: String,
    /// Draft item identifiers for bulk review actions.
    #[serde(default)]
    pub item_ids: Vec<String>,
    /// Generic target identifiers for lifecycle and graph-like batch actions.
    #[serde(default)]
    pub target_ids: Vec<String>,
    /// Generic singular target identifier for graph-like session actions.
    pub target_id: Option<String>,
    /// Singular draft item identifier for edit-like actions.
    pub item_id: Option<String>,
    /// Optional JSON object payload encoded as a string for edit-like actions.
    pub patch_json: Option<String>,
    /// Optional rationale for rejection-like actions.
    pub reason: Option<String>,
    /// Optional dry-run flag for destructive actions.
    pub dry_run: Option<bool>,
    /// Optional explicit confirmation flag for destructive actions.
    pub confirmed: Option<bool>,
    /// Optional export format for diff-like actions.
    pub format: Option<String>,
    /// Optional graph traversal direction for graph exploration commands.
    pub direction: Option<String>,
    /// Optional graph traversal depth for graph exploration commands.
    pub depth: Option<i32>,
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
            "project",
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
        assert_eq!(
            properties["project"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(properties["factTypes"]["type"], "array");
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

    #[test]
    fn open_app_params_schema_preserves_historical_snake_case_contract() {
        let schema = schema_json::<OpenAppParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        for key in [
            "app",
            "scope",
            "target_type",
            "target_id",
            "from_entity_id",
            "to_entity_id",
            "source_text",
            "draft_episode_id",
            "as_of",
            "as_of_left",
            "as_of_right",
            "time_axis",
            "view",
            "cursor",
            "page_size",
            "max_depth",
            "ttl_seconds",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        assert!(
            !properties.contains_key("targetType"),
            "open_app should keep snake_case field names for parity"
        );
    }

    #[test]
    fn app_command_params_schema_preserves_historical_snake_case_contract() {
        let schema = schema_json::<AppCommandParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        for key in [
            "session_id",
            "action",
            "item_ids",
            "target_ids",
            "target_id",
            "item_id",
            "patch_json",
            "reason",
            "dry_run",
            "confirmed",
            "format",
            "direction",
            "depth",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }

        assert!(
            !properties.contains_key("sessionId"),
            "app_command should keep snake_case field names for parity"
        );
    }
}
