//! Parameter structures for MCP tool calls.
//!
//! All parameter structs use flat, primitive types only (no nested structs)
//! for OpenAI schema compatibility.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(crate) use crate::tools::params::{
    AssembleContextParams, ExplainParams, ExtractParams, IngestParams, InvalidateParams,
    ResolveParams,
};

/// Parameters for the public `open_app` launcher.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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
#[serde(rename_all = "snake_case", deny_unknown_fields)]
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
