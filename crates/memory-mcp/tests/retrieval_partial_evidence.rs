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

mod common;

struct Inventories {
    phases: std::sync::Mutex<Vec<String>>,
    rows: std::sync::Mutex<Vec<memory_mcp::eval_support::retrieval::CandidateInventory>>,
    work: std::sync::Mutex<Vec<memory_mcp::eval_support::retrieval::LexicalWork>>,
}
impl memory_mcp::eval_support::retrieval::CandidateObserver for Inventories {
    fn observe_phase(&self, stage: &str, _: f64) {
        self.phases.lock().unwrap().push(stage.to_owned());
    }
    fn observe(&self, inventory: &memory_mcp::eval_support::retrieval::CandidateInventory) {
        self.rows.lock().unwrap().push(inventory.clone());
    }
    fn observe_work(&self, work: &memory_mcp::eval_support::retrieval::LexicalWork) {
        self.work.lock().unwrap().push(work.clone());
    }
}

async fn read_url_admission(
    facts: &[(&str, &str, &str, &[&str])],
) -> (
    memory_mcp::eval_support::retrieval::ContextEvidence,
    memory_mcp::eval_support::retrieval::LexicalWork,
) {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let (service, db) = common::make_service_with_client().await;
    for (id, fact_type, content, keys) in facts {
        db.create(
            &format!("fact:{id}"),
            json!({
                "fact_id":format!("fact:{id}"),"fact_type":fact_type,"content":content,"quote":content,
                "source_episode":format!("episode:{id}"),"t_valid":"2026-01-01T00:00:00Z",
                "t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":keys,
                "scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}
            }),
            "org",
            &["t_valid", "t_ingested"],
        )
        .await
        .unwrap();
    }
    let sink = Arc::new(Inventories {
        phases: Default::default(),
        rows: Default::default(),
        work: Default::default(),
    });
    let capture = ContextEvidenceCapture::with_candidate_observer("episode:probe", sink.clone());
    let request = serde_json::from_value(json!({
        "query":"archive source","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"
    }))
    .unwrap();
    let actual = assemble_context_recording_evidence(&service, request, &capture)
        .await
        .unwrap();
    let admission = sink
        .work
        .lock()
        .unwrap()
        .iter()
        .find(|work| work.stage == "admission.topics")
        .expect("the public read must report completed admission work")
        .clone();
    (actual, admission)
}

#[tokio::test]
async fn public_url_admission_builds_one_view_for_observations_with_repeated_topics() {
    let keys = &["archive", "https://example.invalid/source/frostline"];
    let (read, work) = read_url_admission(&[
        (
            "probe",
            "observation",
            "Archive source logged frostline attendance.",
            keys,
        ),
        (
            "control",
            "observation",
            "Archive source updated frostline capacity.",
            keys,
        ),
    ])
    .await;
    assert_eq!(read.items.len(), 2);
    assert_eq!(
        read.items[0].provenance["retrieval_admission"]["matched_index_key_terms"],
        json!(["archive"])
    );
    assert_eq!(work.records, 2);
    assert_eq!(work.index_dictionaries, 1);
}

#[tokio::test]
async fn public_url_admission_uses_raw_view_for_repeated_topic_experience() {
    let (read, work) = read_url_admission(&[
        (
            "directa",
            "observation",
            "Archive source logged frostline attendance.",
            &[],
        ),
        (
            "directb",
            "observation",
            "Archive source updated frostline capacity.",
            &[],
        ),
        (
            "probe",
            "experience",
            "Reserve a quiet corner for rehearsals.",
            &["https://example.invalid/archive/frostline"],
        ),
    ])
    .await;
    let probe = read
        .items
        .iter()
        .find(|item| item.fact_id == "fact:probe")
        .expect("raw URL evidence admits the experience");
    let evidence = &probe.provenance["retrieval_admission"];
    assert_eq!(evidence["basis"], "repeated_topic");
    assert_eq!(evidence["matched_index_key_terms"], json!([]));
    assert_eq!(
        evidence["repeated_topic"],
        json!({
            "matched_query_index_terms":["archive"],
            "matched_repeated_topics":["frostline"],
            "supporting_fact_ids":["fact:directa","fact:directb"]
        })
    );
    assert_eq!(work.records, 3);
    assert_eq!(work.index_dictionaries, 3);
}

