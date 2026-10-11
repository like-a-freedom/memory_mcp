//! Refusal contract of the `extract` tool's argument validation.
//!
//! The four invalid-input shapes must each produce their exact
//! `MemoryError::Validation` message, and the checks must fire in a fixed
//! order: the dual-field check (content+text) wins over the dual-source
//! check (episode_id+inline) when all three are present. These tests pin
//! that contract so the C2 refactor cannot reorder or reword it.

use memory_mcp::MemoryService;
use memory_mcp::service::MemoryError;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use memory_mcp::tools::params::ExtractParams;
use std::sync::Arc;

async fn make_service(name: &str) -> MemoryService {
    let namespaces = vec!["org".to_string()];
    let db = Arc::new(
        SurrealDbClient::connect_in_memory_with_namespaces(name, &namespaces, "warn")
            .await
            .expect("in-memory database"),
    );
    db.apply_migrations("org")
        .await
        .expect("apply memory schema");
    MemoryService::new(db, "org".to_string(), "warn".to_string(), 50, 100).expect("memory service")
}

const DUAL_FIELD_MSG: &str = "Invalid extract arguments: use only one inline snake_case field — `content` or `text` — not both. Do not wrap arguments in `payload`.";
const DUAL_SOURCE_MSG: &str = "Invalid extract arguments: provide exactly one snake_case input source. Use `episode_id` for stored content, or `content`/`text` for inline text, but not both. Do not wrap arguments in `payload`.";
const NEITHER_SOURCE_MSG: &str = "Invalid extract arguments: provide exactly one snake_case input source — `episode_id` or non-empty `content`/`text`. Do not wrap arguments in `payload`.";

fn validation_message<T: std::fmt::Debug>(result: Result<T, MemoryError>) -> String {
    match result {
        Err(MemoryError::Validation(msg)) => msg,
        other => panic!("expected Validation error, got: {other:?}"),
    }
}

/// content + text (no episode_id): the dual-field check fires.
#[tokio::test]
async fn content_and_text_together_is_refused() {
    let service = make_service("extract_refuse_dual_field").await;
    let msg = validation_message(
        memory_mcp::tools::extract::extract(
            &service,
            ExtractParams {
                episode_id: None,
                content: Some("inline content".to_string()),
                text: Some("inline text".to_string()),
                source_type: None,
                source_id: None,
                t_ref: None,
                zero_shot_labels: None,
            },
        )
        .await,
    );
    assert_eq!(msg, DUAL_FIELD_MSG, "dual-field message must be exact");
}

/// episode_id + content (no text): the dual-source check fires.
#[tokio::test]
async fn episode_id_and_content_together_is_refused() {
    let service = make_service("extract_refuse_dual_source").await;
    let msg = validation_message(
        memory_mcp::tools::extract::extract(
            &service,
            ExtractParams {
                episode_id: Some("episode:test".to_string()),
                content: Some("inline content".to_string()),
                text: None,
                source_type: None,
                source_id: None,
                t_ref: None,
                zero_shot_labels: None,
            },
        )
        .await,
    );
    assert_eq!(msg, DUAL_SOURCE_MSG, "dual-source message must be exact");
}

/// No input at all: the neither-source check fires.
#[tokio::test]
async fn neither_episode_nor_inline_is_refused() {
    let service = make_service("extract_refuse_neither").await;
    let msg = validation_message(
        memory_mcp::tools::extract::extract(
            &service,
            ExtractParams {
                episode_id: None,
                content: None,
                text: None,
                source_type: None,
                source_id: None,
                t_ref: None,
                zero_shot_labels: None,
            },
        )
        .await,
    );
    assert_eq!(
        msg, NEITHER_SOURCE_MSG,
        "neither-source message must be exact"
    );
}

/// episode_id + content + text together: the dual-field check wins — its
/// message must be the one returned, not the dual-source or neither one.
#[tokio::test]
async fn dual_field_check_wins_over_dual_source_when_all_three_are_present() {
    let service = make_service("extract_refuse_precedence").await;
    let msg = validation_message(
        memory_mcp::tools::extract::extract(
            &service,
            ExtractParams {
                episode_id: Some("episode:test".to_string()),
                content: Some("inline content".to_string()),
                text: Some("inline text".to_string()),
                source_type: None,
                source_id: None,
                t_ref: None,
                zero_shot_labels: None,
            },
        )
        .await,
    );
    assert_eq!(
        msg, DUAL_FIELD_MSG,
        "dual-field check must fire first with its exact message, got: {msg}"
    );
}
