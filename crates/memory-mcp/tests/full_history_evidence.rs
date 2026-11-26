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

async fn observe(case_id: &str) {
    let input: serde_json::Value = serde_json::from_slice(
        &std::fs::read(std::env::var("MEMORY_FULL_HISTORY_INPUT").expect("registered input path"))
            .unwrap(),
    )
    .unwrap();
    let case = &input["cases"][case_id];
    let config = SurrealConfigBuilder::new()
        .embedded(true)
        .credentials("root", "root")
        .db_name("memory")
        .namespace("main")
        .data_dir(std::env::var("MEMORY_FULL_HISTORY_STORE").expect("disposable snapshot clone"))
        .build()
        .unwrap();
    let db = Arc::new(SurrealDbClient::connect(&config).await.unwrap());
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
    let observed = tokio::time::timeout(
        Duration::from_secs(input["limits"]["read_seconds"].as_u64().unwrap()),
        memory_mcp::eval_support::retrieval::assemble_context_with_evidence(
            &service, request, episode,
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
