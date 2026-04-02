//! Data models and types for the Memory MCP system.
//!
//! This module defines the core data structures used throughout the application,
//! including request/response types, domain entities, and access control types.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Unique identifier for an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EpisodeId(pub String);

impl From<String> for EpisodeId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for EpisodeId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl std::fmt::Display for EpisodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for an entity.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EntityId(pub String);

impl From<String> for EntityId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for EntityId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a fact.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct FactId(pub String);

impl From<String> for FactId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for FactId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

impl std::fmt::Display for FactId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique identifier for a community.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct CommunityId(pub String);

/// Unique identifier for an edge.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct EdgeId(pub String);

/// Request to ingest a new episode into memory.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestRequest {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    #[serde(default = "default_scope")]
    pub scope: String,
    pub t_ingested: Option<DateTime<Utc>>,
    pub visibility_scope: Option<String>,
    #[serde(default)]
    pub policy_tags: Vec<String>,
}

/// Input for creating an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EpisodeInput {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    pub scope: String,
    pub uri: Option<String>,
}

/// Request to explain context items with source citations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
}

/// A single item to explain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
        }
    }
}

/// A single provenance source for a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProvenanceSource {
    /// Source episode ID.
    pub episode_id: String,
    /// Source episode content (excerpt).
    pub episode_content: String,
    /// Source episode timestamp.
    pub episode_t_ref: String,
    /// Relationship to fact: "direct" (created fact) or "linked" (via entity).
    pub relationship: String,
    /// Entity link path (if relationship is "linked").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entity_path: Option<String>,
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
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default = "default_budget")]
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
pub struct ExtractResult {
    pub episode_id: String,
    pub entities: Vec<ExtractedEntity>,
    pub facts: Vec<ExtractedFact>,
    pub links: Vec<ExtractedLink>,
}

impl ExtractResult {
    /// Returns an empty extraction result for partial or no-input responses.
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }
}

/// A ranked context item returned by the MCP `assemble_context` tool.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AssembledContextItem {
    pub fact_id: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    pub confidence: f64,
    pub provenance: serde_json::Value,
    pub rationale: String,
    /// Temporal anchor of the fact — ISO 8601. Use this to compute date differences.
    #[serde(default)]
    pub t_ref: Option<DateTime<Utc>>,
    /// Validity interval start — ISO 8601. May differ from t_ref for retconned facts.
    #[serde(default)]
    pub t_valid: Option<DateTime<Utc>>,
}

/// Defines allowed scope transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessScopeAllow {
    pub from: String,
    pub to: String,
}

/// Access control payload for requests.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AccessPayload {
    pub allowed_scopes: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    pub caller_id: Option<String>,
    pub session_vars: Option<serde_json::Value>,
    pub transport: Option<String>,
    pub content_type: Option<String>,
    pub cross_scope_allow: Option<Vec<AccessScopeAllow>>,
}

/// Resolved access context derived from payload.
#[derive(Debug, Clone, Default)]
pub struct AccessContext {
    pub allowed_scopes: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    pub caller_id: Option<String>,
    pub session_vars: Option<serde_json::Value>,
    pub transport: Option<String>,
    pub content_type: Option<String>,
    pub cross_scope_allow: Option<Vec<AccessScopeAllow>>,
}

impl AccessContext {
    /// Creates an access context from an optional payload.
    #[must_use]
    pub fn from_payload(payload: Option<AccessPayload>) -> Option<Self> {
        payload.map(|access| Self {
            allowed_scopes: access.allowed_scopes,
            allowed_tags: access.allowed_tags,
            caller_id: access.caller_id,
            session_vars: access.session_vars,
            transport: access.transport,
            content_type: access.content_type,
            cross_scope_allow: access.cross_scope_allow,
        })
    }

    /// Checks if a scope is allowed.
    #[must_use]
    pub fn is_scope_allowed(&self, scope: &str) -> bool {
        if let Some(scopes) = &self.allowed_scopes
            && !scopes.contains(&scope.to_string())
        {
            return self.cross_scope_allow.as_ref().is_some_and(|cross| {
                cross
                    .iter()
                    .any(|pair| pair.from == "*" && pair.to == scope)
            });
        }
        true
    }
}

/// An episode represents a unit of ingested content.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Episode {
    pub episode_id: String,
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub scope: String,
    pub visibility_scope: String,
    pub policy_tags: Vec<String>,
}

