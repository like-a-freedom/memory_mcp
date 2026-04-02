use memory_mcp::service::apps::session::AppSessionManager;
use memory_mcp::service::error::MemoryError;

#[tokio::test]
async fn create_and_get_session() {
    let manager = AppSessionManager::new();
    let session = manager
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), Some(3600))
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
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), Some(1))
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
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), None)
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
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), None)
        .await
        .expect("first session");
    manager
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), None)
        .await
        .expect("second session");
    let result = manager
        .create_session("inspector", "org", serde_json::json!({}), serde_json::json!({}), None)
        .await;
    assert!(matches!(result, Err(MemoryError::SessionLimitExceeded)));
}
