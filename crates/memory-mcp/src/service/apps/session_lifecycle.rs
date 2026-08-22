//! App session lifecycle orchestration (C6).
//!
//! Gathers the session command lifecycle (parse → dispatch → persist →
//! close → shape) into the service layer, so the MCP `app_command` handler
//! is a thin decode–call–encode adapter.

use crate::error::MemoryError;
use crate::mcp::response::AppCommandResult;
use crate::mcp::session::app_command_result_from_details;
use crate::service::MemoryService;
use crate::service::apps::dispatch::{AppContext, find_descriptor};
use crate::service::apps::session::SessionManager;
use crate::service::apps::workflow::{AppCommand, AppCommandInput};

/// Executes an app command against a session, returning the shaped result.
///
/// Orchestrates the full lifecycle: purge expired sessions, validate the
/// session, parse and dispatch the command, persist any payload mutation,
/// close the session when the command requires it, and shape the
/// protocol-facing [`AppCommandResult`].
pub(crate) async fn execute_app_command(
    service: &MemoryService,
    session_manager: &SessionManager,
    session_id: &str,
    input: AppCommandInput,
) -> Result<AppCommandResult, MemoryError> {
    session_manager.purge_expired().await;
    let session = session_manager.get_valid(session_id).await?;

    let command = AppCommand::parse(&session.app, input)
        .map_err(|error| MemoryError::Validation(error.to_string()))?;
    let descriptor =
        find_descriptor(&command).map_err(|error| MemoryError::Validation(error.to_string()))?;
    let ctx = AppContext {
        service,
        session_id,
        app: &session.app,
        payload: session.payload.clone(),
    };
    let outcome = (descriptor.execute)(&ctx, &command)
        .await
        .map_err(|error| MemoryError::Validation(error.to_string()))?;

    if let Some(payload) = outcome.new_payload {
        session_manager.replace_payload(session_id, payload).await?;
    }
    if outcome.close_session {
        session_manager.remove(session_id).await?;
    }

    let resource_uri = if outcome.close_session {
        None
    } else {
        Some(crate::mcp::resources::app_session_uri(
            &session.app,
            session_id,
        ))
    };

    Ok(app_command_result_from_details(
        &session.app,
        session_id,
        outcome.action,
        resource_uri,
        outcome.details,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::apps::session::{AppSessionState, SessionManager};
    use crate::storage::{DbClient, SurrealDbClient};
    use serde_json::json;
    use std::sync::Arc;

    async fn test_service() -> MemoryService {
        let namespaces = vec!["org".to_string()];
        let db_client = Arc::new(
            SurrealDbClient::connect_in_memory_with_namespaces(
                "memory_mcp_session_lifecycle_test",
                &namespaces,
                "warn",
            )
            .await
            .expect("connect in-memory test db"),
        );
        for namespace in &namespaces {
            db_client
                .apply_migrations(namespace)
                .await
                .expect("apply test migrations");
        }
        MemoryService::new(db_client, "org".to_string(), "warn".to_string(), 50, 100)
            .expect("create test service")
    }

    fn close_input() -> AppCommandInput {
        AppCommandInput {
            action: "close_session".to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn close_session_removes_session_and_shapes_result() {
        let service = test_service().await;
        let manager = SessionManager::new();
        manager
            .insert(
                "ses:0001".to_string(),
                AppSessionState {
                    app: "inspector".to_string(),
                    expires_at: None,
                    payload: json!({"app": "inspector"}),
                },
            )
            .await;

        let result = execute_app_command(&service, &manager, "ses:0001", close_input())
            .await
            .expect("close_session should succeed");

        assert_eq!(result.app, "inspector");
        assert_eq!(result.session_id, "ses:0001");
        assert_eq!(result.action, "close_session");
        assert!(result.ok);
        assert!(
            result.resource_uri.is_none(),
            "closed session must not advertise a resource URI"
        );
        assert!(
            manager.get("ses:0001").await.is_err(),
            "session must be removed after close_session"
        );
    }

    #[tokio::test]
    async fn persisting_command_keeps_session_and_updates_payload() {
        let service = test_service().await;
        let manager = SessionManager::new();
        manager
            .insert(
                "ses:0002".to_string(),
                AppSessionState {
                    app: "diff".to_string(),
                    expires_at: None,
                    payload: json!({"app": "diff", "exports": []}),
                },
            )
            .await;

        let input = AppCommandInput {
            action: "export_diff".to_string(),
            format: Some("json".to_string()),
            ..Default::default()
        };
        let result = execute_app_command(&service, &manager, "ses:0002", input)
            .await
            .expect("export_diff should succeed");

        assert_eq!(result.action, "export_diff");
        assert!(result.refresh_required);
        assert_eq!(
            result.resource_uri.as_deref(),
            Some("ui://memory/app/diff/ses:0002"),
            "open session must advertise its resource URI"
        );

        let session = manager
            .get("ses:0002")
            .await
            .expect("session must remain open");
        assert!(
            session.payload.get("last_export").is_some(),
            "mutated payload must be persisted back to the session"
        );
        assert_eq!(session.payload["exports"].as_array().map(Vec::len), Some(1));
    }

    #[tokio::test]
    async fn unknown_session_is_rejected() {
        let service = test_service().await;
        let manager = SessionManager::new();

        let err = execute_app_command(&service, &manager, "ses:missing", close_input())
            .await
            .expect_err("unknown session must be rejected");
        assert!(err.to_string().contains("Unknown or closed app session"));
    }

    #[tokio::test]
    async fn unsupported_action_is_rejected_as_invalid_params() {
        let service = test_service().await;
        let manager = SessionManager::new();
        manager
            .insert(
                "ses:0003".to_string(),
                AppSessionState {
                    app: "inspector".to_string(),
                    expires_at: None,
                    payload: json!({}),
                },
            )
            .await;

        let input = AppCommandInput {
            action: "no_such_action".to_string(),
            ..Default::default()
        };
        let err = execute_app_command(&service, &manager, "ses:0003", input)
            .await
            .expect_err("unsupported action must be rejected");
        assert!(err.to_string().contains("Unsupported app action"));
    }
}