/// An entity represents a canonical named thing.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
}

/// Deserializes entity_links from either string IDs or SurrealDB record-ref JSON shapes.
///
/// SurrealDB may serialize typed record references in several ways when returning
/// JSON: plain strings, `{"String": "entity:atlas"}`, `{"tb": "entity", "id": "atlas"}`,
/// or `{"id": "entity:atlas"}`. This deserializer handles all forms.
fn deserialize_entity_links<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match value {
            serde_json::Value::String(s) => Some(s),
            serde_json::Value::Object(map) => map
                .get("String")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .or_else(|| {
                    map.get("tb")
                        .and_then(serde_json::Value::as_str)
                        .zip(map.get("id").and_then(serde_json::Value::as_str))
                        .map(|(tb, id)| format!("{tb}:{id}"))
                })
                .or_else(|| {
                    map.get("id")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_string)
                }),
            _ => None,
        })
        .collect())
}

/// A fact represents a piece of knowledge extracted from an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    pub fact_id: String,
    pub fact_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
    /// Reference timestamp — when the source episode was anchored.
    /// For facts created before this field was added, falls back to t_valid at deserialization.
    #[serde(default)]
    pub t_ref: Option<DateTime<Utc>>,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
    pub t_invalid_ingested: Option<DateTime<Utc>>,
    pub confidence: f64,
    #[serde(default)]
    pub index_keys: Vec<String>,
    #[serde(default)]
    pub access_count: i64,
    #[serde(default)]
    pub last_accessed: Option<DateTime<Utc>>,
    #[serde(default, deserialize_with = "deserialize_entity_links")]
    pub entity_links: Vec<String>,
    pub scope: String,
    pub policy_tags: Vec<String>,
    pub provenance: serde_json::Value,
    /// Full-text search relevance score (only present for FTS results).
    pub ft_score: f64,
}

/// An edge represents a relationship between entities or facts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    #[serde(rename = "in")]
    pub in_id: String,
    pub relation: String,
    #[serde(rename = "out")]
    pub out_id: String,
    pub strength: f64,
    pub confidence: f64,
    pub provenance: serde_json::Value,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
    pub t_invalid_ingested: Option<DateTime<Utc>>,
}

/// A community groups related entities.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Community {
    pub community_id: String,
    pub member_entities: Vec<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

// --- MCP Apps models (§2.4, §4.2, §5.2, §6.2) ---

/// Server-side session for MCP Apps (FR-COM-07, §2.4).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AppSession {
    pub session_id: String,
    pub app_id: String,
    pub scope: String,
    pub access: serde_json::Value,
    pub target: serde_json::Value,
    pub state: String,
    pub created_at: DateTime<Utc>,
    pub last_active: DateTime<Utc>,
    pub ttl_seconds: i64,
}

/// Result of an App action (§2.4).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AppActionResult {
    pub ok: bool,
    pub message: String,
    pub refresh_required: bool,
    pub updated_targets: Vec<String>,
    pub task_id: Option<String>,
}

/// App error for clients (§2.4).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AppError {
    pub code: String,
    pub user_message: String,
    pub debug_hint: String,
}

/// Draft ingestion record (§5.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DraftIngestion {
    pub draft_id: String,
    pub scope: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub access_ctx: serde_json::Value,
}

/// Draft item record (§5.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DraftItem {
    pub draft_id: String,
    pub item_id: String,
    pub item_type: String,
    pub status: String,
    pub payload: serde_json::Value,
    pub original_payload: Option<serde_json::Value>,
    pub confidence: f64,
    pub rationale: Option<String>,
    pub source_snippet: Option<String>,
}

/// Lifecycle filters (§6.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct LifecycleFilters {
    pub min_confidence: Option<f64>,
    pub max_confidence: Option<f64>,
    pub inactive_days: Option<i32>,
    pub include_archived: Option<bool>,
}

/// Diff filters (§4.2).
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DiffFilters {
    pub only_facts: Option<bool>,
    pub only_edges: Option<bool>,
    pub only_active: Option<bool>,
    pub only_policy_visible: Option<bool>,
}

#[must_use]
pub fn default_scope() -> String {
    "org".to_string()
}

