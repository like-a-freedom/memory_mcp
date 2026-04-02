use chrono::Utc;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::models::AppSession;
use crate::service::error::MemoryError;

const DEFAULT_SESSION_TTL_SECS: i64 = 3600;
const DEFAULT_SESSION_LIMIT: usize = 10;
const SESSION_CLEANUP_INTERVAL_SECS: u64 = 300;

/// Manages in-memory AppSession lifecycle (FR-COM-06, FR-COM-07).
#[derive(Clone)]
pub struct AppSessionManager {
    sessions: Arc<RwLock<HashMap<String, AppSession>>>,
    max_sessions_per_scope: usize,
    default_ttl_seconds: i64,
}

impl AppSessionManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_sessions_per_scope: DEFAULT_SESSION_LIMIT,
            default_ttl_seconds: DEFAULT_SESSION_TTL_SECS,
        }
    }

    #[must_use]
    pub fn with_config(max_sessions: usize, ttl_seconds: i64) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            max_sessions_per_scope: max_sessions,
            default_ttl_seconds: ttl_seconds,
        }
    }

    pub fn spawn_cleanup_task(&self) -> tokio::task::JoinHandle<()> {
        let sessions = self.sessions.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(
                SESSION_CLEANUP_INTERVAL_SECS,
            ));
            loop {
                interval.tick().await;
                let mut map = sessions.write().await;
                let now = Utc::now();
                map.retain(|_, session| {
                    let elapsed = now.signed_duration_since(session.last_active).num_seconds();
                    elapsed <= session.ttl_seconds
                });
            }
        })
    }

    fn generate_session_id() -> String {
        format!("ses:{}", Uuid::new_v4())
    }

    pub async fn create_session(
        &self,
        app_id: &str,
        scope: &str,
        access: serde_json::Value,
        target: serde_json::Value,
        ttl_seconds: Option<i64>,
    ) -> Result<AppSession, MemoryError> {
        let ttl = ttl_seconds.unwrap_or(self.default_ttl_seconds);
        let now = Utc::now();
        let session_id = Self::generate_session_id();

        let session = AppSession {
            session_id: session_id.clone(),
            app_id: app_id.to_string(),
            scope: scope.to_string(),
            access,
            target,
            state: "loading".to_string(),
            created_at: now,
            last_active: now,
            ttl_seconds: ttl,
        };

        let mut sessions = self.sessions.write().await;
        let scope_count = sessions.values().filter(|s| s.scope == scope).count();
        if scope_count >= self.max_sessions_per_scope {
            return Err(MemoryError::SessionLimitExceeded);
        }

        sessions.insert(session_id, session.clone());
        Ok(session)
    }

    pub async fn get_session(&self, session_id: &str) -> Result<AppSession, MemoryError> {
        let sessions = self.sessions.read().await;
        let session = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| MemoryError::SessionNotFound(session_id.to_string()))?;

        let now = Utc::now();
        let elapsed = now.signed_duration_since(session.last_active).num_seconds();
        if elapsed > session.ttl_seconds {
            return Err(MemoryError::SessionExpired(session_id.to_string()));
        }

        Ok(session)
    }

    pub async fn touch_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| MemoryError::SessionNotFound(session_id.to_string()))?;
        session.last_active = Utc::now();
        Ok(())
    }

    pub async fn update_session_state(
        &self,
        session_id: &str,
        state: &str,
        target: serde_json::Value,
    ) -> Result<(), MemoryError> {
        let mut sessions = self.sessions.write().await;
        let session = sessions
            .get_mut(session_id)
            .ok_or_else(|| MemoryError::SessionNotFound(session_id.to_string()))?;
        session.state = state.to_string();
        session.target = target;
        session.last_active = Utc::now();
        Ok(())
    }

    pub async fn close_session(&self, session_id: &str) -> Result<(), MemoryError> {
        let mut sessions = self.sessions.write().await;
        sessions
            .remove(session_id)
            .ok_or_else(|| MemoryError::SessionNotFound(session_id.to_string()))?;
        Ok(())
    }

    pub async fn list_sessions(&self, scope: &str) -> Vec<AppSession> {
        let sessions = self.sessions.read().await;
        sessions
            .values()
            .filter(|s| s.scope == scope)
            .cloned()
            .collect()
    }
}

impl Default for AppSessionManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn create_and_get_session() {
        let manager = AppSessionManager::new();
        let session = manager
            .create_session("inspector", "org", json!({}), json!({}), Some(3600))
            .await
            .expect("create session");
        let retrieved = manager
            .get_session(&session.session_id)
            .await
            .expect("get session");
        assert_eq!(retrieved.session_id, session.session_id);
        assert_eq!(retrieved.app_id, "inspector");
    }

    #[tokio::test]
    async fn session_expired_returns_error() {
        let manager = AppSessionManager::new();
        let session = manager
            .create_session("inspector", "org", json!({}), json!({}), Some(1))
            .await
            .expect("create session");
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        let result = manager.get_session(&session.session_id).await;
        assert!(matches!(result, Err(MemoryError::SessionExpired(_))));
    }

    #[tokio::test]
    async fn close_session_returns_not_found() {
        let manager = AppSessionManager::new();
        let session = manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await
            .expect("create session");
        manager
            .close_session(&session.session_id)
            .await
            .expect("close session");
        let result = manager.get_session(&session.session_id).await;
        assert!(matches!(result, Err(MemoryError::SessionNotFound(_))));
    }

    #[tokio::test]
    async fn session_limit_per_scope() {
        let manager = AppSessionManager::with_config(2, 3600);
        manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await
            .expect("first session");
        manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await
            .expect("second session");
        let result = manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await;
        assert!(matches!(result, Err(MemoryError::SessionLimitExceeded)));
    }

    #[tokio::test]
    async fn touch_session_updates_last_active() {
        let manager = AppSessionManager::new();
        let session = manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await
            .expect("create session");
        let before = session.last_active;
        tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
        manager
            .touch_session(&session.session_id)
            .await
            .expect("touch session");
        let updated = manager
            .get_session(&session.session_id)
            .await
            .expect("get session");
        assert!(updated.last_active > before);
    }

    #[tokio::test]
    async fn update_session_state_changes_state() {
        let manager = AppSessionManager::new();
        let session = manager
            .create_session("inspector", "org", json!({}), json!({}), None)
            .await
            .expect("create session");
        manager
            .update_session_state(&session.session_id, "ready", json!({"target": "test"}))
            .await
            .expect("update state");
        let updated = manager
            .get_session(&session.session_id)
            .await
            .expect("get session");
        assert_eq!(updated.state, "ready");
    }
}
