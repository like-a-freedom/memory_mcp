use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EpisodeInput {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: DateTime<Utc>,
    pub scope: String,
    pub uri: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainRequest {
    pub context_pack: Vec<ExplainItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExplainItem {
    pub content: String,
    pub quote: String,
    pub source_episode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ExtractRequest {
    pub episode_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EntityCandidate {
    pub entity_type: String,
    pub canonical_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct InvalidateRequest {
    pub fact_id: String,
    pub reason: String,
    pub t_invalid: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct AssembleContextRequest {
    pub query: String,
    pub scope: String,
    pub as_of: Option<DateTime<Utc>>,
    #[serde(default = "default_budget")]
    pub budget: i32,
    #[serde(skip_serializing, default)]
    #[schemars(skip)]
    pub access: Option<AccessPayload>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessScopeAllow {
    pub from: String,
    pub to: String,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Entity {
    pub entity_id: String,
    pub entity_type: String,
    pub canonical_name: String,
    pub aliases: Vec<String>,
}

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
    pub entity_links: Vec<String>,
    pub scope: String,
    pub policy_tags: Vec<String>,
    pub provenance: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Edge {
    pub from_id: String,
    pub relation: String,
    pub to_id: String,
    pub strength: f64,
    pub confidence: f64,
    pub provenance: serde_json::Value,
    pub t_valid: DateTime<Utc>,
    pub t_ingested: DateTime<Utc>,
    pub t_invalid: Option<DateTime<Utc>>,
    pub t_invalid_ingested: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct Community {
    pub community_id: String,
    pub member_entities: Vec<String>,
    pub summary: String,
    pub updated_at: DateTime<Utc>,
}

pub fn default_scope() -> String {
    "personal".to_string()
}

pub fn default_budget() -> i32 {
    5
}

impl AccessContext {
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
}