#[must_use]
pub fn default_budget() -> i32 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_context_from_payload_maps_fields() {
        let payload = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string(), "personal".to_string()]),
            allowed_tags: Some(vec!["deal.pipeline".to_string()]),
            caller_id: Some("caller-1".to_string()),
            session_vars: Some(serde_json::json!({"user_id": "u1"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };

        let access = AccessContext::from_payload(Some(payload)).expect("access context");
        assert_eq!(
            access.allowed_scopes,
            Some(vec!["org".to_string(), "personal".to_string()])
        );
        assert_eq!(access.allowed_tags, Some(vec!["deal.pipeline".to_string()]));
        assert_eq!(access.caller_id, Some("caller-1".to_string()));
        assert_eq!(access.transport, Some("http".to_string()));
        assert_eq!(access.content_type, Some("application/json".to_string()));
        assert_eq!(
            access.cross_scope_allow,
            Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }])
        );
        assert_eq!(
            access.session_vars,
            Some(serde_json::json!({"user_id": "u1"}))
        );
    }

    #[test]
    fn episode_id_from_str() {
        let id = EpisodeId::from("episode:abc123");
        assert_eq!(id.0, "episode:abc123");
    }

    #[test]
    fn episode_id_display() {
        let id = EpisodeId::from("episode:abc123");
        assert_eq!(format!("{id}"), "episode:abc123");
    }

    #[test]
    fn access_context_is_scope_allowed_with_explicit_scope() {
        let access = AccessContext {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(access.is_scope_allowed("org"));
        assert!(!access.is_scope_allowed("personal"));
    }

    #[test]
    fn access_context_is_scope_allowed_with_cross_scope() {
        let access = AccessContext {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "personal".to_string(),
            }]),
        };
        assert!(access.is_scope_allowed("org"));
        assert!(access.is_scope_allowed("personal"));
    }

    #[test]
    fn access_context_is_scope_allowed_when_none() {
        let access = AccessContext::default();
        assert!(access.is_scope_allowed("any_scope"));
    }

    #[test]
    fn episode_id_clone() {
        let id1 = EpisodeId::from("episode:test123");
        let id2 = id1.clone();
        assert_eq!(id1.0, id2.0);
    }

    #[test]
    fn entity_id_clone() {
        let id1 = EntityId::from("entity:alice");
        let id2 = id1.clone();
        assert_eq!(id1.0, id2.0);
    }

    #[test]
    fn fact_id_clone() {
        let id1 = FactId::from("fact:abc123");
        let id2 = id1.clone();
        assert_eq!(id1.0, id2.0);
    }

    #[test]
    fn access_context_from_payload_with_none() {
        let result = AccessContext::from_payload(None);
        assert!(result.is_none());
    }

    #[test]
    fn access_context_from_payload_maps_all_fields() {
        use serde_json::json;
        let payload = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            allowed_tags: Some(vec!["tag1".to_string()]),
            caller_id: Some("user123".to_string()),
            session_vars: Some(json!({"key": "value"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };

        let context = AccessContext::from_payload(Some(payload)).unwrap();
        assert_eq!(context.allowed_scopes, Some(vec!["org".to_string()]));
        assert_eq!(context.allowed_tags, Some(vec!["tag1".to_string()]));
        assert_eq!(context.caller_id, Some("user123".to_string()));
        assert_eq!(context.transport, Some("http".to_string()));
        assert_eq!(context.content_type, Some("application/json".to_string()));
    }

    #[test]
    fn access_context_is_scope_allowed_with_allowed_list() {
        let access = AccessContext {
            allowed_scopes: Some(vec!["org".to_string(), "personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: None,
        };
        assert!(access.is_scope_allowed("org"));
        assert!(access.is_scope_allowed("personal"));
        assert!(!access.is_scope_allowed("private"));
    }

    #[test]
    fn access_context_is_scope_allowed_with_wildcard_cross_scope() {
        let access = AccessContext {
            allowed_scopes: Some(vec!["personal".to_string()]),
            allowed_tags: None,
            caller_id: None,
            session_vars: None,
            transport: None,
            content_type: None,
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
        };
        assert!(access.is_scope_allowed("personal"));
        assert!(access.is_scope_allowed("org"));
        assert!(!access.is_scope_allowed("private"));
    }

    #[test]
    fn default_scope_returns_org() {
        assert_eq!(default_scope(), "org");
    }

    #[test]
    fn default_budget_returns_5() {
        assert_eq!(default_budget(), 5);
    }
}
