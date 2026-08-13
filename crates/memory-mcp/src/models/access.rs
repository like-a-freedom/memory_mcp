use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Access control payload for requests, also used as the resolved access context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccessPayload {
    pub allowed_tags: Option<Vec<String>>,
    pub caller_id: Option<String>,
    pub session_vars: Option<serde_json::Value>,
    pub transport: Option<String>,
    pub content_type: Option<String>,
}

impl AccessPayload {
    /// Creates an access context from an optional payload.
    #[must_use]
    pub fn from_payload(payload: Option<Self>) -> Option<Self> {
        payload
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_payload_from_payload_preserves_tags_and_metadata() {
        let payload = AccessPayload {
            allowed_tags: Some(vec!["deal.pipeline".to_string()]),
            caller_id: Some("caller-1".to_string()),
            session_vars: Some(serde_json::json!({"user_id": "u1"})),
            transport: Some("http".to_string()),
            content_type: Some("application/json".to_string()),
        };

        let access = AccessPayload::from_payload(Some(payload)).expect("access context");
        assert_eq!(access.allowed_tags, Some(vec!["deal.pipeline".to_string()]));
        assert_eq!(access.caller_id, Some("caller-1".to_string()));
        assert_eq!(access.transport, Some("http".to_string()));
        assert_eq!(access.content_type, Some("application/json".to_string()));
        assert_eq!(
            access.session_vars,
            Some(serde_json::json!({"user_id": "u1"}))
        );
    }

    #[test]
    fn access_payload_serialization_has_no_scope_fields() {
        let access = AccessPayload {
            allowed_tags: Some(vec!["restricted".to_string()]),
            ..AccessPayload::default()
        };

        let serialized = serde_json::to_value(&access).expect("serialize access payload");
        let object = serialized.as_object().expect("serialized object");
        assert_eq!(
            object.get("allowed_tags"),
            Some(&serde_json::json!(["restricted"]))
        );
        assert!(!object.contains_key("allowed_scopes"));
        assert!(!object.contains_key("cross_scope_allow"));
    }

    #[test]
    fn access_context_from_payload_with_none() {
        assert!(AccessPayload::from_payload(None).is_none());
    }
}
