//! Observable lexical matching and allocation contracts; no model or wall-clock gate.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::HashSet;

use memory_mcp::shared::search::search_query_terms;
use memory_mcp::shared::search_lexical::record_term_set;
use proptest::prelude::*;
use proptest::test_runner::RngSeed;
use serde_json::json;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    static ALLOCATED_BYTES: Cell<Option<usize>> = const { Cell::new(None) };
}

struct CountingAllocator;

fn observe_allocation(bytes: usize) {
    ALLOCATED_BYTES.with(|counter| {
        if let Some(total) = counter.get() {
            counter.set(Some(total + bytes));
        }
    });
    ALLOCATIONS.with(|counter| {
        if let Some(count) = counter.get() {
            counter.set(Some(count + 1));
        }
    });
}

// SAFETY: Every operation forwards the original pointer/layout to System.
// The counter is a const-initialized thread-local Cell and allocates nothing.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe_allocation(layout.size());
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe_allocation(size);
        unsafe { System.realloc(ptr, layout, size) }
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

#[test]
fn repeated_source_vocabulary_has_bounded_normalization_allocations() {
    // Warm regex/stemmer initialization outside the measurement; count only the
    // public operation, on this thread. No timing or allocator byte-size claim.
    record_term_set(&json!({"content":"Falcon Проекты M-42"}));
    let record = json!({"content":"Falcon deployment status M-42 ".repeat(2000),"index_keys":["M-42","M-42"]});
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let actual = record_term_set(&record);
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(
        actual,
        ["falcon", "deployment", "status", "42", "m42"]
            .map(str::to_owned)
            .into_iter()
            .collect::<HashSet<_>>()
    );
    println!("LEXICAL_SET_ALLOCATIONS {count}");
    assert!(
        count < 1000,
        "repeated vocabulary must not allocate per occurrence; observed {count}"
    );
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        failure_persistence: None,
        rng_seed: RngSeed::Fixed(20261126),
        .. ProptestConfig::default()
    })]
    #[test]
    fn term_sets_preserve_the_ordered_tokenizer_vocabulary(
        words in prop::collection::vec(prop::sample::select(vec![
            "Falcon", "falcon", "M-42", "episode:A-42", "\"R-0712\"", "2026-11-26",
            "проектами", "ПРОЕКТ", "и", "Ёлки", "déploiements", "deployments", "AND", "not",
            "pre\"A-2.part\"post", "\\nmenu", "\"unclosed", "a.b", "€42.50", "Studio 54",
        ]), 0..50),
        separator in prop::sample::select(vec![" ", "\n", "\t", "\\n"]),
        keys in prop::collection::vec(prop::sample::select(vec!["M-42", "проектами", "AND", "\"R-0712\"", "index-only"]), 0..5),
    ) {
        let content = words.join(separator);
        let mut expected = search_query_terms(&content).into_iter().collect::<HashSet<_>>();
        for key in &keys { expected.extend(search_query_terms(key)); }
        let actual = record_term_set(&json!({"content":content,"index_keys":keys}));
        prop_assert_eq!(actual, expected);
    }
}

mod common;

#[tokio::test]
async fn episode_fallback_reuses_lexical_features_with_bounded_allocations() {
    use memory_mcp::models::AssembleContextRequest;
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    use memory_mcp::storage::DbClient;

    let (service, db) = common::make_service_with_client().await;
    // All scores and reference times tie; the ID tie-break is independently
    // checked. Different source IDs/content prevent source deduplication.
    for n in 0..16 {
        let id = format!("episode:resource{n:02}");
        db.create(
            &id,
            json!({
                "episode_id":id, "source_type":"document", "source_id":format!("archive-{n}"),
                "content":format!("{} Record {n}", "archive source details. ".repeat(128)),
                "t_ref":"2026-03-01T00:00:00Z", "t_ingested":"2026-03-01T00:00:00Z",
                "policy_tags":[]
            }),
            "org",
            &["t_ref", "t_ingested"],
        )
        .await
        .unwrap();
    }
    let rows = memory_mcp::memory::EpisodeContextStore::new(db, "org")
        .select_episodes_by_content("2026-05-01T00:00:00Z", Some("archive source"), 20)
        .await
        .unwrap();
    assert_eq!(
        rows.len(),
        16,
        "fixture must expose all16 source candidates at the fixed cutoff"
    );
    let expected = [
        "episode:resource00",
        "episode:resource01",
        "episode:resource02",
        "episode:resource03",
        "episode:resource04",
    ];
    let request: AssembleContextRequest = serde_json::from_value(json!({
        "query":"archive source", "budget":5, "compact":false,
        "as_of":"2026-05-01T00:00:00Z"
    }))
    .unwrap();
    let (actual, count) = application_allocations(
        AssembleContextCapability::assemble_context_from_service(&service, request),
    )
    .await;
    let actual = actual.unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|item| item.source_episode.clone())
            .collect::<Vec<_>>(),
        expected
    );
    assert!(actual.iter().all(|item| item.grounding == Some(1.0)));
    assert!(
        actual
            .iter()
            .all(|item| (item.confidence - 0.7993775933609959).abs() < 1e-12)
    );
    println!("EPISODE_FALLBACK_ALLOCATIONS {count}");
    assert!(
        count < 200_000,
        "fixed16-source read must not normalize long source text per sort comparison; observed {count}"
    );
}

