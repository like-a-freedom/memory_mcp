//! CLI command adapters over a real in-memory service.
//!
//! `cli::commands::*` are thin adapters: they map parsed CLI args onto the
//! protocol-agnostic tool layer and write the JSON envelope. The logic worth
//! testing is the argument-to-parameter mapping and the error propagation, so
//! these tests drive the public `run` functions against a real
//! `MemoryService` on `mem://` and assert on the returned `Result`.
//!
//! They deliberately do not assert on stdout: `write_response` writes to the
//! process-wide stdout stream, which the test harness does not own.

use memory_mcp::MemoryService;
use memory_mcp::cli::args::{
    AssembleContextArgs, ExplainArgs, ExtractArgs, IngestArgs, InvalidateArgs, ResolveArgs,
};
use memory_mcp::cli::commands;
use memory_mcp::storage::SurrealDbClient;

/// A `MemoryService` over a fresh in-memory database.
async fn service() -> MemoryService {
    let client = SurrealDbClient::connect_in_memory("cli_commands", "org", "warn")
        .await
        .expect("in-memory client");
    MemoryService::new(
        std::sync::Arc::new(client),
        "org".to_string(),
        "warn".to_string(),
        50,
        100,
    )
    .expect("service")
}

fn ingest_args(content: &str) -> IngestArgs {
    IngestArgs {
        source_type: "note".to_string(),
        source_id: "s-1".to_string(),
        content: content.to_string(),
        t_ref: "2026-01-01T00:00:00Z".to_string(),
        t_ingested: None,
        policy_tags: Vec::new(),
    }
}

fn assemble_context_args(query: &str) -> AssembleContextArgs {
    AssembleContextArgs {
        query: query.to_string(),
        fact_types: Vec::new(),
        as_of: String::new(),
        budget: 5,
        view_mode: None,
        window_start: None,
        window_end: None,
    }
}

#[tokio::test]
async fn ingest_stores_a_note() {
    let service = service().await;

    let observed = commands::run_ingest(&service, ingest_args("the API moved to v2")).await;

    assert!(observed.is_ok());
}

#[tokio::test]
async fn ingest_rejects_a_malformed_t_ref() {
    let service = service().await;
    let mut args = ingest_args("content");
    args.t_ref = "not a timestamp".to_string();

    let observed = commands::run_ingest(&service, args).await;

    assert!(observed.is_err(), "an unparseable t_ref must not be stored");
}

#[tokio::test]
async fn extract_rejects_a_call_with_no_input_source() {
    let service = service().await;
    let args = ExtractArgs {
        episode_id: None,
        content: None,
        text: None,
        source_type: None,
        source_id: None,
        t_ref: None,
        zero_shot_labels: None,
    };

    let observed = commands::run_extract(&service, args).await;

    assert!(
        observed.is_err(),
        "extract needs an episode or inline content"
    );
}

#[tokio::test]
async fn extract_rejects_content_and_text_together() {
    let service = service().await;
    let args = ExtractArgs {
        episode_id: None,
        content: Some("alpha".to_string()),
        text: Some("beta".to_string()),
        source_type: None,
        source_id: None,
        t_ref: None,
        zero_shot_labels: None,
    };

    let observed = commands::run_extract(&service, args).await;

    assert!(observed.is_err(), "only one inline field may be supplied");
}

#[tokio::test]
async fn resolve_registers_a_canonical_entity() {
    let service = service().await;
    let args = ResolveArgs {
        entity_type: "project".to_string(),
        canonical_name: "Apollo".to_string(),
        aliases: vec!["Apollo 11".to_string()],
    };

    let observed = commands::run_resolve(&service, args).await;

    assert!(observed.is_ok());
}

#[tokio::test]
async fn invalidate_rejects_an_unknown_fact_id() {
    let service = service().await;
    let args = InvalidateArgs {
        fact_id: "fact:does-not-exist".to_string(),
        reason: "superseded".to_string(),
        t_invalid: "2026-01-01T00:00:00Z".to_string(),
    };

    let observed = commands::run_invalidate(&service, args).await;

    assert!(
        observed.is_err(),
        "invalidating an absent fact must not succeed"
    );
}

#[tokio::test]
async fn invalidate_rejects_a_malformed_t_invalid() {
    let service = service().await;
    let args = InvalidateArgs {
        fact_id: "fact:abc".to_string(),
        reason: "superseded".to_string(),
        t_invalid: "not a timestamp".to_string(),
    };

    let observed = commands::run_invalidate(&service, args).await;

    assert!(
        observed.is_err(),
        "an unparseable t_invalid must be refused"
    );
}

