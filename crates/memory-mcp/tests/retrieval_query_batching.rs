mod common;

#[cfg(feature = "eval-support")]
use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
#[cfg(feature = "eval-support")]
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
#[cfg(feature = "eval-support")]
use memory_mcp::knowledge::graph_store::KnowledgeGraphStore;
use memory_mcp::knowledge::queries::build_select_facts_filtered_query;
use memory_mcp::memory::episode_context_store::EpisodeContextStore;
#[cfg(feature = "eval-support")]
use memory_mcp::models::AssembleContextRequest;
use memory_mcp::models::{IngestRequest, Provenance};
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;
use memory_mcp::service::normalize_dt;
use memory_mcp::storage::DbClient;

#[cfg(feature = "eval-support")]
#[derive(Default)]
struct SqlRecorder(Mutex<Vec<String>>);

#[cfg(feature = "eval-support")]
impl QueryObserver for SqlRecorder {
    fn observe(&self, observation: &SqlObservation) {
        self.0
            .lock()
            .expect("query recorder lock")
            .push(observation.sql.clone());
    }
}

#[cfg(feature = "eval-support")]
fn request(query: &str) -> AssembleContextRequest {
    AssembleContextRequest {
        query: query.to_string(),
        as_of: Some(Utc::now() + Duration::minutes(1)),
        budget: 10,
        view_mode: None,
        window_start: None,
        window_end: None,
        access: None,
        fact_types: Vec::new(),
        compact: false,
    }
}

#[tokio::test]
#[cfg(feature = "eval-support")]
async fn fact_fallback_batches_term_queries_within_the_rpc_budget() {
    let memory = common::TestMemory::new(false).await;
    let recorder = Arc::new(SqlRecorder::default());
    memory.db_client.set_eval_query_observer(recorder.clone());

    memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("marmot zebra otter"),
    )
    .await
    .expect("assemble context through the fact fallback path");

    let fact_search_rpcs = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .filter(|sql| {
            let sql = sql.to_ascii_lowercase();
            sql.contains("from fact") && sql.contains("@1@")
        })
        .count();
    assert!(
        fact_search_rpcs <= 2,
        "three query terms should use one initial fact search and one fallback batch; observed {fact_search_rpcs} fact-search RPCs"
    );
}

#[tokio::test]
#[cfg(feature = "eval-support")]
async fn episode_fallback_batches_terms_without_changing_substring_queries() {
    let memory = common::TestMemory::new(false).await;
    let recorder = Arc::new(SqlRecorder::default());
    memory.db_client.set_eval_query_observer(recorder.clone());

    memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("marmot zebra otter"),
    )
    .await
    .expect("assemble context through the episode fallback path");

    let episode_search_rpcs = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .filter(|sql| {
            let sql = sql.to_ascii_lowercase();
            sql.contains("from episode") && sql.contains("string::contains")
        })
        .count();
    assert!(
        episode_search_rpcs <= 2,
        "three query terms should use one initial episode search and one fallback batch; observed {episode_search_rpcs} episode-search RPCs"
    );
}

