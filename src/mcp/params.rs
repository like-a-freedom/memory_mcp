//! Parameter structures for MCP tool calls.
//!
//! All parameter structs use flat, primitive types only (no nested structs)
//! for OpenAI schema compatibility.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for the `ingest` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

// --- MCP Apps params ---

/// Parameters for open_memory_inspector (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenMemoryInspectorParams {
    pub scope: String,
    pub target_type: String,
    pub target_id: String,
    pub as_of: Option<String>,
    pub page_size: Option<i32>,
    pub cursor: Option<String>,
}

/// Parameters for refresh_memory_inspector (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RefreshMemoryInspectorParams {
    pub session_id: String,
}

/// Parameters for open_related_timeline (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenRelatedTimelineParams {
    pub session_id: String,
    pub target_type: String,
    pub target_id: String,
}

/// Parameters for invalidate_fact (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidateFactParams {
    pub session_id: String,
    pub fact_id: String,
    pub reason: Option<String>,
    pub confirmed: bool,
}

/// Parameters for archive_episode (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveEpisodeParams {
    pub session_id: String,
    pub episode_id: String,
    pub reason: Option<String>,
    pub confirmed: bool,
}

/// Parameters for copy_record_id (APP-01).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CopyRecordIdParams {
    pub session_id: String,
    pub target_id: String,
}

/// Filters for temporal diff (APP-02).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct DiffFilters {
    pub only_facts: Option<bool>,
    pub only_edges: Option<bool>,
    pub only_active: Option<bool>,
    pub only_policy_visible: Option<bool>,
}

/// Parameters for open_temporal_diff (APP-02).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenTemporalDiffParams {
    pub scope: String,
    pub target_type: String,
    pub target_id: Option<String>,
    pub as_of_left: String,
    pub as_of_right: String,
    pub time_axis: Option<String>,
    pub view: Option<String>,
    pub filters: Option<DiffFilters>,
}

/// Parameters for export_temporal_diff (APP-02).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExportTemporalDiffParams {
    pub session_id: String,
    pub format: String,
}

/// Parameters for open_ingestion_review (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenIngestionReviewParams {
    pub scope: String,
    pub source_text: Option<String>,
    pub draft_episode_id: Option<String>,
    pub ttl_seconds: Option<i64>,
}

/// Parameters for get_draft_summary (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetDraftSummaryParams {
    pub session_id: String,
}

/// Parameters for approve_ingestion_items (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ApproveIngestionItemsParams {
    pub session_id: String,
    pub item_ids: Vec<String>,
}

/// Parameters for reject_ingestion_items (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RejectIngestionItemsParams {
    pub session_id: String,
    pub item_ids: Vec<String>,
    pub reason: Option<String>,
}

/// Parameters for edit_ingestion_item (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EditIngestionItemParams {
    pub session_id: String,
    pub item_id: String,
    pub patch: IngestionItemPatch,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestionItemPatch {
    pub content: Option<String>,
    pub canonical_name: Option<String>,
    pub aliases: Option<Vec<String>>,
    pub relation: Option<String>,
    pub confidence: Option<f64>,
    pub policy_tags: Option<Vec<String>>,
}

/// Parameters for bulk_approve_by_type (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkApproveByTypeParams {
    pub session_id: String,
    pub item_type: String,
}

/// Parameters for bulk_reject_low_confidence (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct BulkRejectLowConfidenceParams {
    pub session_id: String,
    pub threshold: f64,
}

/// Parameters for commit_ingestion_review (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CommitIngestionReviewParams {
    pub session_id: String,
    pub confirmed: bool,
}

/// Parameters for cancel_ingestion_review (APP-03).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CancelIngestionReviewParams {
    pub session_id: String,
}

/// Parameters for open_memory_inspector_from_diff (APP-02).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenMemoryInspectorFromDiffParams {
    pub session_id: String,
    pub target_id: String,
    pub target_type: String,
}

/// Filters for lifecycle console (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct LifecycleFilters {
    pub min_confidence: Option<f64>,
    pub max_confidence: Option<f64>,
    pub inactive_days: Option<i32>,
    pub include_archived: Option<bool>,
}

/// Parameters for open_lifecycle_console (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenLifecycleConsoleParams {
    pub scope: String,
    pub filters: Option<LifecycleFilters>,
}

