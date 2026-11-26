//! Alias work may be shared only when application and native query semantics agree.
#![cfg(feature = "eval-support")]
mod common;
use memory_mcp::{
    eval_support::{QueryObserver, SqlObservation},
    service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability,
    storage::DbClient,
};
use serde_json::json;
use std::sync::{Arc, Mutex};
#[derive(Default)]
struct Queries(Mutex<Vec<String>>);
impl QueryObserver for Queries {
    fn observe(&self, event: &SqlObservation) {
        self.0.lock().unwrap().push(event.sql.clone());
    }
}
#[derive(Clone, Copy)]
enum AnalysisFault {
    Failure,
    Incomplete,
    Different,
    Empty,
    EdgeNgram,
    DifferentContentIndex,
    DifferentKeysIndex,
    AdditionalContentIndex,
}
struct FaultDb {
    inner: Arc<memory_mcp::storage::SurrealDbClient>,
    fault: AnalysisFault,
}
#[async_trait::async_trait]
impl DbClient for FaultDb {
    async fn query(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
        ns: &str,
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        if sql.contains("search::analyze") {
            let streams = match self.fault {
                AnalysisFault::Failure => Err(memory_mcp::MemoryError::Storage(
                    "controlled unavailable analyzer".into(),
                )),
                AnalysisFault::Incomplete => Ok(json!([["base"]])),
                AnalysisFault::Different => Ok(json!([["base"], ["alias"]])),
                AnalysisFault::Empty => Ok(json!([[], []])),
                AnalysisFault::EdgeNgram
                | AnalysisFault::DifferentContentIndex
                | AnalysisFault::DifferentKeysIndex
                | AnalysisFault::AdditionalContentIndex => {
                    unreachable!("real analyzer, no injected response")
                }
            }?;
            let mut actual = self.inner.query(sql, vars, ns).await?;
            actual[0]["streams"] = streams;
            return Ok(actual);
        }
        self.inner.query(sql, vars, ns).await
    }
    async fn select_one(
        &self,
        id: &str,
        ns: &str,
    ) -> Result<Option<serde_json::Value>, memory_mcp::MemoryError> {
        self.inner.select_one(id, ns).await
    }
    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        ns: &str,
    ) -> Result<Vec<serde_json::Value>, memory_mcp::MemoryError> {
        self.inner.select_table(table, ns).await
    }
    async fn create(
        &self,
        id: &str,
        data: serde_json::Value,
        ns: &str,
        times: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        self.inner.create(id, data, ns, times).await
    }
    async fn update(
        &self,
        id: &str,
        data: serde_json::Value,
        ns: &str,
        times: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        self.inner.update(id, data, ns, times).await
    }
    async fn apply_migrations(&self, ns: &str) -> Result<(), memory_mcp::MemoryError> {
        self.inner.apply_migrations(ns).await
    }
}
async fn alias_read(canonical: &str, alias: &str) -> (Vec<String>, Vec<String>) {
    alias_read_with_fault(canonical, alias, None).await
}
async fn alias_read_with_fault(
    canonical: &str,
    alias: &str,
    fault: Option<AnalysisFault>,
) -> (Vec<String>, Vec<String>) {
    let (_, db) = common::make_service_with_client().await;
    if matches!(fault, Some(AnalysisFault::EdgeNgram)) {
        db.query("DEFINE ANALYZER OVERWRITE memory_fts TOKENIZERS class FILTERS lowercase, edgengram(1, 3)", None, "org").await.unwrap();
    }
    if matches!(fault, Some(AnalysisFault::DifferentContentIndex)) {
        db.query(
            "DEFINE ANALYZER literal_words TOKENIZERS class FILTERS lowercase",
            None,
            "org",
        )
        .await
        .unwrap();
        db.query("DEFINE INDEX OVERWRITE fact_content_search ON fact FIELDS content FULLTEXT ANALYZER literal_words", None, "org").await.unwrap();
    }
    if matches!(fault, Some(AnalysisFault::DifferentKeysIndex)) {
        db.query(
            "DEFINE ANALYZER literal_words TOKENIZERS class FILTERS lowercase",
            None,
            "org",
        )
        .await
        .unwrap();
        db.query("DEFINE INDEX OVERWRITE fact_index_keys_search ON fact FIELDS index_keys FULLTEXT ANALYZER literal_words", None, "org").await.unwrap();
    }
    if matches!(fault, Some(AnalysisFault::AdditionalContentIndex)) {
        db.query(
            "DEFINE ANALYZER literal_words TOKENIZERS class FILTERS lowercase",
            None,
            "org",
        )
        .await
        .unwrap();
        db.query("DEFINE INDEX additional_content_search ON fact FIELDS content FULLTEXT ANALYZER literal_words", None, "org").await.unwrap();
    }
    db.create("entity:alias",json!({"entity_id":"entity:alias","entity_type":"concept","canonical_name":canonical,"canonical_name_normalized":canonical,"aliases":[alias]}),"org",&[]).await.unwrap();
    for (name, word) in [("base", canonical), ("alias", alias)] {
        let id = format!("fact:{name}");
        let content = format!("Falcon deployment {word}");
        db.create(&id,json!({"fact_id":id,"fact_type":"observation","content":content,"quote":content,"source_episode":format!("episode:{name}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","confidence":0.9,"index_keys":[],"entity_links":[],"scope":"org","policy_tags":[],"provenance":{"source":"manual"}}),"org",&["t_valid","t_ingested"]).await.unwrap();
    }
    let client: Arc<dyn DbClient> = match fault {
        Some(
            AnalysisFault::EdgeNgram
            | AnalysisFault::DifferentContentIndex
            | AnalysisFault::DifferentKeysIndex
            | AnalysisFault::AdditionalContentIndex,
        ) => db.clone(),
        Some(fault) => Arc::new(FaultDb {
            inner: db.clone(),
            fault,
        }),
        None => db.clone(),
    };
    let service =
        memory_mcp::service::MemoryService::new(client, "org".into(), "error".into(), 50, 100)
            .unwrap();
    let queries = Arc::new(Queries::default());
    db.set_eval_query_observer(queries.clone());
    let items=AssembleContextCapability::assemble_context_from_service(&service,serde_json::from_value(json!({"query":format!("Falcon deployment {canonical}"),"budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"})).unwrap()).await.unwrap();
    let queries = queries
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|sql| {
            sql.contains("search::score(1) AS ft_score FROM fact")
                && !sql.contains("array::flatten([")
        })
        .cloned()
        .collect();
    (
        queries,
        items.into_iter().map(|item| item.fact_id).collect(),
    )
}
#[tokio::test]
async fn equivalent_native_and_application_alias_runs_one_lexical_read() {
    let (queries, items) = alias_read("action", "actions").await;
    assert_eq!(items, ["fact:alias", "fact:base"]);
    assert_eq!(
        queries.len(),
        1,
        "identical native and application token streams must share one lexical read"
    );
}
#[tokio::test]
async fn genuine_alias_keeps_its_independent_read_and_evidence() {
    let (queries, items) = alias_read("launch", "liftoff").await;
    assert_eq!(queries.len(), 2);
    assert!(items.contains(&"fact:alias".to_owned()));
    assert!(items.contains(&"fact:base".to_owned()));
}
#[tokio::test]
async fn native_stem_equivalence_alone_cannot_remove_different_application_query() {
    let (queries, items) = alias_read("deploy", "deployment").await;
    assert_eq!(queries.len(), 2);
    assert!(!items.is_empty());
}

