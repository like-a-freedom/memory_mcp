use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::access::AccessPayload;
use super::provenance::ProvenanceSource;

/// Request to ingest a new episode into memory.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestRequest {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    #[serde(default = "super::default_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    pub t_ingested: Option<DateTime<Utc>>,
    pub visibility_scope: Option<String>,
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Request to explain context items with source citations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
}

/// A single item to explain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExplainItem {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_id: Option<String>,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ref: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub t_ingested: Option<DateTime<Utc>>,
    #[serde(default)]
    pub provenance: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub citation_context: Option<String>,
    /// All provenance sources for this fact (direct + linked episodes).
    #[serde(default)]
    pub all_sources: Vec<ProvenanceSource>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph_insights: Option<GraphInsights>,
    /// Age of the fact in days (computed from t_valid vs now).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fact_age_days: Option<i64>,
    /// Confidence after applying time-based decay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decayed_confidence: Option<f64>,
    /// How this fact entered the memory system (e.g. "manual", "extraction").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingestion_method: Option<String>,
}

impl Default for ExplainItem {
    fn default() -> Self {
        Self {
            fact_id: None,
            content: String::new(),
            quote: String::new(),
            source_episode: String::new(),
            scope: None,
            t_ref: None,
            t_ingested: None,
            provenance: serde_json::Value::Null,
            citation_context: None,
            all_sources: Vec::new(),
            graph_insights: None,
            fact_age_days: None,
            decayed_confidence: None,
            ingestion_method: None,
        }
    }
}

/// Ranked hub entities and cross-community paths relevant to an explained fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GraphInsights {
    #[serde(default)]
    pub hub_entities: Vec<GraphHubEntity>,
    #[serde(default)]
    pub surprising_connections: Vec<SurprisingConnection>,
}

/// A high-degree entity in the current graph neighborhood.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct GraphHubEntity {
    pub entity_id: String,
    pub canonical_name: String,
    pub degree: usize,
}

/// A short cross-community path that may reveal a non-obvious connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct SurprisingConnection {
    pub source_entity_id: String,
    pub source_entity_name: String,
    pub target_entity_id: String,
    pub target_entity_name: String,
    pub hop_count: usize,
    #[serde(default)]
    pub path: Vec<String>,
}

/// Request to extract entities and facts from an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractRequest {
    pub episode_id: String,
}

/// Entity candidate for resolution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityCandidate {
    pub entity_type: String,
    pub canonical_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

/// Request to invalidate a fact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidateRequest {
    pub fact_id: String,
    pub reason: String,
    pub t_invalid: DateTime<Utc>,
}

/// Request to assemble context for a query.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssembleContextRequest {
    pub query: String,
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub fact_types: Vec<String>,
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default = "super::default_budget")]
    pub budget: i32,
    #[serde(default)]
    pub view_mode: Option<String>,
    #[serde(default)]
    pub window_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub window_end: Option<DateTime<Utc>>,
    #[serde(skip_serializing, default)]
    #[schemars(skip)]
    pub access: Option<AccessPayload>,
}

/// A compact extracted entity returned by the MCP `extract` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedEntity {
    pub entity_id: String,
    #[serde(rename = "type")]
    pub entity_type: String,
    pub canonical_name: String,
}

/// A compact extracted fact returned by the MCP `extract` tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedFact {
    pub fact_id: String,
    #[serde(rename = "type")]
    pub fact_type: String,
}

/// A relationship link produced during extraction.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExtractedLink {
    pub entity_id: String,
    pub episode_id: String,
}

/// Structured result returned by the MCP `extract` tool.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ExtractResult {
    pub episode_id: String,
    pub entities: Vec<ExtractedEntity>,
    pub facts: Vec<ExtractedFact>,
    pub links: Vec<ExtractedLink>,
    #[serde(default)]
    pub warnings: Vec<ContradictionWarning>,
}

impl ExtractResult {
    /// Returns an empty extraction result for partial or no-input responses.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

/// A non-blocking warning about a newly extracted fact that may contradict an active fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ContradictionWarning {
    pub fact_type: String,
    pub new_fact_id: String,
    pub conflicting_fact_id: String,
    pub existing_content: String,
    pub new_content: String,
    #[serde(default)]
    pub entity_ids: Vec<String>,
    pub reason: String,
}

/// A ranked context item returned by the MCP `assemble_context` tool.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct AssembledContextItem {
    pub fact_id: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub confidence: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub relevance: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grounding: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub semantic_available: Option<bool>,
    pub provenance: serde_json::Value,
    pub rationale: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retrieval_tier: Option<String>,
}