/// Parameters for archive_candidates (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveCandidatesParams {
    pub session_id: String,
    pub candidate_ids: Vec<String>,
    pub dry_run: Option<bool>,
    pub confirmed: bool,
}

/// Parameters for restore_archived (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RestoreArchivedParams {
    pub session_id: String,
    pub episode_ids: Vec<String>,
    pub confirmed: bool,
}

/// Parameters for recompute_decay (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RecomputeDecayParams {
    pub session_id: String,
    pub target_ids: Option<Vec<String>>,
    pub dry_run: Option<bool>,
}

/// Parameters for rebuild_communities (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RebuildCommunitiesParams {
    pub session_id: String,
    pub dry_run: Option<bool>,
    pub confirmed: bool,
}

/// Parameters for get_lifecycle_task_status (APP-04).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct GetLifecycleTaskStatusParams {
    pub task_id: String,
}

/// Parameters for open_graph_path (APP-05).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenGraphPathParams {
    pub scope: String,
    pub from_entity_id: String,
    pub to_entity_id: String,
    pub as_of: Option<String>,
    pub max_depth: Option<i32>,
}

/// Parameters for expand_graph_neighbors (APP-05).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExpandGraphNeighborsParams {
    pub session_id: String,
    pub entity_id: String,
    pub direction: String,
    pub depth: Option<i32>,
}

/// Parameters for open_edge_details (APP-05).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OpenEdgeDetailsParams {
    pub session_id: String,
    pub edge_id: String,
}

/// Parameters for use_path_as_context (APP-05).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UsePathAsContextParams {
    pub session_id: String,
    pub path_id: String,
}

/// Parameters for close_session (all Apps).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CloseSessionParams {
    pub session_id: String,
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

        // Fields use snake_case for MCP client compatibility
        for key in [
            "source_type",
            "source_id",
            "content",
            "t_ref",
            "scope",
            "t_ingested",
            "visibility_scope",
            "policy_tags",
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
        // Field uses snake_case for MCP client compatibility
        assert_eq!(schema["properties"]["context_items"]["type"], "string");
    }

    #[test]
    fn extract_params_schema_exposes_both_episode_and_inline_content_entry_points() {
        let schema = schema_json::<ExtractParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields use snake_case for MCP client compatibility
        for key in [
            "episode_id",
            "content",
            "text",
            "source_type",
            "source_id",
            "t_ref",
            "scope",
        ] {
            assert!(properties.contains_key(key), "missing property {key}");
        }
    }

    #[test]
    fn assemble_context_params_schema_keeps_flat_primitives() {
        let schema = schema_json::<AssembleContextParams>();
        let properties = schema["properties"].as_object().expect("properties object");

        // Fields use snake_case for MCP client compatibility
        assert_eq!(properties["query"]["type"], "string");
        assert_eq!(properties["scope"]["type"], "string");
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
    }

    #[test]
    fn open_app_params_schema_exposes_flat_variant_b_fields() {
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

        assert_eq!(properties["app"]["type"], "string");
        assert_eq!(properties["scope"]["type"], "string");
        assert_eq!(
            properties["page_size"]["type"],
            serde_json::json!(["integer", "null"])
        );
        assert_eq!(
            properties["max_depth"]["type"],
            serde_json::json!(["integer", "null"])
        );
        assert_eq!(
            properties["ttl_seconds"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }

    #[test]
    fn app_command_params_schema_exposes_flat_action_bridge_fields() {
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

        assert_eq!(properties["session_id"]["type"], "string");
        assert_eq!(properties["action"]["type"], "string");
        assert_eq!(properties["item_ids"]["type"], "array");
        assert_eq!(properties["item_ids"]["items"]["type"], "string");
        assert_eq!(properties["target_ids"]["type"], "array");
        assert_eq!(properties["target_ids"]["items"]["type"], "string");
        assert_eq!(
            properties["target_id"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["item_id"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["patch_json"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["reason"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["dry_run"]["type"],
            serde_json::json!(["boolean", "null"])
        );
        assert_eq!(
            properties["confirmed"]["type"],
            serde_json::json!(["boolean", "null"])
        );
        assert_eq!(
            properties["format"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["direction"]["type"],
            serde_json::json!(["string", "null"])
        );
        assert_eq!(
            properties["depth"]["type"],
            serde_json::json!(["integer", "null"])
        );
    }
}