#[tokio::test]
async fn unavailable_analyzer_keeps_the_alias_read() {
    let (queries, items) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::Failure)).await;
    assert_eq!(queries.len(), 2);
    assert_eq!(items, ["fact:alias", "fact:base"]);
}

#[tokio::test]
async fn incomplete_analyzer_keeps_the_alias_read() {
    let (queries, items) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::Incomplete)).await;
    assert_eq!(queries.len(), 2);
    assert_eq!(items, ["fact:alias", "fact:base"]);
}

#[tokio::test]
async fn different_native_tokens_keep_the_application_equivalent_alias() {
    let (queries, items) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::Different)).await;
    assert_eq!(queries.len(), 2);
    assert_eq!(items, ["fact:alias", "fact:base"]);
}

#[tokio::test]
async fn empty_native_tokens_cannot_establish_equivalence() {
    let (queries, items) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::Empty)).await;
    assert_eq!(queries.len(), 2);
    assert_eq!(items, ["fact:alias", "fact:base"]);
}

#[tokio::test]
async fn indexing_only_token_equivalence_cannot_remove_an_alias_read() {
    let (queries, _) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::EdgeNgram)).await;
    assert_eq!(
        queries.len(),
        2,
        "edgengram indexing and querying stages have different semantics"
    );
}

#[tokio::test]
async fn different_content_index_analyzer_keeps_the_alias_read() {
    let (queries, _) = alias_read_with_fault(
        "action",
        "actions",
        Some(AnalysisFault::DifferentContentIndex),
    )
    .await;
    assert_eq!(
        queries.len(),
        2,
        "analyzer equivalence must apply to the actual content index"
    );
}

#[tokio::test]
async fn different_key_index_analyzer_keeps_the_alias_read() {
    let (queries, _) =
        alias_read_with_fault("action", "actions", Some(AnalysisFault::DifferentKeysIndex)).await;
    assert_eq!(
        queries.len(),
        2,
        "analyzer equivalence must apply to the actual key index"
    );
}

#[tokio::test]
async fn additional_fulltext_index_cannot_change_alias_equivalence() {
    let (queries, _) = alias_read_with_fault(
        "action",
        "actions",
        Some(AnalysisFault::AdditionalContentIndex),
    )
    .await;
    assert_eq!(
        queries.len(),
        2,
        "an additional planner-selectable index makes equivalence unknown"
    );
}
