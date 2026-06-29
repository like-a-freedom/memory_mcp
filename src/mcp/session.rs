use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use rmcp::ErrorData;
use tokio::sync::RwLock;
use serde_json::Value;

use crate::models::AccessPayload;
use super::resources::app_session_uri;

use super::response::OpenAppResult;

#[derive(Debug, Clone)]
pub(crate) struct AppSessionState {
    pub(crate) app: String,
    pub(crate) scope: String,
    pub(crate) payload: Value,
}

#[derive(Clone)]
pub(crate) struct SessionManager {
    sessions: Arc<RwLock<std::collections::HashMap<String, AppSessionState>>>,
    counter: Arc<AtomicU64>,
}

impl SessionManager {
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(std::collections::HashMap::new())),
            counter: Arc::new(AtomicU64::new(1)),
        }
    }

    pub fn clone_inner(&self) -> Self {
        Self {
            sessions: self.sessions.clone(),
            counter: self.counter.clone(),
        }
    }

    pub fn next_session_id(&self) -> String {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("ses:{id:04}")
    }

    pub async fn insert(&self, session_id: String, session: AppSessionState) {
        self.sessions.write().await.insert(session_id, session);
    }

    pub async fn get(&self, session_id: &str) -> Result<AppSessionState, ErrorData> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| {
                invalid_params(format!("Unknown or closed app session: {session_id}"))
            })
    }

    pub async fn replace_payload(
        &self,
        session_id: &str,
        payload: Value,
    ) -> Result<AppSessionState, ErrorData> {
        let mut sessions = self.sessions.write().await;
        let session = sessions.get_mut(session_id).ok_or_else(|| {
            invalid_params(format!("Unknown or closed app session: {session_id}"))
        })?;
        session.payload = payload;
        Ok(session.clone())
    }

    pub async fn remove(&self, session_id: &str) -> Result<AppSessionState, ErrorData> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| {
                invalid_params(format!("Unknown or closed app session: {session_id}"))
            })
    }

    pub async fn create(
        &self,
        app: &str,
        scope: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Result<OpenAppResult, ErrorData> {
        let session_id = self.next_session_id();
        let payload = enrich_session_payload(app, &session_id, scope, ttl_seconds, payload);
        self.insert(
            session_id.clone(),
            AppSessionState {
                app: app.to_string(),
                scope: scope.to_string(),
                payload: payload.clone(),
            },
        )
        .await;

        Ok(open_app_result(app, session_id, payload))
    }
}

pub(crate) fn invalid_params(message: impl Into<String>) -> ErrorData {
    let msg = message.into();
    let data = serde_json::json!({
        "guidance": "Review the input arguments, fix any issues, and retry.",
    });
    ErrorData::invalid_params(msg, Some(data))
}

pub(crate) fn missing_app_field(app: &str, field: &str) -> ErrorData {
    let msg = format!("`{field}` is required for {app}");
    let data = serde_json::json!({
        "guidance": format!("Supply the `{field}` parameter and retry."),
    });
    ErrorData::invalid_params(msg, Some(data))
}

pub(crate) fn internal_error(message: impl Into<String>) -> ErrorData {
    let msg = message.into();
    let data = serde_json::json!({
        "guidance": "This is a transient error. Retry the operation.",
    });
    ErrorData::internal_error(msg, Some(data))
}

pub(crate) fn open_app_result(app: &str, session_id: impl Into<String>, fallback: Value) -> OpenAppResult {
    let session_id = session_id.into();
    OpenAppResult {
        app: app.to_string(),
        resource_uri: app_session_uri(app, &session_id),
        session_id,
        fallback,
    }
}

pub(crate) fn app_command_result_from_details(
    app: &str,
    session_id: &str,
    action: &str,
    resource_uri: Option<String>,
    details: Value,
) -> super::response::AppCommandResult {
    let ok = details
        .get("ok")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let message = details
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("done")
        .to_string();
    let refresh_required = details
        .get("refresh_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    super::response::AppCommandResult {
        app: app.to_string(),
        session_id: session_id.to_string(),
        action: action.to_string(),
        ok,
        message,
        refresh_required,
        resource_uri,
        details: Some(details),
    }
}

pub(crate) fn enrich_session_payload(
    app: &str,
    session_id: &str,
    scope: &str,
    ttl_seconds: Option<i64>,
    payload: Value,
) -> Value {
    use chrono::Utc;

    let created_at = Utc::now();
    let expires_at = ttl_seconds
        .filter(|ttl| *ttl > 0)
        .map(|ttl| created_at + chrono::Duration::seconds(ttl));

    let mut object = payload
        .as_object()
        .cloned()
        .unwrap_or_else(serde_json::Map::new);
    object.insert("app".to_string(), serde_json::json!(app));
    object.insert("session_id".to_string(), serde_json::json!(session_id));
    object.insert("scope".to_string(), serde_json::json!(scope));
    object.insert(
        "meta".to_string(),
        serde_json::json!({
            "created_at": created_at.to_rfc3339(),
            "ttl_seconds": ttl_seconds,
            "expires_at": expires_at.map(|value| value.to_rfc3339()),
        }),
    );
    Value::Object(object)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enrich_session_payload_adds_meta_with_expiry() {
        let payload = json!({"data": "value"});
        let enriched = enrich_session_payload("inspector", "ses:1", "org", Some(3600), payload);
        assert_eq!(enriched["app"], "inspector");
        assert_eq!(enriched["session_id"], "ses:1");
        assert_eq!(enriched["scope"], "org");
        assert!(enriched["meta"]["expires_at"].is_string());
        assert_eq!(enriched["meta"]["ttl_seconds"], 3600);
        assert_eq!(enriched["data"], "value");
    }

    #[test]
    fn enrich_session_payload_handles_no_ttl() {
        let payload = json!({});
        let enriched = enrich_session_payload("diff", "ses:2", "personal", None, payload);
        assert_eq!(enriched["app"], "diff");
        assert_eq!(enriched["meta"]["ttl_seconds"], serde_json::Value::Null);
        assert!(enriched["meta"]["expires_at"].is_null());
    }
}
