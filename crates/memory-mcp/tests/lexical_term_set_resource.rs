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
}

struct CountingAllocator;

fn observe_allocation() {
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
        observe_allocation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        observe_allocation();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        observe_allocation();
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
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let actual = AssembleContextCapability::assemble_context_from_service(&service, request).await;
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
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
