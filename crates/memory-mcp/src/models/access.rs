use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Defines allowed scope transitions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AccessScopeAllow {
    pub from: String,
    pub to: String,
}

/// Access control payload for requests, also used as the resolved access context.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct AccessPayload {
    pub allowed_scopes: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    pub caller_id: Option<String>,
    pub session_vars: Option<serde_json::Value>,
    pub transport: Option<String>,
    pub content_type: Option<String>,
    pub cross_scope_allow: Option<Vec<AccessScopeAllow>>,
}

impl AccessPayload {
    /// Creates an access context from an optional payload.
    #[must_use]
    pub fn from_payload(payload: Option<Self>) -> Option<Self> {
        payload
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_scope_allowed_returns_true_when_no_restrictions() {
        let access = AccessPayload::default();
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_true_for_allowed_scope() {
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string()]),
            ..AccessPayload::default()
        };
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_returns_false_for_disallowed_scope() {
        let access = AccessPayload {
            allowed_scopes: Some(vec!["personal".to_string()]),
            ..AccessPayload::default()
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_allows_with_cross_scope_wildcard() {
        let access = AccessPayload {
            allowed_scopes: Some(vec!["personal".to_string()]),
            cross_scope_allow: Some(vec![AccessScopeAllow {
                from: "*".to_string(),
                to: "org".to_string(),
            }]),
            ..AccessPayload::default()
        };
        assert!(access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_with_empty_allowed_scopes() {
        let access = AccessPayload {
            allowed_scopes: Some(vec![]),
            ..Default::default()
        };
        assert!(!access.is_scope_allowed("org"));
    }

    #[test]
    fn is_scope_allowed_with_multiple_allowed_scopes() {
        let access = AccessPayload {
            allowed_scopes: Some(vec!["org".to_string(), "personal".to_string()]),
            ..Default::default()
        };
        assert!(access.is_scope_allowed("org"));
        assert!(access.is_scope_allowed("personal"));
        assert!(!access.is_scope_allowed("private"));
    }

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

        let access = AccessPayload::from_payload(Some(payload)).expect("access context");
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
    fn access_context_is_scope_allowed_with_explicit_scope() {
        let access = AccessPayload {
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
        let access = AccessPayload {
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
        let access = AccessPayload::default();
        assert!(access.is_scope_allowed("any_scope"));
    }

    #[test]
    fn access_context_from_payload_with_none() {
        let result = AccessPayload::from_payload(None);
        assert!(result.is_none());
    }

    #[test]
    fn access_context_is_scope_allowed_with_allowed_list() {
        let access = AccessPayload {
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
        let access = AccessPayload {
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
}
