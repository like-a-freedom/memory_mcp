#![cfg(feature = "eval-support")]
//! Opt-in diagnostic of an existing isolated snapshot; never seeds a reduced history.
use memory_mcp::{config::SurrealConfigBuilder, service::MemoryService, storage::SurrealDbClient};
use std::{
    io::Write,
    sync::Arc,
    time::{Duration, Instant},
};

struct QueryProgress(Instant);

impl memory_mcp::eval_support::QueryObserver for QueryProgress {
    fn observe(&self, query: &memory_mcp::eval_support::SqlObservation) {
        println!(
            "FULL_HISTORY_QUERY {}",
            serde_json::json!({"elapsed_ms":self.0.elapsed().as_secs_f64()*1000.0,"sql":query.sql,"duration_us":query.micros})
        );
        std::io::stdout().flush().unwrap();
    }
}

async fn connect_snapshot() -> Arc<SurrealDbClient> {
    let config = SurrealConfigBuilder::new()
        .embedded(true)
        .credentials("root", "root")
        .db_name("memory")
        .namespace("main")
        .data_dir(std::env::var("MEMORY_FULL_HISTORY_STORE").expect("disposable snapshot clone"))
        .build()
        .unwrap();
    Arc::new(SurrealDbClient::connect(&config).await.unwrap())
}

