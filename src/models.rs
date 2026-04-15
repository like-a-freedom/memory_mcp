//! Data models and types for the Memory MCP system.
//!
//! This module defines the core data structures used throughout the application,
//! including request/response types, domain entities, and access control types.

use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! define_id_type {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq, Eq, Hash)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }

        impl std::borrow::Borrow<str> for $name {
            fn borrow(&self) -> &str {
                &self.0
            }
        }
    };
}

define_id_type!(EpisodeId, "Unique identifier for an episode.");
define_id_type!(EntityId, "Unique identifier for an entity.");
define_id_type!(FactId, "Unique identifier for a fact.");
define_id_type!(CommunityId, "Unique identifier for a community.");
define_id_type!(EdgeId, "Unique identifier for an edge.");

/// Request to ingest a new episode into memory.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct IngestRequest {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    #[serde(default = "default_scope")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
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
    pub project: Option<String>,
    pub uri: Option<String>,
}

/// Request to explain context items with source citations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
}

/// A single item to explain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
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
        }
    }
}

/// A single provenance source for a fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
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

/// Standard fact type classification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FactType {
    Note,
    Decision,
    Metric,
    Promise,
    Experience,
}

impl FactType {
    /// All standard fact types.
    pub const ALL: &'static [Self] = &[
        Self::Note,
        Self::Decision,
        Self::Metric,
        Self::Promise,
        Self::Experience,
    ];

    /// Returns the string representation for database storage.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Note => "note",
            Self::Decision => "decision",
            Self::Metric => "metric",
            Self::Promise => "promise",
            Self::Experience => "experience",
        }
    }
}

impl std::fmt::Display for FactType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
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

/// Resolved access context derived from a payload.
///
/// Shares the same fields as `AccessPayload`; this type exists solely to carry
/// the `is_scope_allowed` behaviour and a `Default` impl.
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

impl From<AccessPayload> for AccessContext {
    fn from(p: AccessPayload) -> Self {
        Self {
            allowed_scopes: p.allowed_scopes,
            allowed_tags: p.allowed_tags,
            caller_id: p.caller_id,
            session_vars: p.session_vars,
            transport: p.transport,
            content_type: p.content_type,
            cross_scope_allow: p.cross_scope_allow,
        }
    }
}

impl AccessContext {
    /// Creates an access context from an optional payload.
    #[must_use]
    pub fn from_payload(payload: Option<AccessPayload>) -> Option<Self> {
        payload.map(Self::from)
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

/// A fact represents a piece of knowledge extracted from an episode.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Fact {
    pub fact_id: String,
    pub fact_type: String,
    pub content: String,
    pub quote: String,
    pub source_episode: String,
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
    pub entity_links: Vec<String>,
    pub scope: String,
    pub policy_tags: Vec<String>,
    pub provenance: serde_json::Value,
    /// Full-text search relevance score (only present for FTS results).
    pub ft_score: f64,
}

/// Origin of an edge (relationship between entities or facts).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EdgeOrigin {
    #[default]
    Extracted,
    Inferred,
    Ambiguous,
}

/// An edge represents a relationship between entities or facts.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    #[serde(rename = "in")]
    pub in_id: String,
    pub relation: String,
    #[serde(rename = "out")]
    pub out_id: String,
    #[serde(default)]
    pub origin: EdgeOrigin,
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