#[tokio::test]
async fn public_read_observes_actual_time_visible_candidate_payload_without_source_text() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let (service, db) = common::make_service_with_client().await;
    for (id, content, source, keys, valid) in [
        (
            "fact:probea",
            "archive source alpha",
            "episode:probea",
            vec!["M-42", "Été"],
            "2026-01-01T00:00:00Z",
        ),
        (
            "fact:probeb",
            "archive source beta",
            "episode:probeb",
            vec!["M-42", "Été"],
            "2026-01-01T00:00:00Z",
        ),
        (
            "fact:future",
            "archive source future",
            "episode:future",
            vec![],
            "2027-01-01T00:00:00Z",
        ),
    ] {
        db.create(id, json!({"fact_id":id,"fact_type":"observation","content":content,"quote":content,
            "source_episode":source,"t_valid":valid,"t_ingested":valid,"confidence":0.9,"index_keys":keys,
            "scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
            "org", &["t_valid", "t_ingested"]).await.unwrap();
    }
    let sink = Arc::new(Inventories {
        phases: std::sync::Mutex::new(Vec::new()),
        rows: std::sync::Mutex::new(Vec::new()),
        work: std::sync::Mutex::new(Vec::new()),
    });
    let capture = ContextEvidenceCapture::with_candidate_observer("episode:probea", sink.clone());
    let request = serde_json::from_value(
        json!({"query":"archive source","budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    assemble_context_recording_evidence(&service, request, &capture)
        .await
        .unwrap();
    assert_eq!(
        *sink.phases.lock().unwrap(),
        [
            "channels.completed",
            "subject.resolved",
            "topics.completed",
            "fusion.completed",
            "admission.completed",
            "selection.completed",
            "access_heat.started",
            "access_heat.settled"
        ]
    );
    let observed = sink.rows.lock().unwrap();
    let initial = observed
        .iter()
        .find(|row| row.stage == "facts.initial_fetched")
        .expect("actual candidate boundary must be emitted before ranking");
    assert_eq!(initial.records, 2);
    assert_eq!(initial.content_bytes, 39);
    assert_eq!(initial.max_content_bytes, 20);
    assert_eq!(initial.index_keys, 4);
    assert_eq!(initial.index_key_bytes, 18);
    assert_eq!(initial.max_index_key_bytes, 5);
    assert_eq!(initial.unique_contents, 2);
    assert_eq!(initial.unique_sources, 2);
    assert_eq!(capture.candidates().unwrap().len(), observed.len());
    let work = sink.work.lock().unwrap();
    assert!(
        !work.is_empty(),
        "actual dictionary construction must be observed"
    );
    assert_eq!(work[0].records, 2);
    assert_eq!(work[0].stage, "lexical.pool");
    assert_eq!(work[0].content_dictionaries, 2);
    assert_eq!(
        work[0].index_dictionaries, 1,
        "different fact identities with equal key lists share one dictionary"
    );
    let admission = work
        .iter()
        .find(|item| item.stage == "admission.topics")
        .expect("completed admission must report its own actual dictionary work");
    assert_eq!(admission.records, 2);
    assert_eq!(admission.content_dictionaries, 2);
    assert_eq!(admission.index_dictionaries, 1);
    let serialized = serde_json::to_string(initial).unwrap();
    assert!(!serialized.contains("archive source"));
    assert!(!serialized.contains("index-only"));
    assert!(!serialized.contains("episode:probe"));
}

#[tokio::test]
async fn duplicate_query_grounding_can_choose_a_different_source_sentence() {
    use memory_mcp::eval_support::retrieval::{
        ContextEvidenceCapture, assemble_context_recording_evidence,
    };
    let (service, db) = common::make_service_with_client().await;
    let content = "Falcon Falcon. Falcon archive source.";
    db.create("fact:distinctspan", json!({"fact_id":"fact:distinctspan","fact_type":"observation",
        "content":content,"quote":content,"source_episode":"episode:distinctspan",
        "t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,
        "index_keys":[],"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
        "org", &["t_valid","t_ingested"]).await.unwrap();
    let request = serde_json::from_value(json!({"query":"Falcon Falcon Falcon Falcon Falcon Falcon Falcon Falcon Falcon Falcon archive source",
        "budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap();
    let capture = ContextEvidenceCapture::new("episode:distinctspan");
    let actual = assemble_context_recording_evidence(&service, request, &capture)
        .await
        .unwrap();
    assert_eq!(actual.items.len(), 1);
    assert_eq!(actual.items[0].fact_id, "fact:distinctspan");
    assert_eq!(actual.items[0].quote, content);
    // Original query: "Falcon Falcon" scores28 vs19 for the second sentence.
    // Unique query: first scores1 vs10. Full coverage + phrase support gives
    // 0.55 +0.20 =0.75, whereas reusing the first span would give only0.55.
    assert_eq!(actual.items[0].grounding, Some(0.75));
}