#[test]
fn index_key_vocabulary_normalizes_repeated_words_once_across_distinct_keys() {
    record_term_set(&json!({"content":"Falcon M-42"}));
    let keys = (0..2000)
        .map(|n| format!("Falcon source M-42 carrier{n:04}"))
        .collect::<Vec<_>>();
    let record = json!({"content":"Falcon", "index_keys":keys});
    let mut expected = ["falcon", "source", "42", "m42"]
        .map(str::to_owned)
        .into_iter()
        .collect::<HashSet<_>>();
    expected.extend((0..2000).map(|n| format!("carrier{n:04}")));
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let actual = record_term_set(&record);
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(actual, expected);
    println!("INDEX_VOCABULARY_ALLOCATIONS {count}");
    assert!(
        count < 30_000,
        "a2000-key dictionary with2004 terms must fit the30000-call allocation budget; observed {count}"
    );
}

#[test]
fn index_preprocessing_does_not_clone_unchanged_inputs() {
    record_term_set(&json!({"content":"AND OR NOT"}));
    let keys = (0..2000)
        .map(|n| format!("{}AND OR NOT", " ".repeat(n)))
        .collect::<Vec<_>>();
    let record = json!({"content":"AND OR NOT", "index_keys":keys});
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let actual = record_term_set(&record);
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert!(actual.is_empty());
    println!("UNCHANGED_INDEX_PREPROCESSING_ALLOCATIONS {count}");
    assert!(
        count < 1000,
        "unchanged inputs with three shared excluded tokens must not allocate copies per key; observed {count}"
    );
}

#[tokio::test]
async fn public_fact_read_reuses_index_vocabulary_without_changing_tied_source_order() {
    assert_index_vocabulary_read(8, "observation").await;
}

#[tokio::test]
async fn public_admission_reuses_exact_index_lists_across_distinct_fact_identities() {
    assert_index_vocabulary_read(32, "observation").await;
}

#[tokio::test]
async fn public_experience_read_reuses_index_vocabulary_for_repeated_topic_evidence() {
    assert_index_vocabulary_read(32, "experience").await;
}

#[tokio::test]
async fn public_selection_reuses_index_vocabulary_for_index_only_anchors() {
    assert_index_vocabulary_read_query(32, "observation", "Falcon source M-42 Falcon").await;
}

async fn assert_index_vocabulary_read(fact_count: usize, fact_type: &str) {
    assert_index_vocabulary_read_query(fact_count, fact_type, "Falcon source Falcon").await;
}

async fn assert_index_vocabulary_read_query(fact_count: usize, fact_type: &str, query: &str) {
    assert_index_vocabulary_read_variants(fact_count, fact_type, query, false).await;
}

