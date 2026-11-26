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
