#![cfg(feature = "eval-support")]
//! A cancelled diagnostic must retain completed source observations.
use std::sync::Arc;

use async_trait::async_trait;
use memory_mcp::{MemoryError, service::MemoryService, storage::DbClient};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

struct BlockedAfterSource {
    entered: Semaphore,
    policy_tags: Vec<String>,
    fail_after_source: bool,
}

#[async_trait]
impl DbClient for BlockedAfterSource {
    async fn query(&self, sql: &str, vars: Option<Value>, _: &str) -> Result<Value, MemoryError> {
        if sql == "SELECT * FROM episode WHERE episode_id = $episode_id LIMIT 1" {
            assert_eq!(vars.unwrap()["episode_id"], "episode:reviewed");
            return Ok(
                json!([{"episode_id":"episode:reviewed","content":"Verified archive source", "policy_tags":self.policy_tags} ]),
            );
        }
        if self.fail_after_source {
            return Err(MemoryError::Storage(
                "controlled post-formation failure".into(),
            ));
        }
        self.entered.add_permits(1);
        std::future::pending().await
    }
    async fn select_one(&self, _: &str, _: &str) -> Result<Option<Value>, MemoryError> {
        panic!("unexpected select_one")
    }
    async fn select_table(
        &self,
        _: memory_mcp::storage::table_scope::OwnedTable,
        _: &str,
    ) -> Result<Vec<Value>, MemoryError> {
        panic!("unexpected select_table")
    }
    async fn create(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        panic!("unexpected create")
    }
    async fn update(&self, _: &str, _: Value, _: &str, _: &[&str]) -> Result<Value, MemoryError> {
        panic!("unexpected update")
    }
    async fn apply_migrations(&self, _: &str) -> Result<(), MemoryError> {
        panic!("unexpected migration")
    }
}

#[tokio::test]
async fn cancelled_public_read_retains_completed_source_stage_without_claiming_selection() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };

    let db = Arc::new(BlockedAfterSource {
        entered: Semaphore::new(0),
        policy_tags: vec![],
        fail_after_source: false,
    });
    let service = MemoryService::new(db.clone(), "main".into(), "error".into(), 50, 100).unwrap();
    let capture = ContextEvidenceCapture::new("episode:reviewed");
    let request =
        serde_json::from_value(json!({"query":"archive source","budget":5,"compact":false}))
            .unwrap();
    let mut read = Box::pin(assemble_context_recording_evidence(
        &service, request, &capture,
    ));
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        tokio::select! {
            _ = &mut read => panic!("controlled read must stay pending"),
            permit = db.entered.acquire() => permit.unwrap().forget(),
        }
    })
    .await
    .expect("controlled boundary reached");
    // Dropping the future simulates outer timeout cancellation without a sleep.
    drop(read);
    let stages = capture.stages().unwrap();
    assert_eq!(stages.len(), 1, "unobserved stages must not be invented");
    assert_eq!(stages[0].stage, "formation");
    assert_eq!(stages[0].items.len(), 1);
    assert_eq!(stages[0].items[0].source_episode, "episode:reviewed");
    assert_eq!(stages[0].items[0].content, "Verified archive source");
}

#[tokio::test]
async fn failed_public_read_retains_source_evidence_and_original_storage_error() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let db = Arc::new(BlockedAfterSource {
        entered: Semaphore::new(0),
        policy_tags: vec![],
        fail_after_source: true,
    });
    let service = MemoryService::new(db, "main".into(), "error".into(), 50, 100).unwrap();
    let capture = ContextEvidenceCapture::new("episode:reviewed");
    let request =
        serde_json::from_value(json!({"query":"archive source","budget":5,"compact":false}))
            .unwrap();
    let result = assemble_context_recording_evidence(&service, request, &capture).await;
    assert!(
        matches!(result, Err(MemoryError::Storage(message)) if message.contains("controlled post-formation failure"))
    );
    let stages = capture.stages().unwrap();
    assert_eq!(stages.len(), 1);
    assert_eq!(stages[0].items[0].content, "Verified archive source");
}

#[tokio::test]
async fn failed_public_read_does_not_expose_a_policy_excluded_source() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let db = Arc::new(BlockedAfterSource {
        entered: Semaphore::new(0),
        policy_tags: vec!["private".into()],
        fail_after_source: true,
    });
    let service = MemoryService::new(db, "main".into(), "error".into(), 50, 100).unwrap();
    let capture = ContextEvidenceCapture::new("episode:reviewed");
    let request = serde_json::from_value(
        json!({"query":"archive source","budget":5,"compact":false,"access":{"allowed_tags":[]}}),
    )
    .unwrap();
    assert!(
        assemble_context_recording_evidence(&service, request, &capture)
            .await
            .is_err()
    );
    let stages = capture.stages().unwrap();
    assert_eq!(stages.len(), 1);
    assert!(stages[0].items.is_empty());
}

#[tokio::test]
async fn one_capture_cannot_merge_stages_from_two_public_reads() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let db = Arc::new(BlockedAfterSource {
        entered: Semaphore::new(0),
        policy_tags: vec![],
        fail_after_source: true,
    });
    let service = MemoryService::new(db, "main".into(), "error".into(), 50, 100).unwrap();
    let capture = ContextEvidenceCapture::new("episode:reviewed");
    let request: memory_mcp::models::AssembleContextRequest =
        serde_json::from_value(json!({"query":"archive source","budget":5,"compact":false}))
            .unwrap();
    assert!(
        assemble_context_recording_evidence(&service, request.clone(), &capture)
            .await
            .is_err()
    );
    let second = assemble_context_recording_evidence(&service, request, &capture).await;
    assert!(
        matches!(second, Err(MemoryError::Validation(message)) if message.contains("one read")),
        "a reused capture must fail before another read"
    );
    assert_eq!(capture.stages().unwrap().len(), 1);
}