async fn assert_index_vocabulary_read_variants(
    fact_count: usize,
    fact_type: &str,
    query: &str,
    distinct_lists: bool,
) {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    use memory_mcp::storage::DbClient;
    let (_, db) = common::make_service_with_client().await;
    for n in 0..fact_count {
        let id = format!("fact:keyresource{n:02}");
        let content = format!("Falcon source deployment number {n}.");
        let mut keys = (0..2000)
            .map(|k| format!("Falcon source M-42 carrier{k:04}"))
            .collect::<Vec<_>>();
        if distinct_lists {
            keys.push(format!("identity{n:02}"));
        }
        db.create(&id, json!({"fact_id":id,"fact_type":fact_type,"content":content,"quote":content,
            "source_episode":format!("episode:keyresource{n:02}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
            "confidence":0.9,"index_keys":keys,"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
            "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let service = memory_mcp::service::MemoryService::new(
        std::sync::Arc::new(MeasuredApplicationDb(db)),
        "org".into(),
        "error".into(),
        50,
        100,
    )
    .unwrap();
    let request = serde_json::from_value(
        json!({"query":query,"budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let (actual, count) = application_allocations(
        AssembleContextCapability::assemble_context_from_service(&service, request),
    )
    .await;
    let actual = actual.unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        [
            "fact:keyresource00",
            "fact:keyresource01",
            "fact:keyresource02",
            "fact:keyresource03",
            "fact:keyresource04"
        ]
    );
    assert!(actual.iter().all(|item| item.content == item.quote));
    println!("FACT_INDEX_APPLICATION_ALLOCATIONS facts={fact_count} allocations={count}");
    assert!(
        count < 1_000_000,
        "{fact_count}2000-key facts must fit the one-million-call application budget (storage excluded); observed {count}"
    );
}

/// The public use-case resource budget measures application work. Database
/// engine allocation/serialization is measured separately by full-history runs.
/// This target uses a current-thread runtime, so the pause cannot cross threads.
struct StorageAllocationPause(Option<usize>);
impl StorageAllocationPause {
    fn new() -> Self {
        Self(ALLOCATIONS.with(|counter| counter.replace(None)))
    }
}
impl Drop for StorageAllocationPause {
    fn drop(&mut self) {
        ALLOCATIONS.with(|counter| counter.set(self.0));
    }
}
async fn application_allocations<T>(future: impl std::future::Future<Output = T>) -> (T, usize) {
    let mut future = std::pin::pin!(future);
    let mut total = 0;
    let result = std::future::poll_fn(|cx| {
        // Measure this caller's application poll, excluding runtime/engine tasks
        // executed while it is suspended. Storage polls pause this same counter.
        ALLOCATIONS.with(|counter| assert_eq!(counter.replace(Some(0)), None));
        let result = future.as_mut().poll(cx);
        total += ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
        result
    })
    .await;
    (result, total)
}

async fn excluding_storage_allocations<T>(future: impl std::future::Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    std::future::poll_fn(|cx| {
        // Concurrent queries may suspend and finish in a different order.
        // Restore the caller counter after every poll, before another future runs.
        let _pause = StorageAllocationPause::new();
        future.as_mut().poll(cx)
    })
    .await
}

struct MeasuredApplicationDb(std::sync::Arc<dyn memory_mcp::storage::DbClient>);
#[async_trait::async_trait]
impl memory_mcp::storage::DbClient for MeasuredApplicationDb {
    async fn query(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
        namespace: &str,
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.query(sql, vars, namespace)).await
    }
    async fn select_one(
        &self,
        id: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Value>, memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.select_one(id, namespace)).await
    }
    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<serde_json::Value>, memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.select_table(table, namespace)).await
    }
    async fn create(
        &self,
        id: &str,
        value: serde_json::Value,
        namespace: &str,
        fields: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.create(id, value, namespace, fields)).await
    }
    async fn update(
        &self,
        id: &str,
        value: serde_json::Value,
        namespace: &str,
        fields: &[&str],
    ) -> Result<serde_json::Value, memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.update(id, value, namespace, fields)).await
    }
    async fn apply_migrations(&self, namespace: &str) -> Result<(), memory_mcp::MemoryError> {
        excluding_storage_allocations(self.0.apply_migrations(namespace)).await
    }
}

