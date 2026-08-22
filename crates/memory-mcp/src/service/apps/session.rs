//! App-session state store (ADR-0045).
//!
//! Owns session identity, expiry, and payload persistence. Returns
//! [`MemoryError`]s; the MCP layer maps them to protocol errors.

#![cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Utc};
use serde_json::Value;
use tokio::sync::RwLock;

use crate::error::MemoryError;

#[derive(Debug, Clone)]
pub(crate) struct AppSessionState {
    pub(crate) app: String,
    pub(crate) expires_at: Option<DateTime<Utc>>,
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

    pub fn next_session_id(&self) -> String {
        let id = self.counter.fetch_add(1, Ordering::Relaxed);
        format!("ses:{id:04}")
    }

    pub async fn insert(&self, session_id: String, session: AppSessionState) {
        self.sessions.write().await.insert(session_id, session);
    }

    pub async fn get(&self, session_id: &str) -> Result<AppSessionState, MemoryError> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .cloned()
            .ok_or_else(|| unknown_session(session_id))
    }

    pub async fn get_valid(&self, session_id: &str) -> Result<AppSessionState, MemoryError> {
        let session = self.get(session_id).await?;
        if session
            .expires_at
            .is_some_and(|expires_at| expires_at <= Utc::now())
        {
            self.sessions.write().await.remove(session_id);
            return Err(MemoryError::Validation(format!(
                "App session expired: {session_id}"
            )));
        }

        Ok(session)
    }

    pub async fn replace_payload(
        &self,
        session_id: &str,
        payload: Value,
    ) -> Result<AppSessionState, MemoryError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| unknown_session(session_id))?;
        session.payload = payload;
        Ok(session.clone())
    }

    pub async fn remove(&self, session_id: &str) -> Result<AppSessionState, MemoryError> {
        self.sessions
            .write()
            .await
            .remove(session_id)
            .ok_or_else(|| unknown_session(session_id))
    }

    pub async fn purge_expired(&self) -> usize {
        let now = Utc::now();
        let mut sessions = self.sessions.write().await;
        let before = sessions.len();
        sessions.retain(|_, session| session.expires_at.is_none_or(|expires_at| expires_at > now));
        before.saturating_sub(sessions.len())
    }

    /// Creates a session with an enriched payload (meta block with TTL).
    ///
    /// Payload enrichment is protocol-neutral; the MCP layer shapes the
    /// [`crate::mcp::response::OpenAppResult`] envelope on top.
    pub async fn create(
        &self,
        app: &str,
        ttl_seconds: Option<i64>,
        payload: Value,
    ) -> Result<(String, Value), MemoryError> {
        let session_id = self.next_session_id();
        let payload = enrich_session_payload(app, &session_id, ttl_seconds, payload);
        let expires_at = payload
            .get("meta")
            .and_then(|meta| meta.get("expires_at"))
            .and_then(Value::as_str)
            .and_then(|raw| chrono::DateTime::parse_from_rfc3339(raw).ok())
            .map(|value| value.with_timezone(&Utc));
        self.insert(
            session_id.clone(),
            AppSessionState {
                app: app.to_string(),
                expires_at,
                payload: payload.clone(),
            },
        )
        .await;

        Ok((session_id, payload))
    }
}

pub(crate) fn unknown_session(session_id: &str) -> MemoryError {
    MemoryError::NotFound(format!("Unknown or closed app session: {session_id}"))
}

pub(crate) fn enrich_session_payload(
    app: &str,
    session_id: &str,
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
    use chrono::Duration;
    use serde_json::json;

    #[test]
    fn enrich_session_payload_adds_meta_with_expiry() {
        let payload = json!({"data": "value"});
        let enriched = enrich_session_payload("inspector", "ses:1", Some(3600), payload);
        assert_eq!(enriched["app"], "inspector");
        assert_eq!(enriched["session_id"], "ses:1");

        assert!(enriched["meta"]["expires_at"].is_string());
        assert_eq!(enriched["meta"]["ttl_seconds"], 3600);
        assert_eq!(enriched["data"], "value");
    }

    #[test]
    fn enrich_session_payload_handles_no_ttl() {
        let payload = json!({});
        let enriched = enrich_session_payload("diff", "ses:2", None, payload);
        assert_eq!(enriched["app"], "diff");
        assert_eq!(enriched["meta"]["ttl_seconds"], serde_json::Value::Null);
        assert!(enriched["meta"]["expires_at"].is_null());
    }

    #[tokio::test]
    async fn get_valid_rejects_expired_session() {
        let manager = SessionManager::new();
        manager
            .insert(
                "ses:9999".to_string(),
                AppSessionState {
                    app: "diff".to_string(),
                    expires_at: Some(Utc::now() - Duration::seconds(1)),
                    payload: json!({"app": "diff"}),
                },
            )
            .await;

        let err = manager
            .get_valid("ses:9999")
            .await
            .expect_err("expired session should be rejected");
        assert!(err.to_string().contains("expired"));
        assert!(
            manager.get("ses:9999").await.is_err(),
            "expired session should be purged"
        );
    }

    #[tokio::test]
    async fn purge_expired_removes_only_expired_sessions() {
        let manager = SessionManager::new();
        manager
            .insert(
                "ses:expired".to_string(),
                AppSessionState {
                    app: "diff".to_string(),
                    expires_at: Some(Utc::now() - Duration::seconds(1)),
                    payload: json!({}),
                },
            )
            .await;
        manager
            .insert(
                "ses:live".to_string(),
                AppSessionState {
                    app: "diff".to_string(),
                    expires_at: Some(Utc::now() + Duration::seconds(30)),
                    payload: json!({}),
                },
            )
            .await;

        assert_eq!(manager.purge_expired().await, 1);
        assert!(manager.get("ses:expired").await.is_err());
        assert!(manager.get("ses:live").await.is_ok());
    }
}