#[tokio::test]
async fn surreal_subqueries_preserve_each_ordered_fact_score_group() {
    let memory = common::TestMemory::new(false).await;
    let now = Utc::now();
    let mut expected_rollout_ids = Vec::new();
    for index in 0..55 {
        let fact_id = memory
            .service
            .add_fact(
                "note",
                &format!("rollout checklist entry {index}"),
                "source quote",
                "episode:batch-ft-search",
                now - Duration::seconds(index),
                0.9,
                Vec::new(),
                Vec::new(),
                Provenance::agent_observation("episode:batch-ft-search"),
            )
            .await
            .expect("seed FTS limit rows");
        expected_rollout_ids.push(fact_id);
    }
    let rare_fact_id = memory
        .service
        .add_fact(
            "note",
            "saffronanswer contains a late rare result",
            "rare source quote",
            "episode:batch-ft-search",
            now,
            0.9,
            Vec::new(),
            Vec::new(),
            Provenance::agent_observation("episode:batch-ft-search"),
        )
        .await
        .expect("seed late-term rare result");

    let cutoff = normalize_dt(now + Duration::seconds(1));
    let terms = ["rollout", "saffronanswer"];
    let mut sequential = Vec::new();
    for term in terms {
        let (sql, vars) = build_select_facts_filtered_query(&cutoff, Some(term), 50, &[]);
        let rows = memory
            .db_client
            .query(&sql, Some(vars), "org")
            .await
            .expect("read one FTS group as the sequential oracle");
        sequential.push(rows.as_array().expect("fact query returns rows").clone());
    }
    assert_eq!(sequential[0].len(), 50);
    assert_eq!(sequential[1].len(), 1);
    assert_eq!(sequential[1][0]["fact_id"], rare_fact_id);

    let subqueries = terms
        .iter()
        .map(|term| {
            let (sql, _) = build_select_facts_filtered_query(&cutoff, Some(term), 50, &[]);
            format!("({sql})")
        })
        .collect::<Vec<_>>();
    let grouped = memory
        .db_client
        .query(
            &format!("RETURN [{}]", subqueries.join(", ")),
            Some(serde_json::json!({"cutoff": cutoff, "limit": 50})),
            "org",
        )
        .await
        .expect("run nested FTS subqueries in one SurrealDB RPC");
    let groups = grouped.as_array().expect("top-level query result groups");
    assert_eq!(groups.len(), terms.len());
    let actual = groups
        .iter()
        .map(|group| {
            group
                .as_array()
                .expect("each subquery returns an ordered row array")
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(
        actual, sequential,
        "nested FTS groups match sequential results and scores"
    );

    let first_group_ids = actual[0]
        .iter()
        .map(|row| row["fact_id"].as_str().expect("fact ID"))
        .collect::<Vec<_>>();
    assert_eq!(first_group_ids.len(), 50);
    assert!(
        expected_rollout_ids
            .iter()
            .any(|fact_id| first_group_ids.contains(&fact_id.as_str()))
    );
    assert_eq!(actual[1][0]["fact_id"], rare_fact_id);
}

#[tokio::test]
async fn episode_subqueries_preserve_unicode_casefolded_substring_groups() {
    let memory = common::TestMemory::new(false).await;
    let now = Utc::now();
    for (source_id, content, offset) in [
        ("batch-episode-cyrillic", "КРАСИВЫЙ спутниковый снимок", 0),
        ("batch-episode-latin", "A microORBITal instrument update", 1),
    ] {
        IngestCapability::ingest_from_service(
            &memory.service,
            IngestRequest {
                source_context: None,
                source_type: "document".to_string(),
                source_id: source_id.to_string(),
                content: content.to_string(),
                t_ref: now - Duration::seconds(offset),
                t_ingested: Some(now - Duration::seconds(offset)),
                policy_tags: Vec::new(),
            },
            None,
        )
        .await
        .expect("seed Unicode and casefolded substring episode");
    }

    let cutoff = normalize_dt(now + Duration::seconds(1));
    let terms = ["спутник", "orbit"];
    let store = EpisodeContextStore::new(memory.db_client.clone(), "org");
    let mut sequential = Vec::new();
    for term in terms {
        sequential.push(
            store
                .select_episodes_by_content(&cutoff, Some(term), 10)
                .await
                .expect("read one substring group as the sequential oracle"),
        );
    }
    assert_eq!(sequential[0].len(), 1);
    assert_eq!(sequential[1].len(), 1);

    let mut variables = serde_json::Map::new();
    variables.insert("cutoff".to_string(), serde_json::json!(cutoff));
    variables.insert("limit".to_string(), serde_json::json!(10));
    let subqueries = terms
        .iter()
        .enumerate()
        .map(|(index, term)| {
            let (sql, vars) = memory_mcp::memory::queries::build_select_episodes_by_content_query(
                &cutoff,
                Some(term),
                10,
            );
            variables.insert(format!("query_{index}"), vars["query"].clone());
            let sql = sql.replace("$query", &format!("$query_{index}"));
            format!("({sql})")
        })
        .collect::<Vec<_>>();
    let grouped = memory
        .db_client
        .query(
            &format!("RETURN [{}]", subqueries.join(", ")),
            Some(serde_json::Value::Object(variables)),
            "org",
        )
        .await
        .expect("run grouped episode substring queries in one SurrealDB RPC");
    let groups = grouped.as_array().expect("top-level query result groups");
    assert_eq!(groups.len(), terms.len());
    let actual = groups
        .iter()
        .map(|group| {
            group
                .as_array()
                .expect("each episode subquery returns an array")
                .clone()
        })
        .collect::<Vec<_>>();
    assert_eq!(actual, sequential);
    assert!(
        actual[0][0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("спутниковый")
    );
    assert!(
        actual[1][0]["content"]
            .as_str()
            .unwrap_or_default()
            .contains("microORBITal")
    );
}

#[tokio::test]
#[cfg(feature = "eval-support")]
async fn fact_term_batches_are_limited_to_eight_groups_per_rpc() {
    let memory = common::TestMemory::new(false).await;
    let recorder = Arc::new(SqlRecorder::default());
    memory.db_client.set_eval_query_observer(recorder.clone());
    memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("marmot zebra otter badger stoat"),
    )
        .await
        .expect("fact fallback should assemble across bounded batches");

    let grouped_queries = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .filter(|sql| sql.starts_with("RETURN [") && sql.contains("search::score(1)"))
        .map(|sql| sql.matches("search::score(1)").count())
        .collect::<Vec<_>>();
    assert!(grouped_queries.iter().all(|count| *count <= 8));
    let total_groups = grouped_queries.iter().sum::<usize>();
    assert!(
        total_groups > 8,
        "the query should exercise more than one batch"
    );
    assert_eq!(grouped_queries.len(), total_groups.div_ceil(8));
}

#[tokio::test]
#[cfg(feature = "eval-support")]
async fn episode_term_batches_are_limited_to_eight_groups_per_rpc() {
    let memory = common::TestMemory::new(false).await;
    let recorder = Arc::new(SqlRecorder::default());
    memory.db_client.set_eval_query_observer(recorder.clone());
    memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability::assemble_context_from_service(
        &memory.service,
        request("marmot zebra otter badger stoat"),
    )
        .await
        .expect("episode fallback should assemble across bounded batches");

    let grouped_queries = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .filter(|sql| sql.starts_with("RETURN [") && sql.contains("string::contains"))
        .map(|sql| sql.matches("string::contains").count())
        .collect::<Vec<_>>();
    assert!(grouped_queries.iter().all(|count| *count <= 8));
    let total_groups = grouped_queries.iter().sum::<usize>();
    assert!(
        total_groups > 8,
        "the query should exercise more than one batch"
    );
    assert_eq!(grouped_queries.len(), total_groups.div_ceil(8));
}

#[tokio::test]
#[cfg(feature = "eval-support")]
async fn source_fact_reads_keep_independent_limits_in_batches_of_eight() {
    let memory = common::TestMemory::new(false).await;
    let now = Utc::now();
    let episode_ids = (0..10)
        .map(|index| format!("episode:source-batch-{index}"))
        .collect::<Vec<_>>();
    for episode_id in &episode_ids {
        for fact_index in 0..3 {
            memory
                .service
                .add_fact(
                    "note",
                    &format!("source candidate {episode_id} {fact_index}"),
                    "source quote",
                    episode_id,
                    now - Duration::seconds(fact_index),
                    0.9,
                    Vec::new(),
                    Vec::new(),
                    Provenance::agent_observation(episode_id),
                )
                .await
                .expect("seed source candidate facts");
        }
    }

    let cutoff = normalize_dt(now + Duration::seconds(1));
    let store = KnowledgeGraphStore::new(memory.db_client.clone(), "org");
    let mut sequential = Vec::new();
    for episode_id in &episode_ids {
        sequential.push(
            store
                .select_active_facts_by_episode(episode_id, &cutoff, 2)
                .await
                .expect("read one independently limited source group"),
        );
    }

    let recorder = Arc::new(SqlRecorder::default());
    memory.db_client.set_eval_query_observer(recorder.clone());
    let actual = store
        .select_active_facts_by_episodes(&episode_ids, &cutoff, 2)
        .await
        .expect("read bounded source groups in batches");

    assert_eq!(
        actual, sequential,
        "each episode keeps its own result group and limit"
    );
    let grouped_queries = recorder
        .0
        .lock()
        .expect("query recorder lock")
        .iter()
        .filter(|sql| sql.starts_with("RETURN [") && sql.contains("source_episode = $episode_"))
        .map(|sql| sql.matches("source_episode = $episode_").count())
        .collect::<Vec<_>>();
    assert_eq!(grouped_queries, [8, 2]);
}