#[tokio::test]
async fn lexical_fact_projection_hydrates_each_winner_with_bounded_engine_allocations() {
    use memory_mcp::storage::{ContextFactQuery, DbClient};
    let (_, db) = common::make_service_with_client().await;
    for n in 0..2 {
        let id = format!("fact:hydration{n}");
        let keys = (0..1000)
            .map(|k| format!("Falcon source carrier{k:04}"))
            .collect::<Vec<_>>();
        db.create(&id,json!({"fact_id":id,"fact_type":"observation","content":"Falcon source","quote":"literal quote",
            "source_episode":format!("episode:hydration{n}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
            "confidence":0.9,"index_keys":keys,"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
            "org",&["t_valid","t_ingested"]).await.unwrap();
    }
    let store = memory_mcp::knowledge::KnowledgeStoreClient::new(db, "org");
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let rows = store
        .select_facts_filtered(ContextFactQuery {
            cutoff: "2026-05-01T00:00:00Z",
            query_contains: Some("Falcon source"),
            limit: 5,
            fact_types: &[],
        })
        .await;
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    let rows = rows.unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["fact_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["fact:hydration0", "fact:hydration1"]
    );
    assert_eq!(rows[0]["quote"], "literal quote");
    assert_eq!(rows[0]["index_keys"].as_array().unwrap().len(), 1000);
    println!("FACT_HYDRATION_ALLOCATIONS {count}");
    assert!(
        count < 200_000,
        "two1000-key winner projections must fit the200000-call engine budget; observed {count}"
    );
}

#[tokio::test]
async fn public_long_source_read_does_not_normalize_every_word_for_membership() {
    assert_long_read_with_bounded_allocations(
        "Falcon archive source deployment details. ",
        "Falcon archive source",
        2000,
    )
    .await;
}

#[tokio::test]
async fn public_unbroken_source_read_reuses_scoring_tokens() {
    assert_long_read_with_bounded_allocations(
        "Falcon archive source deployment details ",
        "Falcon archive source",
        2000,
    )
    .await;
}

#[tokio::test]
async fn public_repeated_query_read_reuses_grounding_spans() {
    assert_long_read_with_bounded_allocations(
        "Falcon archive source deployment details ",
        "Falcon archive Falcon source",
        5000,
    )
    .await;
}

async fn assert_long_read_with_bounded_allocations(
    source_unit: &str,
    query: &str,
    repetitions: usize,
) {
    assert_source_read_with_bounded_allocations(&source_unit.repeat(repetitions), query).await;
}

async fn assert_source_read_with_bounded_allocations(source: &str, query: &str) {
    use memory_mcp::service::memory_container_shims::memory_capabilities_assemble_context::AssembleContextCapability;
    use memory_mcp::storage::DbClient;
    let (_, db) = common::make_service_with_client().await;
    for n in 0..8 {
        let id = format!("fact:longresource{n:02}");
        let content = format!("{source} Record {n}");
        db.create(&id, json!({"fact_id":id,"fact_type":"observation","content":content,"quote":content,
            "source_episode":format!("episode:longresource{n:02}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
            "confidence":0.9,"index_keys":[],"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
            "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let service = memory_mcp::service::MemoryService::new(
        std::sync::Arc::new(MeasuredApplicationDb(db)),
        "org".into(),
        "error".into(),
        50,
        100,
    )
    .unwrap();
    let request = serde_json::from_value(
        json!({"query":query,"budget":5,"compact":false,"as_of":"2026-05-01T00:00:00Z"}),
    )
    .unwrap();
    let (actual, count) = application_allocations(
        AssembleContextCapability::assemble_context_from_service(&service, request),
    )
    .await;
    let actual = actual.unwrap();
    assert_eq!(
        actual
            .iter()
            .map(|item| item.fact_id.as_str())
            .collect::<Vec<_>>(),
        [
            "fact:longresource00",
            "fact:longresource01",
            "fact:longresource02",
            "fact:longresource03",
            "fact:longresource04"
        ]
    );
    let expected_text = source[..2048].to_owned();
    assert!(
        actual
            .iter()
            .all(|item| item.content == expected_text && item.quote == expected_text)
    );
    println!("LONG_SOURCE_APPLICATION_ALLOCATIONS {count}");
    assert!(
        count < 1_000_000,
        "eight long repeated-vocabulary sources must fit the million-call application budget; observed {count}"
    );
}

#[tokio::test]
async fn lexical_union_does_not_copy_the_growing_accumulator_per_hit() {
    use memory_mcp::{knowledge::KnowledgeStoreClient, storage::DbClient};
    let (_, db) = common::make_service_with_client().await;
    for n in 0..256 {
        let id = format!("fact:unionresource{n:03}");
        db.create(&id, json!({"fact_id":id,"fact_type":"observation","content":format!("hydration{} source evidence", n / 32),"quote":"literal evidence",
            "source_episode":format!("episode:unionresource{n:03}"),"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z",
            "confidence":0.9,"index_keys":[],"scope":"org","policy_tags":[],"entity_links":[],"provenance":{"source":"manual"}}),
            "org", &["t_valid","t_ingested"]).await.unwrap();
    }
    let store = KnowledgeStoreClient::new(db, "org");
    let terms = (0..8).map(|n| format!("hydration{n}")).collect::<Vec<_>>();
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let rows = store
        .select_lexical_fact_candidates("2026-05-01T00:00:00Z", &terms, 50, &[])
        .await;
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    let rows = rows.unwrap();
    assert_eq!(rows.len(), 256);
    assert_eq!(rows[0]["fact_id"], "fact:unionresource000");
    assert_eq!(rows[255]["fact_id"], "fact:unionresource255");
    assert_eq!(rows[0]["quote"], "literal evidence");
    println!("LEXICAL_UNION_ALLOCATIONS {count}");
    assert!(
        count < 200_000,
        "256 thin hits and256 hydrated winners must fit the200000-call owner-read budget; observed {count}"
    );
}

#[tokio::test]
async fn public_repeated_ordered_words_share_normalized_scoring_tokens() {
    assert_long_read_with_bounded_allocations(
        "Falcon archive source deployment details ",
        "Falcon archive source",
        20_000,
    )
    .await;
}

#[tokio::test]
async fn public_mostly_unique_ordered_words_keep_the_same_resource_bound() {
    let unique = (0..2000)
        .map(|n| format!("carrier{n:05}"))
        .collect::<Vec<_>>()
        .join(" ");
    let source = format!("Falcon archive source deployment details {unique}");
    assert_source_read_with_bounded_allocations(&source, "Falcon archive source").await;
}

#[tokio::test]
async fn public_distinct_index_lists_reuse_shared_token_normalization() {
    assert_index_vocabulary_read_variants(64, "observation", "Falcon source Falcon", true).await;
}

#[tokio::test]
async fn episode_term_batch_bounds_additional_source_allocation_bytes() {
    assert_episode_batch_additional_allocations([
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta",
    ])
    .await;
}

#[tokio::test]
async fn partial_episode_term_batch_bounds_additional_source_allocation_bytes() {
    assert_episode_batch_additional_allocations([
        "alpha",
        "absentone",
        "absenttwo",
        "absentthree",
        "absentfour",
        "absentfive",
        "absentsix",
        "absentseven",
    ])
    .await;
}

#[tokio::test]
async fn mostly_matching_episode_term_batch_bounds_additional_source_allocation_bytes() {
    assert_episode_batch_additional_allocations([
        "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "absent",
    ])
    .await;
}

async fn assert_episode_batch_additional_allocations(terms: [&str; 8]) {
    use memory_mcp::{memory::episode_context_store::EpisodeContextStore, storage::DbClient};
    let (_, db) = common::make_service_with_client().await;
    let text = format!(
        "Alpha beta gamma delta epsilon zeta eta theta {}",
        "x".repeat(65536)
    );
    for id in [
        "episode:bytes_a",
        "episode:bytes_b",
        "episode:bytes_c",
        "episode:bytes_d",
    ] {
        db.create(id, json!({"episode_id":id,"source_type":"document","source_id":id,"content":text,"t_ref":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z","scope":"org","policy_tags":[]}), "org", &["t_ref","t_ingested"]).await.unwrap();
    }
    let store = EpisodeContextStore::new(db, "org");
    let terms = terms.map(str::to_owned);
    ALLOCATED_BYTES.with(|counter| counter.set(Some(0)));
    let result = store
        .select_episode_candidates("2026-05-01T00:00:00Z", &terms, 4, &[])
        .await;
    let bytes = ALLOCATED_BYTES.with(|counter| counter.replace(None).unwrap());
    let rows = result.unwrap();
    assert_eq!(
        rows.iter()
            .map(|row| row["episode_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        [
            "episode:bytes_a",
            "episode:bytes_b",
            "episode:bytes_c",
            "episode:bytes_d"
        ]
    );
    assert!(rows.iter().all(|row| row["content"] == text));
    // A second identical-source read with one term accounts for fixed engine
    // planning/hydration traffic. The contract bounds additional term work,
    // rather than attributing every engine allocation to source duplication.
    ALLOCATED_BYTES.with(|counter| counter.set(Some(0)));
    let single = store
        .select_episode_candidates("2026-05-01T00:00:00Z", &["alpha".into()], 4, &[])
        .await;
    let single_bytes = ALLOCATED_BYTES.with(|counter| counter.replace(None).unwrap());
    assert_eq!(single.unwrap(), rows);
    let additional = bytes.saturating_sub(single_bytes);
    println!(
        "EPISODE_BATCH_ALLOCATED_BYTES eight={bytes} single={single_bytes} additional={additional}"
    );
    // Caller-thread requested bytes, not live memory or elapsed time. Preserve
    // the32-source-payload bound for work added by seven further terms.
    assert!(
        additional < text.len() * 4 * 32,
        "additional episode term allocation traffic: {additional}"
    );
}

#[tokio::test]
async fn public_composite_source_tokens_reuse_their_repeated_word_parts() {
    let source = (0..2000)
        .map(|n| format!("Falcon archive source deployment visible\\n\\t\\t[{n}]-Archive-Source "))
        .collect::<String>();
    assert_source_read_with_bounded_allocations(&source, "Falcon archive source").await;
}

#[test]
fn unicode_lowercase_shrinking_to_one_byte_keeps_the_existing_search_vocabulary() {
    assert_eq!(search_query_terms("K K KK K-42"), ["kk", "42", "k42"]);
}