async fn observe(case_id: &str) {
    let input: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("MEMORY_FULL_HISTORY_INPUT").expect("registered input path"))
            .unwrap(),
    )
    .unwrap();
    let case = &input["cases"][case_id];
    let db = connect_snapshot().await;
    db.set_eval_query_observer(Arc::new(QueryProgress(Instant::now())));
    let episodes = memory_mcp::memory::episode_store::EpisodeStoreClient::new(db.clone(), "main");
    let sources = episodes
        .select_by_source_identity(
            "other",
            case["source_id"].as_str().unwrap(),
            "2000-01-01T00:00:00Z",
            2,
        )
        .await
        .unwrap();
    assert_eq!(
        sources.len(),
        1,
        "exact registered source must exist in full snapshot"
    );
    let source = &sources[0];
    let episode = source["episode_id"].as_str().unwrap();
    let span = case["required_serialized_span"].as_str().unwrap();
    assert!(
        source["content"].as_str().unwrap().contains(span),
        "formation carrier lacks reviewed span"
    );
    let service = MemoryService::new(db, "main".into(), "error".into(), 50, 100).unwrap();
    let request = serde_json::from_value(
        serde_json::json!({"query":case["query"],"budget":5,"compact":false}),
    )
    .unwrap();
    let started = Instant::now();
    println!(
        "FULL_HISTORY_START {}",
        serde_json::json!({"case_id":case_id,"source_episode":episode,"episode_count":episodes.count_episodes().await.unwrap(),"formation_complete":true})
    );
    let capture = memory_mcp::eval_support::retrieval::ContextEvidenceCapture::new(episode);
    let observed = tokio::time::timeout(
        Duration::from_secs(input["limits"]["read_seconds"].as_u64().unwrap()),
        memory_mcp::eval_support::retrieval::assemble_context_recording_evidence(
            &service, request, &capture,
        ),
    )
    .await;
    match observed {
        Ok(Ok(receipt)) => {
            println!(
                "FULL_HISTORY_RECEIPT {}",
                serde_json::json!({"case_id":case_id,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"receipt":receipt})
            );
            assert!(
                receipt
                    .items
                    .iter()
                    .any(|item| item.source_episode == episode && item.content.contains(span)),
                "full-history retrieval must retain the complete reviewed source block"
            );
        }
        other => {
            println!(
                "FULL_HISTORY_PARTIAL_RECEIPT {}",
                serde_json::json!({"case_id":case_id,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"completed":false,"stages":capture.stages().unwrap()})
            );
            std::io::stdout().flush().unwrap();
            println!(
                "FULL_HISTORY_FAILURE {}",
                serde_json::json!({"case_id":case_id,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"error":format!("{other:?}")})
            );
            panic!("full-history read did not complete within registered budget");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires registered full snapshot clone and input; local diagnostic only"]
async fn full_history_dropdown_retains_reviewed_source() {
    observe("0c6b0c60").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires registered full snapshot clone and input; local diagnostic only"]
async fn full_history_toolbar_retains_reviewed_source() {
    observe("f61a096f").await;
}

#[tokio::test]
#[ignore = "requires a disposable full snapshot clone; metadata observation, not quality acceptance"]
async fn full_history_schema_inventory() {
    use memory_mcp::storage::DbClient;

    let db = connect_snapshot().await;
    for (kind, sql) in [
        (
            "migrations",
            "SELECT script_name, checksum, status FROM script_migration ORDER BY script_name ASC",
        ),
        ("fact", "INFO FOR TABLE fact"),
        ("edge", "INFO FOR TABLE edge"),
        ("episode", "INFO FOR TABLE episode"),
    ] {
        let inventory = db.query(sql, None, "main").await.unwrap();
        println!(
            "FULL_HISTORY_SCHEMA {}",
            serde_json::json!({"kind":kind,"inventory":inventory})
        );
        std::io::stdout().flush().unwrap();
    }
}

const PAYLOAD_FIELDS: &str = "fact_id, episode_id, source_episode, \
    string::len(content) AS content_chars, crypto::sha256(content) AS content_sha256, \
    array::map(index_keys ?? [], |$key| string::len($key)) AS index_key_chars, \
    array::map(index_keys ?? [], |$key| crypto::sha256($key)) AS index_key_sha256";

#[tokio::test]
async fn payload_inventory_reports_characters_and_digests_without_source_text() {
    use memory_mcp::storage::DbClient;
    use serde_json::json;

    let db = SurrealDbClient::connect_in_memory_with_dimension(
        "payload_calibration",
        "main",
        "warn",
        64,
    )
    .await
    .unwrap();
    let rows = db
        .query(
            &format!("SELECT {PAYLOAD_FIELDS} FROM $rows"),
            Some(json!({"rows":[{"fact_id":"fact:calibration", "source_episode":"episode:calibration", "content":"éé", "index_keys":["α", "ab"]}]})),
            "main",
        )
        .await
        .unwrap();
    assert_eq!(
        rows,
        json!([{
            "fact_id":"fact:calibration", "episode_id":null, "source_episode":"episode:calibration",
            "content_chars":2,
            "content_sha256":"f13c007a1d8e6e1300b5957a143810cdd3555825466cf5d2617b1ac2fd8bd76b",
            "index_key_chars":[1,2],
            "index_key_sha256":["3e32da9648c78feedcc821d56e483c126dcd270e9fe49973c528e3a88912d557", "fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603"]
        }])
    );
}

#[tokio::test]
async fn payload_inventory_accepts_an_episode_without_index_keys() {
    use memory_mcp::storage::DbClient;
    use serde_json::json;

    let db = SurrealDbClient::connect_in_memory_with_dimension(
        "payload_missing_keys",
        "main",
        "warn",
        64,
    )
    .await
    .unwrap();
    let rows = db
        .query(
            &format!("SELECT {PAYLOAD_FIELDS} FROM $rows"),
            Some(json!({"rows":[{"episode_id":"episode:calibration", "content":"éé"}]})),
            "main",
        )
        .await
        .unwrap();
    assert_eq!(
        rows,
        json!([{
            "fact_id":null,"episode_id":"episode:calibration","source_episode":null,
            "content_chars":2,
            "content_sha256":"f13c007a1d8e6e1300b5957a143810cdd3555825466cf5d2617b1ac2fd8bd76b",
            "index_key_chars":[], "index_key_sha256":[]
        }])
    );
}

#[tokio::test]
#[ignore = "requires a disposable full snapshot; first stored row only, not a representative sample"]
async fn full_history_first_fact_payload() {
    use memory_mcp::storage::DbClient;

    let db = connect_snapshot().await;
    let sql = format!("SELECT {PAYLOAD_FIELDS} FROM fact LIMIT 1");
    println!(
        "FULL_HISTORY_INVENTORY_START {}",
        serde_json::json!({"kind":"first_fact_payload","sql":sql})
    );
    std::io::stdout().flush().unwrap();
    let started = Instant::now();
    let rows = db.query(&sql, None, "main").await.unwrap();
    println!(
        "FULL_HISTORY_INVENTORY {}",
        serde_json::json!({"kind":"first_fact_payload","elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"rows":rows})
    );
    std::io::stdout().flush().unwrap();
}
