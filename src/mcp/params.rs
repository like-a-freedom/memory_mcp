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
    /// JSON array of context items to explain
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
    /// Known aliases (comma-separated or JSON array string)
    #[serde(default)]
    pub aliases: String,
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
}

/// Parameters for the `ingest_document` alias tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AliasIngestParams {
    /// Type of source
    pub source_type: String,
    /// Unique source ID
    pub source_id: String,
    /// Content to ingest
    pub content: String,
    /// Reference timestamp (ISO 8601)
    pub t_ref: String,
    /// Scope
    #[serde(default = "super::default_scope")]
    pub scope: String,
    /// Ingestion timestamp (optional)
    pub t_ingested: Option<String>,
    /// Visibility scope (optional)
    pub visibility_scope: Option<String>,
    /// Policy tags (optional)
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Parameters for the `extract_entities` alias tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AliasExtractParams {
    /// Episode ID (optional if content provided)
    pub episode_id: Option<String>,
    /// Content to analyze (optional)
    pub content: Option<String>,
    /// Alternative content field
    pub text: Option<String>,
    /// Source type
    pub source_type: Option<String>,
    /// Source ID
    pub source_id: Option<String>,
    /// Reference timestamp (ISO 8601)
    pub t_ref: Option<String>,
    /// Scope
    pub scope: Option<String>,
}

/// Parameters for the `create_task` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Task title
    pub title: String,
    /// Due date (ISO 8601 format, optional)
    pub due_date: Option<String>,
}

/// Parameters for the `send_message_draft` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SendMessageParams {
    /// Recipient
    pub to: Option<String>,
    /// Subject line
    pub subject: Option<String>,
    /// Message body
    pub body: Option<String>,
}

/// Parameters for the `schedule_meeting` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ScheduleMeetingParams {
    /// Meeting title
    pub title: Option<String>,
    /// Start time (ISO 8601 format)
    pub start: String,
    /// End time (ISO 8601 format)
    pub end: String,
}

/// Parameters for the `update_metric` tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UpdateMetricParams {
    /// Metric name
    pub name: Option<String>,
    /// Metric value
    pub value: Option<f64>,
}

/// Parameters for the `resolve_entity` alias tool.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ResolveEntityParams {
    /// Type of entity (e.g., "person", "project", "company")
    pub entity_type: String,
    /// Canonical name for the entity
    pub canonical_name: String,
    /// Known aliases (comma-separated or JSON array string)
    #[serde(default)]
    pub aliases: String,
}

/// Empty parameters for UI tools.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct UiParams {
    #[serde(rename = "_")]
    marker: Option<serde_json::Value>,
}