#[tokio::test]
async fn explain_rejects_context_items_that_are_not_json() {
    let service = service().await;
    let args = ExplainArgs {
        context_items: "not json".to_string(),
    };

    let observed = commands::run_explain(&service, args).await;

    assert!(
        observed.is_err(),
        "an unparseable context pack must be refused"
    );
}

#[tokio::test]
async fn explain_accepts_a_well_formed_context_pack() {
    let service = service().await;
    let args = ExplainArgs {
        context_items: r#"[{"content":"alpha","source_episode":"episode:abc"}]"#.to_string(),
    };

    let observed = commands::run_explain(&service, args).await;

    assert!(observed.is_ok());
}

#[tokio::test]
async fn assemble_context_answers_a_query_over_an_empty_store() {
    let service = service().await;

    let observed =
        commands::run_assemble_context(&service, assemble_context_args("what changed")).await;

    assert!(
        observed.is_ok(),
        "an empty store is a valid answer, not an error"
    );
}

#[tokio::test]
async fn assemble_context_ignores_a_blank_as_of() {
    // `as_of` defaults to the empty string and means "now"; a blank value must
    // not become a parse failure.
    let service = service().await;
    let args = assemble_context_args("what changed");

    let observed = commands::run_assemble_context(&service, args).await;

    assert!(observed.is_ok());
}

/// A well-formed `NormalizedHostEvent` for the lifecycle commands.
const VALID_EVENT: &str = r#"{
    "kind": "task_start",
    "task_fingerprint": "abc123",
    "policy_tags": [],
    "content": "starting work"
}"#;

/// A well-formed `InvocationContext` for the lifecycle commands.
const VALID_CONTEXT: &str = r#"{
    "origin": "cli",
    "session_id": "sess-1"
}"#;

/// A well-formed lifecycle-command argument pair.
fn lifecycle_args(event: &str, context: &str) -> memory_mcp::cli::args::LifecycleRecallArgs {
    memory_mcp::cli::args::LifecycleRecallArgs {
        event: event.to_string(),
        context: context.to_string(),
    }
}

/// A well-formed lifecycle-capture argument pair.
fn capture_args(event: &str, context: &str) -> memory_mcp::cli::args::LifecycleCaptureArgs {
    memory_mcp::cli::args::LifecycleCaptureArgs {
        event: event.to_string(),
        context: context.to_string(),
    }
}

#[tokio::test]
async fn lifecycle_recall_rejects_an_event_that_is_not_json() {
    let service = service().await;

    let observed =
        commands::run_lifecycle_recall(&service, lifecycle_args("not json", VALID_CONTEXT)).await;

    assert!(matches!(
        observed,
        Err(memory_mcp::error::MemoryError::Validation(_))
    ));
}

#[tokio::test]
async fn lifecycle_recall_rejects_a_context_that_is_not_json() {
    let service = service().await;

    let observed =
        commands::run_lifecycle_recall(&service, lifecycle_args(VALID_EVENT, "not json")).await;

    assert!(matches!(
        observed,
        Err(memory_mcp::error::MemoryError::Validation(_))
    ));
}

#[tokio::test]
async fn lifecycle_recall_names_the_event_argument_in_its_error() {
    let service = service().await;

    let observed =
        commands::run_lifecycle_recall(&service, lifecycle_args("not json", VALID_CONTEXT)).await;

    let Err(error) = observed else {
        panic!("invalid JSON must be rejected");
    };
    let message = error.to_string();
    assert!(
        message.contains("--event"),
        "the operator must be told which argument is malformed: {message}"
    );
}

#[tokio::test]
async fn lifecycle_capture_rejects_an_event_that_is_not_json() {
    let service = service().await;

    let observed =
        commands::run_lifecycle_capture(&service, capture_args("not json", VALID_CONTEXT)).await;

    assert!(matches!(
        observed,
        Err(memory_mcp::error::MemoryError::Validation(_))
    ));
}

#[tokio::test]
async fn lifecycle_capture_rejects_a_context_that_is_not_json() {
    let service = service().await;

    let observed =
        commands::run_lifecycle_capture(&service, capture_args(VALID_EVENT, "not json")).await;

    assert!(matches!(
        observed,
        Err(memory_mcp::error::MemoryError::Validation(_))
    ));
}
