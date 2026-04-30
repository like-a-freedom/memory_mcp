# Local-First Retrieval Refinement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** strengthen `assemble_context` with deterministic query-intent routing, bounded entity-anchor graph expansion, and richer retrieval diagnostics while staying fully usable in lexical/graph-only mode and without any external LLM integration.

**Architecture:** Keep lexical/BM25 retrieval as the first stage and preserve the current MCP tool surface. Add a small internal query-mode layer that resolves automatic timeline behavior and graph hop budgets, then add an entity-anchor graph collector built on the existing `select_entities_batch`, `select_edge_neighbors`, and `select_facts_by_entity_links` primitives. Persist richer retrieval diagnostics into `query_log` and extend the evaluation harness with graph/timeline regressions so future ranking work has observable baselines.

**Tech Stack:** Rust 2024, rmcp, SurrealDB 3.x, chrono, serde/serde_json, existing `src/service/context/*` modules, query-log analytics, integration/acceptance/eval tests under `tests/`

---

## Scope guardrails

- Do **not** re-plan or re-implement shipped adaptive-memory work: `index_keys`, `access_count`, `last_accessed`, hot-aware archival/decay, timeline `view_mode`, or `tests/longmem_acceptance.rs`.
- Do **not** add a new public MCP tool. This work must stay behind `assemble_context` and optional query-log analytics.
- Do **not** require an external LLM, agent service, reranker, sidecar, or hosted search backend.
- Do **not** make graph expansion depend on semantic retrieval. The refined path must still work when embeddings are disabled or rebuilding.
- Keep the `AssembledContextItem` contract backwards-compatible. Prefer richer `rationale` text and additive `provenance` JSON over new required top-level fields.
- If query-log schema changes are needed, add exactly one new migration: `migrations/020_query_log_retrieval_diagnostics.surql`.
- Leave fact-type-specific decay and persona retention policy to a separate lifecycle plan.
- Leave embedding rebuild/resume work to `docs/superpowers/plans/2026-04-30-reembed-maintenance-implementation-plan.md`.

## Why this is one plan, not three

The next useful no-LLM work splits naturally into two categories: retrieval-time behavior and lifecycle-time behavior. This plan intentionally covers **only** the retrieval-time category because these changes all land in the same request path (`assemble_context`) and share the same verification surface (service/integration/eval tests). Selective decay/persona retention and embedding maintenance are separate subsystems and should stay in separate plans.

## File map

### Application files

- Create: `src/service/context/query_mode.rs` — deterministic query flags, phrase extraction, and automatic view resolution
- Create: `src/service/context/graph.rs` — entity-anchor resolution, bounded BFS over existing graph edges, and graph candidate shaping
- Modify: `src/service/context.rs` — compute resolved view mode/flags before dispatch and thread diagnostics through cache-hit and cache-miss paths
- Modify: `src/service/context/params.rs` — extend `DefaultContextParams` with resolved view-mode and query-flag context
- Modify: `src/service/context/alias_expansion.rs` — reuse shared query-phrase extraction helper instead of duplicating n-gram parsing
- Modify: `src/service/context/pipeline.rs` — invoke graph candidate collection between lexical retrieval and community/semantic expansion
- Modify: `src/service/context/ranking.rs` — add graph hop-aware ranking weights and graph trace support in `RankedContextFact`
- Modify: `src/service/context/scoring.rs` — enrich `provenance` with matched-query-term and graph-trace metadata
- Modify: `src/service/context/logging.rs` — persist `resolved_view_mode`, `query_flags`, and retrieval-tier distribution to `query_log`
- Modify: `src/storage/migrations.rs` — register migration `020_query_log_retrieval_diagnostics.surql`
- Create: `migrations/020_query_log_retrieval_diagnostics.surql` — additive `query_log` fields/indexes for retrieval diagnostics

### Tests and evals

- Modify: `tests/service_integration.rs` — auto-timeline, graph-anchor retrieval, and query-log diagnostics coverage
- Modify: `tests/service_acceptance.rs` — acceptance-level assertion that graph results expose deterministic trace metadata
- Modify: `tests/eval_retrieval.rs` — tag graph/timeline/first-person retrieval cases and assert coverage
- Modify: `tests/eval_support/metrics.rs` — aggregate pass-rate counts by eval tag in addition to tier
- Modify: `tests/eval_support/report.rs` — print expected-tag pass-rate slices in summary output
- Modify: `tests/fixtures/evals/retrieval_cases.json` — add tagged regression cases for `timeline_auto`, `graph_anchor`, and `first_person_rescue`

### Docs

- Modify: `README.md` — describe automatic timeline routing and richer query-log diagnostics
- Modify: `docs/MEMORY_SYSTEM_SPEC.md` — document auto timeline resolution and query-log diagnostic fields as shipped behavior once implemented
- Modify: `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md` — update Wave 4 status to reflect bounded entity-anchor graph expansion and deterministic query-mode routing

---

### Task 0: Capture baseline eval metrics before any code changes

> **⚠️ This task MUST run first, before any implementation.** It records the current retrieval quality so every subsequent task can measure whether the change helped or hurt.

**Files:**
- Create: `docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt` — snapshot of eval output before changes

- [ ] **Step 1: Run the internal retrieval eval and capture output**

Run:

```bash
mkdir -p docs/superpowers/plans/baselines
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt
```

Save the full output. The key metrics to record are:

- `suite=eval_retrieval total=N passed=N recall_at_5=X.XX mrr=X.XX top1_hit_rate=X.XX`
- Per-tier pass rates: `direct`, `alias`, `temporal`, `graph`, `reasoning`
- Diversity pass rate

- [ ] **Step 2: Run the external retrieval eval (sampled, 100 cases) and capture output**

Run:

```bash
MEMORY_MCP_EVAL_MAX_CASES=100 cargo test --test eval_external_retrieval -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt
```

The key metrics to record:

- `suite=longmemeval total=N passed=N recall_at_5=X.XX mrr=X.XX`
- `suite=locomo total=N passed=N recall_at_5=X.XX mrr=X.XX`

- [ ] **Step 3: Run the extraction and latency evals as secondary baselines**

Run:

```bash
cargo test --test eval_extraction -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt
cargo test --test eval_latency -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt
```

- [ ] **Step 4: Extract the headline numbers into a compact baseline summary**

Create `docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline-summary.md`:

```md
# Retrieval Baseline — 2026-04-30 (pre-refinement)

## Internal retrieval eval (eval_retrieval)
- recall_at_5: ____
- mrr: ____
- top1_hit_rate: ____
- direct pass_rate: ____
- alias pass_rate: ____
- temporal pass_rate: ____
- graph pass_rate: ____
- reasoning pass_rate: ____

## External — LongMemEval (100 cases)
- recall_at_5: ____
- mrr: ____
- top1_hit_rate: ____

## External — LoCoMo (100 cases)
- recall_at_5: ____
- mrr: ____
- top1_hit_rate: ____

## Extraction eval
- (pass/fail + key metric): ____

## Latency eval
- (pass/fail + key metric): ____
```

Fill in the blanks from the captured output.

- [ ] **Step 5: Commit the baseline**

Run:

```bash
git add docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline.txt docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline-summary.md
git commit -m "eval: capture pre-refinement retrieval baseline"
```

---

### Task 1: Detect query intent and auto-route timeline queries

**Files:**
- Create: `src/service/context/query_mode.rs`
- Modify: `src/service/context.rs`
- Modify: `src/service/context/params.rs`
- Modify: `src/service/context/alias_expansion.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Write the failing unit tests for deterministic query flags and explicit-view precedence**

Add these tests to `src/service/context/query_mode.rs` before the implementation:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};

    #[test]
    fn detect_query_flags_marks_timeline_and_path_queries() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

        let timeline_terms = crate::service::query::search_query_terms(
            "timeline of Atlas launch changes in March 2026",
        );
        let timeline_flags = detect_query_flags(
            Some("timeline of Atlas launch changes in March 2026"),
            &timeline_terms,
            cutoff,
        );
        assert!(timeline_flags.wants_timeline);
        assert!(!timeline_flags.wants_graph_path);
        assert!(timeline_flags.wants_graph_context);

        let path_terms = crate::service::query::search_query_terms(
            "who can introduce me to OpenAI",
        );
        let path_flags = detect_query_flags(
            Some("who can introduce me to OpenAI"),
            &path_terms,
            cutoff,
        );
        assert!(!path_flags.wants_timeline);
        assert!(path_flags.wants_graph_path);
        assert_eq!(path_flags.max_graph_hops(), 2);
    }

    #[test]
    fn resolve_view_mode_prefers_explicit_value_over_auto_detection() {
        let cutoff = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();
        let query_terms = crate::service::query::search_query_terms(
            "timeline of Atlas launch changes",
        );

        let (view_mode, flags) = resolve_view_mode(
            Some("map"),
            Some("timeline of Atlas launch changes"),
            &query_terms,
            cutoff,
        );

        assert_eq!(view_mode, ResolvedViewMode::Map);
        assert!(flags.wants_timeline);
    }
}
```

- [ ] **Step 2: Write the failing integration test for automatic timeline ordering when `view_mode` is omitted**

Add this test to `tests/service_integration.rs`:

```rust
#[tokio::test]
async fn assemble_context_auto_timeline_orders_results_without_explicit_view_mode() {
    use chrono::TimeZone;
    use memory_mcp::models::AssembleContextRequest;

    let service = common::make_service().await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas planning started",
        chrono::Utc.with_ymd_and_hms(2026, 1, 5, 9, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas budget increased",
        chrono::Utc.with_ymd_and_hms(2026, 2, 10, 9, 0, 0).unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "personal",
        "Atlas launch confirmed",
        chrono::Utc.with_ymd_and_hms(2026, 3, 20, 9, 0, 0).unwrap(),
    )
    .await;

    let items = service
        .assemble_context(AssembleContextRequest {
            query: "timeline of atlas changes in q1 2026".to_string(),
            scope: "personal".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble context");

    assert_eq!(items.len(), 3);
    assert!(items[0].content.contains("planning started"));
    assert!(items[1].content.contains("budget increased"));
    assert!(items[2].content.contains("launch confirmed"));
}
```

- [ ] **Step 3: Run the focused tests and confirm the failures are about missing query-mode logic**

Run:

```bash
cargo test detect_query_flags_marks_timeline_and_path_queries --lib
cargo test resolve_view_mode_prefers_explicit_value_over_auto_detection --lib
cargo test assemble_context_auto_timeline_orders_results_without_explicit_view_mode --test service_integration -- --nocapture
```

Expected:
- the lib tests fail because `detect_query_flags`, `ResolvedViewMode`, and `resolve_view_mode` do not exist yet;
- the integration test fails because `assemble_context` still uses standard relevance ordering when `view_mode` is omitted.

- [ ] **Step 4: Add the shared query-mode module and wire it before cache hits and pipeline dispatch**

Create `src/service/context/query_mode.rs` with this implementation:

```rust
use chrono::{DateTime, Utc};

use super::ranking;
use super::temporal::infer_temporal_window;

const TIMELINE_HINT_TERMS: &[&str] = &[
    "timeline",
    "history",
    "chronology",
    "changed",
    "changes",
    "progress",
    "sequence",
    "when",
];
const GRAPH_PATH_HINT_TERMS: &[&str] = &[
    "introduce",
    "intro",
    "connection",
    "connections",
    "connected",
    "path",
    "know",
    "knows",
];

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct QueryFlags {
    pub(super) wants_timeline: bool,
    pub(super) wants_graph_path: bool,
    pub(super) wants_graph_context: bool,
    pub(super) is_first_person_memory: bool,
}

impl QueryFlags {
    pub(super) fn max_graph_hops(&self) -> usize {
        if self.wants_graph_path { 2 } else { 1 }
    }

    pub(super) fn as_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        if self.wants_timeline {
            labels.push("timeline".to_string());
        }
        if self.wants_graph_path {
            labels.push("graph_path".to_string());
        }
        if self.wants_graph_context {
            labels.push("graph_context".to_string());
        }
        if self.is_first_person_memory {
            labels.push("first_person".to_string());
        }
        labels
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ResolvedViewMode {
    Standard,
    Timeline,
    Facets,
    WakeUp,
    Map,
}

impl ResolvedViewMode {
    pub(super) fn as_option_str(self) -> Option<&'static str> {
        match self {
            Self::Standard => None,
            Self::Timeline => Some("timeline"),
            Self::Facets => Some("facets"),
            Self::WakeUp => Some("wake_up"),
            Self::Map => Some("map"),
        }
    }
}

pub(super) fn query_phrase_candidates(query: &str) -> Vec<String> {
    let terms = query.split_whitespace().collect::<Vec<_>>();
    let mut phrases = Vec::new();

    for span_len in (1..=terms.len()).rev() {
        for start in 0..=terms.len().saturating_sub(span_len) {
            let phrase = terms[start..start + span_len].join(" ");
            if phrase.trim().len() >= 2 {
                phrases.push(phrase);
            }
        }
    }

    phrases.sort();
    phrases.dedup();
    phrases
}

pub(super) fn detect_query_flags(
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> QueryFlags {
    let normalized = raw_query_opt
        .map(crate::service::normalize_text)
        .unwrap_or_default();
    let has_temporal_focus = raw_query_opt
        .is_some_and(|query| infer_temporal_window(query, cutoff).is_some());
    let wants_timeline = has_temporal_focus
        && query_terms
            .iter()
            .any(|term| TIMELINE_HINT_TERMS.contains(&term.as_str()));
    let wants_graph_path = query_terms
        .iter()
        .any(|term| GRAPH_PATH_HINT_TERMS.contains(&term.as_str()));

    QueryFlags {
        wants_timeline,
        wants_graph_path,
        wants_graph_context: !normalized.is_empty(),
        is_first_person_memory: raw_query_opt.is_some_and(ranking::query_is_first_person_memory),
    }
}

pub(super) fn resolve_view_mode(
    explicit_view_mode: Option<&str>,
    raw_query_opt: Option<&str>,
    query_terms: &[String],
    cutoff: DateTime<Utc>,
) -> (ResolvedViewMode, QueryFlags) {
    let flags = detect_query_flags(raw_query_opt, query_terms, cutoff);

    let resolved = match explicit_view_mode.map(str::trim) {
        Some("timeline") => ResolvedViewMode::Timeline,
        Some("facets") => ResolvedViewMode::Facets,
        Some("wake_up") => ResolvedViewMode::WakeUp,
        Some("map") => ResolvedViewMode::Map,
        _ if flags.wants_timeline => ResolvedViewMode::Timeline,
        _ => ResolvedViewMode::Standard,
    };

    (resolved, flags)
}
```

Then update `src/service/context/alias_expansion.rs` to reuse `query_phrase_candidates()` instead of rebuilding phrase spans inline:

```rust
use super::query_mode::query_phrase_candidates;

pub(crate) async fn expand_query_with_aliases(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    let phrase_entries = query_phrase_candidates(query)
        .into_iter()
        .filter_map(|phrase| {
            let phrase_terms = phrase.split_whitespace().collect::<Vec<_>>();
            terms.windows(phrase_terms.len())
                .position(|window| window == phrase_terms)
                .map(|start| (phrase, start, start + phrase_terms.len()))
        })
        .collect::<Vec<_>>();

    let normalized_names = phrase_entries
        .iter()
        .map(|(phrase, _, _)| crate::service::normalize_text(phrase))
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let entities = service
        .db_client
        .select_entities_batch(namespace, &normalized_names)
        .await
        .unwrap_or_default();

    let mut entity_aliases = std::collections::HashMap::<String, Vec<String>>::new();
    for entity in &entities {
        let obj = match entity.as_object() {
            Some(obj) => obj,
            None => continue,
        };
        let canonical_norm = obj
            .get("canonical_name_normalized")
            .and_then(serde_json::Value::as_str)
            .map(String::from)
            .or_else(|| {
                obj.get("canonical_name")
                    .and_then(serde_json::Value::as_str)
                    .map(crate::service::normalize_text)
            })
            .unwrap_or_default();
        let aliases: Vec<String> = obj
            .get("aliases")
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
        if !canonical_norm.is_empty() && !aliases.is_empty() {
            entity_aliases.entry(canonical_norm).or_insert(aliases);
        }
    }

    let mut expanded = std::collections::HashSet::new();
    for (phrase, start, end) in &phrase_entries {
        let normalized = crate::service::normalize_text(phrase);
        if let Some(aliases) = entity_aliases.get(&normalized) {
            for alias_str in aliases {
                let mut parts: Vec<String> = terms[..*start]
                    .iter()
                    .map(|term| (*term).to_string())
                    .collect();
                parts.push(alias_str.clone());
                parts.extend(terms[*end..].iter().map(|term| (*term).to_string()));
                let alias_expanded = parts.join(" ");
                if alias_expanded != query {
                    expanded.insert(alias_expanded);
                }
            }
        }
    }

    expanded.into_iter().collect()
}
```

Finally, compute the resolved mode and flags in `src/service/context.rs` before the cache-hit early return and thread them into `DefaultContextParams`:

```rust
mod graph;
mod query_mode;

let cleaned_query = super::preprocess_search_query(&request.query);
let query_opt = if cleaned_query.is_empty() {
    None
} else {
    Some(cleaned_query.as_str())
};
let raw_query_opt = if request.query.trim().is_empty() {
    None
} else {
    Some(request.query.as_str())
};
let query_terms = query_opt
    .map(super::query::search_query_terms)
    .unwrap_or_default();
let requested_view_mode = request
    .view_mode
    .as_deref()
    .map(str::trim)
    .filter(|value| !value.is_empty());
let (resolved_view_mode, query_flags) = query_mode::resolve_view_mode(
    requested_view_mode,
    raw_query_opt,
    &query_terms,
    cutoff,
);
```

And extend `src/service/context/params.rs`:

```rust
use super::query_mode::QueryFlags;

pub(super) struct DefaultContextParams<'a> {
    pub(super) namespace: &'a str,
    pub(super) scope: &'a str,
    pub(super) cutoff_iso: &'a str,
    pub(super) cutoff: chrono::DateTime<chrono::Utc>,
    pub(super) raw_query_opt: Option<&'a str>,
    pub(super) query_opt: Option<&'a str>,
    pub(super) query_terms: &'a [String],
    pub(super) project_opt: Option<&'a str>,
    pub(super) fact_types: &'a [String],
    pub(super) budget: i32,
    pub(super) window_start: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) window_end: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) resolved_view_mode: Option<&'a str>,
    pub(super) query_flags: &'a QueryFlags,
    pub(super) access: &'a AccessContext,
}
```

- [ ] **Step 5: Run the focused tests again and verify timeline auto-routing now passes**

Run:

```bash
cargo test detect_query_flags_marks_timeline_and_path_queries --lib
cargo test resolve_view_mode_prefers_explicit_value_over_auto_detection --lib
cargo test assemble_context_auto_timeline_orders_results_without_explicit_view_mode --test service_integration -- --nocapture
```

Expected:
- the two lib tests PASS;
- the service integration test PASSes and returns the three facts in chronological order even with `view_mode=None`.

- [ ] **Step 6: Commit the query-mode layer**

Run:

```bash
git add src/service/context/query_mode.rs src/service/context.rs src/service/context/params.rs src/service/context/alias_expansion.rs tests/service_integration.rs
git commit -m "feat: detect retrieval intent for assemble_context"
```

- [ ] **Step 7: Re-run retrieval evals and compare against the baseline**

Auto-timeline routing changes retrieval ordering, so verify that the existing eval suites still pass and that the new `timeline_auto` fixture cases (added in Task 4) are already seeded in the fixture file.

Run:

```bash
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-post-task1-retrieval.txt
```

Compare the headline numbers against `docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline-summary.md`:

- `recall_at_5` should not decrease.
- `mrr` should not decrease.
- No existing tier pass rate should drop below its target.
- If any metric regresses, stop and diagnose before proceeding to Task 2.

Expected: all eval suites PASS at their existing target thresholds. The `timeline_auto` fixture cases will fail until Task 4 adds them, which is expected.

---

### Task 2: Add bounded entity-anchor graph expansion

**Files:**
- Create: `src/service/context/graph.rs`
- Modify: `src/service/context.rs`
- Modify: `src/service/context/pipeline.rs`
- Test: `src/service/context/graph.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Write the failing integration test for graph-only retrieval from a named entity anchor**

Add this test to `tests/service_integration.rs`:

```rust
#[tokio::test]
async fn assemble_context_graph_expansion_returns_anchor_neighbor_fact() {
    use chrono::TimeZone;
    use serde_json::json;

    let (service, db_client) = common::make_service_with_client().await;
    let t = chrono::Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

    common::seed_entity(
        &db_client,
        "org",
        "entity:alice",
        "person",
        "Alice Stone",
        &[],
    )
    .await;
    common::seed_entity(
        &db_client,
        "org",
        "entity:bob",
        "person",
        "Bob Chen",
        &[],
    )
    .await;
    db_client
        .relate_edge(
            "org",
            "edge:alice-bob",
            "entity:alice",
            "entity:bob",
            json!({
                "edge_id": "edge:alice-bob",
                "relation": "knows",
                "confidence": 0.9,
                "origin": "extracted",
                "t_valid": memory_mcp::service::normalize_dt(t),
                "t_ingested": memory_mcp::service::normalize_dt(t),
            }),
        )
        .await
        .expect("seed edge");
    common::seed_fact_with_links(
        &service,
        "org",
        "Bob Chen owns the Atlas launch checklist.",
        t,
        vec!["entity:bob".to_string()],
    )
    .await;

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Alice Stone".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble context");

    let graph_item = items
        .iter()
        .find(|item| item.retrieval_tier.as_deref() == Some("graph"))
        .expect("graph-expanded item should exist");
    assert!(graph_item.content.contains("Atlas launch checklist"));
}
```

- [ ] **Step 2: Write the failing unit test for shortest-hop tracking inside the BFS helper**

Add this test to `src/service/context/graph.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn insert_shortest_hop_keeps_the_smallest_depth_for_each_entity() {
        let mut traces = HashMap::new();

        assert!(insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 2,
                path: vec!["entity:alice".to_string(), "entity:bob".to_string()],
            },
        ));
        assert!(!insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 3,
                path: vec![
                    "entity:alice".to_string(),
                    "episode:1".to_string(),
                    "entity:bob".to_string(),
                ],
            },
        ));
        assert!(insert_shortest_hop(
            &mut traces,
            "entity:bob",
            GraphTrace {
                anchor_entity_id: "entity:alice".to_string(),
                anchor_canonical_name: "Alice Stone".to_string(),
                hop_count: 1,
                path: vec!["entity:alice".to_string(), "entity:bob".to_string()],
            },
        ));

        assert_eq!(traces.get("entity:bob").map(|trace| trace.hop_count), Some(1));
    }
}
```

- [ ] **Step 3: Run the focused tests and confirm graph expansion is still missing from the default pipeline**

Run:

```bash
cargo test insert_shortest_hop_keeps_the_smallest_depth_for_each_entity --lib
cargo test assemble_context_graph_expansion_returns_anchor_neighbor_fact --test service_integration -- --nocapture
```

Expected:
- the lib test fails because `GraphTrace` and `insert_shortest_hop()` do not exist yet;
- the integration test fails because `assemble_context("Alice Stone")` returns no `graph` tier result.

- [ ] **Step 4: Add the graph collector module and resolve anchors from query phrases plus lexical facts**

Create `src/service/context/graph.rs` with this implementation:

```rust
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use chrono::{DateTime, Utc};
use serde_json::Value;

use crate::models::Fact;
use crate::service::error::MemoryError;
use crate::storage::GraphDirection;

use super::filtering::filter_facts_by_constraints;
use super::query_mode::query_phrase_candidates;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct GraphTrace {
    pub(super) anchor_entity_id: String,
    pub(super) anchor_canonical_name: String,
    pub(super) hop_count: usize,
    pub(super) path: Vec<String>,
}

#[derive(Debug, Clone)]
pub(super) struct GraphCandidate {
    pub(super) fact: Fact,
    pub(super) rationale: String,
    pub(super) origin_factor: f64,
    pub(super) trace: GraphTrace,
}

pub(super) struct CollectGraphFactsRequest<'a> {
    pub(super) namespace: &'a str,
    pub(super) scope: &'a str,
    pub(super) cutoff_iso: &'a str,
    pub(super) cutoff: DateTime<Utc>,
    pub(super) raw_query: &'a str,
    pub(super) access: &'a crate::models::AccessContext,
    pub(super) project: Option<&'a str>,
    pub(super) fact_types: &'a [String],
    pub(super) direct_fact_ids: &'a HashSet<String>,
    pub(super) lexical_facts: &'a [Fact],
    pub(super) max_hops: usize,
    pub(super) budget: i32,
}

fn entity_anchor_from_value(value: &Value) -> Option<(String, String)> {
    let map = value.as_object()?;
    let entity_id = map
        .get("entity_id")
        .and_then(crate::service::episode::unwrap_record_string)
        .or_else(|| map.get("id").and_then(crate::service::episode::unwrap_record_string))?;
    let canonical_name = map
        .get("canonical_name")
        .and_then(crate::service::episode::unwrap_record_string)
        .unwrap_or_else(|| entity_id.clone());
    Some((entity_id, canonical_name))
}

fn insert_shortest_hop(
    traces: &mut HashMap<String, GraphTrace>,
    entity_id: &str,
    trace: GraphTrace,
) -> bool {
    match traces.get(entity_id) {
        Some(existing) if existing.hop_count <= trace.hop_count => false,
        _ => {
            traces.insert(entity_id.to_string(), trace);
            true
        }
    }
}

async fn resolve_query_anchor_entities(
    service: &crate::service::MemoryService,
    namespace: &str,
    raw_query: &str,
    lexical_facts: &[Fact],
) -> Result<BTreeMap<String, String>, MemoryError> {
    let normalized_names = query_phrase_candidates(raw_query)
        .into_iter()
        .map(|phrase| crate::service::normalize_text(&phrase))
        .filter(|phrase| !phrase.is_empty())
        .collect::<Vec<_>>();
    let mut anchors = service
        .db_client
        .select_entities_batch(namespace, &normalized_names)
        .await?
        .into_iter()
        .filter_map(|value| entity_anchor_from_value(&value))
        .collect::<BTreeMap<_, _>>();

    for fact in lexical_facts {
        for entity_id in &fact.entity_links {
            anchors
                .entry(entity_id.clone())
                .or_insert_with(|| entity_id.clone());
        }
    }

    Ok(anchors)
}

async fn walk_anchor_entities(
    service: &crate::service::MemoryService,
    namespace: &str,
    cutoff_iso: &str,
    anchors: &BTreeMap<String, String>,
    max_hops: usize,
) -> Result<HashMap<String, GraphTrace>, MemoryError> {
    let mut traces = HashMap::<String, GraphTrace>::new();
    let mut queue = VecDeque::new();

    for (entity_id, canonical_name) in anchors {
        let trace = GraphTrace {
            anchor_entity_id: entity_id.clone(),
            anchor_canonical_name: canonical_name.clone(),
            hop_count: 0,
            path: vec![entity_id.clone()],
        };
        insert_shortest_hop(&mut traces, entity_id, trace.clone());
        queue.push_back((entity_id.clone(), trace));
    }

    while let Some((current_entity, current_trace)) = queue.pop_front() {
        if current_trace.hop_count >= max_hops {
            continue;
        }

        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            for edge in service
                .db_client
                .select_edge_neighbors(namespace, &current_entity, cutoff_iso, direction)
                .await?
            {
                let Some(map) = edge.as_object() else {
                    continue;
                };
                let in_id = map
                    .get("in")
                    .and_then(crate::service::episode::unwrap_record_string);
                let out_id = map
                    .get("out")
                    .and_then(crate::service::episode::unwrap_record_string);
                let neighbor = match (in_id.as_deref(), out_id.as_deref()) {
                    (Some(left), Some(right)) if left == current_entity => Some(right.to_string()),
                    (Some(left), Some(right)) if right == current_entity => Some(left.to_string()),
                    _ => None,
                };
                let Some(neighbor) = neighbor else {
                    continue;
                };
                if !neighbor.starts_with("entity:") {
                    continue;
                }

                let mut next_path = current_trace.path.clone();
                next_path.push(neighbor.clone());
                let next_trace = GraphTrace {
                    anchor_entity_id: current_trace.anchor_entity_id.clone(),
                    anchor_canonical_name: current_trace.anchor_canonical_name.clone(),
                    hop_count: current_trace.hop_count + 1,
                    path: next_path,
                };

                if insert_shortest_hop(&mut traces, &neighbor, next_trace.clone()) {
                    queue.push_back((neighbor, next_trace));
                }
            }
        }
    }

    Ok(traces)
}

pub(super) async fn collect_graph_facts(
    service: &crate::service::MemoryService,
    request: CollectGraphFactsRequest<'_>,
) -> Result<Vec<GraphCandidate>, MemoryError> {
    if request.raw_query.trim().is_empty() || request.max_hops == 0 {
        return Ok(Vec::new());
    }

    let anchors = resolve_query_anchor_entities(
        service,
        request.namespace,
        request.raw_query,
        request.lexical_facts,
    )
    .await?;
    if anchors.is_empty() {
        return Ok(Vec::new());
    }

    let traces = walk_anchor_entities(
        service,
        request.namespace,
        request.cutoff_iso,
        &anchors,
        request.max_hops,
    )
    .await?;
    let entity_ids = traces.keys().cloned().collect::<Vec<_>>();
    let records = service
        .db_client
        .select_facts_by_entity_links(
            request.namespace,
            request.scope,
            request.cutoff_iso,
            &entity_ids,
            request.budget.max(1) * 4,
        )
        .await?;

    let mut facts = filter_facts_by_constraints(
        records,
        request.access,
        request.project,
        request.fact_types,
    )
    .into_iter()
    .filter(|fact| fact.scope == request.scope)
    .filter(|fact| !request.direct_fact_ids.contains(&fact.fact_id))
    .collect::<Vec<_>>();
    facts.sort_by(|left, right| {
        right
            .t_valid
            .cmp(&left.t_valid)
            .then_with(|| left.fact_id.cmp(&right.fact_id))
    });

    Ok(facts
        .into_iter()
        .filter_map(|fact| {
            let trace = fact
                .entity_links
                .iter()
                .filter_map(|entity_id| traces.get(entity_id))
                .min_by(|left, right| {
                    left.hop_count
                        .cmp(&right.hop_count)
                        .then_with(|| left.anchor_entity_id.cmp(&right.anchor_entity_id))
                })?
                .clone();

            Some(GraphCandidate {
                rationale: format!(
                    "matched graph anchor={} hop_count={} path={}",
                    trace.anchor_canonical_name,
                    trace.hop_count,
                    trace.path.join(" -> ")
                ),
                origin_factor: 1.0,
                trace,
                fact,
            })
        })
        .take(request.budget.max(1) as usize)
        .collect())
}
```

- [ ] **Step 5: Extend `build_ranked_context_facts` to accept graph candidates as a new second parameter**

The pipeline code in Step 6 will call `build_ranked_context_facts(lexical_facts, graph_facts, community_facts, semantic_facts, ...)`. Update the function signature in `src/service/context/ranking.rs` now, before the pipeline integration, so the code compiles. Add the `graph_facts` parameter and a simple insertion loop that treats graph candidates like community facts (same reciprocal rank weight, no hop-aware scoring yet — that comes in Task 3):

```rust
use super::graph::GraphCandidate;

pub(crate) fn build_ranked_context_facts(
    lexical_facts: Vec<(Fact, RetrievalTier)>,
    graph_facts: Vec<GraphCandidate>,
    community_facts: Vec<(Fact, String, f64)>,
    semantic_facts: Vec<(Fact, String)>,
    query_opt: Option<&str>,
    semantic_available: bool,
    scope: &str,
    cutoff: DateTime<Utc>,
    decayed_fn: impl Fn(&Fact, DateTime<Utc>) -> f64,
) -> Vec<RankedContextFact> {
    let mut ranked_by_fact_id = HashMap::<String, RankedContextFact>::new();
    let query_alignment = |fact: &Fact| query_alignment_factor(query_opt, fact);
    let grounding = |fact: &Fact| query_grounding_score(query_opt, fact);
    let lexical_query_terms = query_opt.map(search_query_terms).unwrap_or_default();

    for (rank, (fact, retrieval_tier)) in lexical_facts.into_iter().enumerate() {
        // existing lexical ranking loop stays the same
    }

    // Insert graph candidates with reciprocal rank weight (hop-aware weights come in Task 3)
    for (rank, candidate) in graph_facts.into_iter().enumerate() {
        let fact_id = candidate.fact.fact_id.clone();
        let confidence = decayed_fn(&candidate.fact, cutoff);
        let query_alignment_factor = query_alignment(&candidate.fact);
        let grounding_score = grounding(&candidate.fact);
        let weighted_rank = reciprocal_rank(rank);

        if let Some(existing) = ranked_by_fact_id.get_mut(&fact_id) {
            existing.fusion_score += weighted_rank;
            existing.decayed_confidence = existing.decayed_confidence.max(confidence);
            existing.query_alignment_factor = existing
                .query_alignment_factor
                .max(query_alignment_factor);
            existing.grounding_score = existing.grounding_score.max(grounding_score);
            continue;
        }

        ranked_by_fact_id.insert(
            fact_id,
            RankedContextFact {
                rationale: candidate.rationale,
                fact: candidate.fact,
                retrieval_tier: RetrievalTier::GraphExpanded,
                fusion_score: weighted_rank,
                source_priority: 1,
                decayed_confidence: confidence,
                query_alignment_factor,
                grounding_score,
                semantic_available,
            },
        );
    }

    // community and semantic loops stay the same below
```

Also add the `mod graph;` declaration inside `src/service/context/ranking.rs` at the top (or import `GraphCandidate` via `use super::graph::GraphCandidate;`).

- [ ] **Step 6: Integrate graph expansion into the default retrieval pipeline after lexical retrieval and before community/semantic expansion**

Update `src/service/context/pipeline.rs` by inserting the graph candidate collection between the experience-fact merge and community collection. In the query path, change the `all_direct_ids` set to include graph facts, and in the `build_ranked_context_facts` call pass graph candidates as the second argument. The updated function body becomes:

```rust
use super::graph::{CollectGraphFactsRequest, collect_graph_facts};

pub(super) async fn assemble_default_context(
    service: &MemoryService,
    params: DefaultContextParams<'_>,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let lexical_result = select_fact_records_for_query(
        service,
        FactQueryParams {
            namespace: params.namespace,
            scope: params.scope,
            cutoff_iso: params.cutoff_iso,
            query_opt: params.query_opt,
            limit: params.budget,
            project: params.project_opt,
            fact_types: params.fact_types,
        },
    )
    .await?;

    let direct_retrieval_tier = lexical_result.retrieval_tier;
    let mut direct_facts = filter_facts_by_constraints(
        lexical_result.records,
        params.access,
        params.project_opt,
        params.fact_types,
    );

    let mut expanded_facts = Vec::new();
    let mut ranked_facts = if let Some(query) = params.query_opt {
        let temporal_facts = collect_temporal_facts(
            service,
            CollectTemporalFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff_iso: params.cutoff_iso,
                cutoff: params.cutoff,
                query,
                access: params.access,
                project: params.project_opt,
                fact_types: params.fact_types,
                budget: params.budget,
            },
        )
        .await?;

        let expanded_queries = expand_query_with_aliases(service, query, params.namespace).await;
        let direct_fact_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        for expanded_query in &expanded_queries {
            if expanded_query == query {
                continue;
            }
            let extra_records = select_fact_records_for_query(
                service,
                FactQueryParams {
                    namespace: params.namespace,
                    scope: params.scope,
                    cutoff_iso: params.cutoff_iso,
                    query_opt: Some(expanded_query),
                    limit: params.budget,
                    project: params.project_opt,
                    fact_types: params.fact_types,
                },
            )
            .await?;
            for fact in filter_facts_by_constraints(
                extra_records.records,
                params.access,
                params.project_opt,
                params.fact_types,
            ) {
                if !direct_fact_ids.contains(&fact.fact_id) {
                    expanded_facts.push(fact);
                }
            }
        }
        let base_direct_ids: HashSet<_> = direct_facts
            .iter()
            .chain(temporal_facts.iter())
            .chain(expanded_facts.iter())
            .map(|fact| fact.fact_id.clone())
            .collect();

        let experience_query_terms =
            expand_experience_query_terms(params.query_terms, &direct_facts);
        let experience_topic_terms = experience_query_terms
            .iter()
            .filter(|term| !params.query_terms.contains(term))
            .cloned()
            .collect::<Vec<_>>();
        let mut experience_facts = collect_recent_experience_facts(
            service,
            RecentExperienceRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff: params.cutoff,
                project: params.project_opt,
                access: params.access,
                budget: params.budget,
                fact_types: params.fact_types,
            },
            &experience_query_terms,
            &experience_topic_terms,
            &base_direct_ids,
        )
        .await?;

        if !experience_topic_terms.is_empty() {
            let topical_floor = direct_facts
                .first()
                .map(|fact| fact.ft_score)
                .unwrap_or(0.0);
            for fact in &mut experience_facts {
                fact.ft_score = fact.ft_score.max(topical_floor + 1.0);
            }
        }

        direct_facts.extend(experience_facts);
        direct_facts.sort_by(|left, right| {
            right
                .ft_score
                .total_cmp(&left.ft_score)
                .then_with(|| right.t_valid.cmp(&left.t_valid))
                .then_with(|| left.fact_id.cmp(&right.fact_id))
        });

        let graph_facts = collect_graph_facts(
            service,
            CollectGraphFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff_iso: params.cutoff_iso,
                cutoff: params.cutoff,
                raw_query: query,
                access: params.access,
                project: params.project_opt,
                fact_types: params.fact_types,
                direct_fact_ids: &base_direct_ids,
                lexical_facts: &direct_facts,
                max_hops: params.query_flags.max_graph_hops(),
                budget: params.budget,
            },
        )
        .await?;

        let all_direct_ids: HashSet<_> = base_direct_ids
            .iter()
            .cloned()
            .chain(
                graph_facts
                    .iter()
                    .map(|candidate| candidate.fact.fact_id.clone()),
            )
            .collect();

        let community_facts = collect_community_facts(
            service,
            CollectCommunityFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff_iso: params.cutoff_iso,
                query,
                access: params.access,
                project: params.project_opt,
                fact_types: params.fact_types,
                direct_fact_ids: &all_direct_ids,
                budget: params.budget,
            },
        )
        .await?;

        let excluded_fact_ids = all_direct_ids
            .iter()
            .cloned()
            .chain(
                community_facts
                    .iter()
                    .map(|(fact, _, _)| fact.fact_id.clone()),
            )
            .collect::<HashSet<_>>();

        let semantic_facts = collect_semantic_facts(
            service,
            CollectSemanticFactsRequest {
                namespace: params.namespace,
                scope: params.scope,
                cutoff: params.cutoff,
                query,
                access: params.access,
                project: params.project_opt,
                fact_types: params.fact_types,
                excluded_fact_ids: &excluded_fact_ids,
                budget: params.budget,
            },
        )
        .await?;

        let mut lexical_facts = direct_facts
            .into_iter()
            .map(|fact| (fact, direct_retrieval_tier))
            .collect::<Vec<_>>();
        lexical_facts.extend(
            temporal_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::TemporalExpanded)),
        );
        lexical_facts.extend(
            expanded_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::AliasExpanded)),
        );

        build_ranked_context_facts(
            lexical_facts,
            graph_facts,
            community_facts,
            semantic_facts,
            params.raw_query_opt,
            service.embedding_provider.is_enabled(),
            params.scope,
            params.cutoff,
            decayed_confidence,
        )
    } else {
        build_ranked_context_facts(
            direct_facts
                .into_iter()
                .map(|fact| (fact, RetrievalTier::Direct))
                .collect(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            params.raw_query_opt,
            service.embedding_provider.is_enabled(),
            params.scope,
            params.cutoff,
            decayed_confidence,
        )
    };

    let episode_fallback_items = if let Some(query) = params.query_opt {
        collect_episode_fallback_items(service, &params, query).await?
    } else {
        Vec::new()
    };

    if ranked_facts.is_empty() {
        if params.query_opt.is_some() {
            return Ok(episode_fallback_items);
        }

        unreachable!("ranked_facts is empty but no query provided")
    }

    apply_time_window(&mut ranked_facts, params.window_start, params.window_end);
    let ranked_candidates = ranked_facts.clone();
    let selected_ranked = if params.resolved_view_mode == Some("timeline") {
        sort_ranked_context_facts_for_timeline(&mut ranked_facts);
        ranked_facts
            .into_iter()
            .take(params.budget.max(1) as usize)
            .collect::<Vec<_>>()
    } else {
        let temporal_focus = params
            .query_opt
            .and_then(|query| infer_temporal_window(query, params.cutoff));
        select_ranked_context_facts(
            ranked_facts,
            params.budget.max(1) as usize,
            temporal_focus,
            params.query_terms.to_vec(),
        )
    };

    let prefer_episode_content = should_prefer_episode_content(
        &selected_ranked,
        &episode_fallback_items,
        params.query_terms,
    );

    if params.query_opt.is_some() {
        service.logger.log(
            log_event(
                "assemble_context.episode_rescue",
                json!({"scope": params.scope, "query": params.query_opt}),
                build_episode_rescue_log_result(
                    episode_fallback_items.len(),
                    selected_ranked.len(),
                    prefer_episode_content,
                ),
                Some(params.access),
                None,
                None,
            ),
            LogLevel::Debug,
        );
    }

    if prefer_episode_content {
        return Ok(episode_fallback_items);
    }

    let selected_terms = selected_fact_matched_terms(&selected_ranked, params.query_terms);
    let mut results = selected_ranked
        .into_iter()
        .map(|ranked| ranked_fact_to_item(ranked, params.cutoff, decayed_confidence))
        .collect::<Vec<_>>();

    maybe_append_first_person_episode_item(
        &mut results,
        &episode_fallback_items,
        &selected_terms,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
    );

    maybe_append_first_person_ranked_fact_item(
        &mut results,
        &ranked_candidates,
        params.raw_query_opt,
        params.query_terms,
        params.budget.max(1) as usize,
        params.cutoff,
    );

    Ok(results)
}
```

In this updated function, the timeline sorting now uses `params.resolved_view_mode` instead of the raw `params.view_mode`, and the non-query path passes `Vec::new()` for the new graph-facts parameter of `build_ranked_context_facts()`.


- [ ] **Step 7: Run the focused tests again and verify graph expansion can now surface anchor-neighbor facts**

Run:

```bash
cargo test insert_shortest_hop_keeps_the_smallest_depth_for_each_entity --lib
cargo test assemble_context_graph_expansion_returns_anchor_neighbor_fact --test service_integration -- --nocapture
```

Expected:
- the lib test PASSes;
- the service integration test PASSes and returns the Bob-linked fact with `retrieval_tier="graph"`.

- [ ] **Step 8: Commit the graph-expansion collector**

Run:

```bash
git add src/service/context/graph.rs src/service/context.rs src/service/context/pipeline.rs tests/service_integration.rs
git commit -m "feat: add entity-anchor graph expansion"
```

- [ ] **Step 9: Re-run retrieval evals and compare against baseline and post-Task-1 results**

Graph expansion adds a new retrieval tier (`graph`) that didn't exist before. Verify that:

1. Existing tiers don't regress.
2. The `graph` tier shows up in actual tier counts.
3. External eval suites still pass.

Run:

```bash
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-post-task2-retrieval.txt
MEMORY_MCP_EVAL_MAX_CASES=100 cargo test --test eval_external_retrieval -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-post-task2-retrieval.txt
```

Compare against `docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline-summary.md` and `docs/superpowers/plans/baselines/2026-04-30-post-task1-retrieval.txt`:

- `recall_at_5` should not decrease vs baseline.
- `mrr` should not decrease vs baseline.
- The `graph` tier should appear in `actual_tier` counts where entity-anchor queries are present.
- External LongMemEval/LoCoMo `recall_at_5` and `mrr` should not regress.
- If any metric regresses, stop and diagnose before proceeding to Task 3.

Expected: all eval suites PASS at their existing target thresholds. The `graph_anchor` fixture cases will fail until Task 4 adds them, which is expected.

---

### Task 3: Make graph ranking depth-aware and explainable

**Files:**
- Modify: `src/service/context/ranking.rs`
- Modify: `src/service/context/scoring.rs`
- Modify: `src/service/context/graph.rs`
- Test: `src/service/context/ranking.rs`
- Test: `tests/service_acceptance.rs`

- [ ] **Step 1: Write the failing unit test that a one-hop graph candidate outranks an equally-scored two-hop graph candidate**

Add this test to `src/service/context/ranking.rs`:

```rust
#[test]
fn graph_rank_weight_penalizes_deeper_matches() {
    let one_hop = graph_rank_weight(0, 1, 1.0);
    let two_hop = graph_rank_weight(0, 2, 1.0);
    let weak_one_hop = graph_rank_weight(3, 1, 0.5);

    assert!(one_hop > two_hop);
    assert!(one_hop > weak_one_hop);
}
```

- [ ] **Step 2: Extend the existing acceptance test so graph results must expose anchor and hop trace metadata**

Update `tests/service_acceptance.rs` by adding this acceptance test near `test_assemble_context_exposes_retrieval_tier_and_rationale_metadata()`:

```rust
#[tokio::test]
async fn test_assemble_context_graph_results_include_anchor_and_hop_trace() {
    use chrono::TimeZone;
    use serde_json::json;

    let (service, db_client) = common::make_service_with_client().await;
    let t = Utc.with_ymd_and_hms(2026, 4, 30, 12, 0, 0).unwrap();

    common::seed_entity(&db_client, "personal", "entity:alice", "person", "Alice Stone", &[]).await;
    common::seed_entity(&db_client, "personal", "entity:bob", "person", "Bob Chen", &[]).await;
    db_client
        .relate_edge(
            "personal",
            "edge:alice-bob",
            "entity:alice",
            "entity:bob",
            json!({
                "edge_id": "edge:alice-bob",
                "relation": "knows",
                "confidence": 0.9,
                "origin": "extracted",
                "t_valid": memory_mcp::service::normalize_dt(t),
                "t_ingested": memory_mcp::service::normalize_dt(t),
            }),
        )
        .await
        .expect("seed edge");
    common::seed_fact_with_links(
        &service,
        "personal",
        "Bob Chen owns the Atlas launch checklist.",
        t,
        vec!["entity:bob".to_string()],
    )
    .await;

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Alice Stone".to_string(),
            scope: "personal".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble context");

    let graph_item = items
        .iter()
        .find(|item| item.retrieval_tier.as_deref() == Some("graph"))
        .expect("graph-expanded item should exist");

    assert!(graph_item.rationale.contains("anchor=Alice Stone"));
    assert!(graph_item.rationale.contains("hops=1"));
    assert_eq!(
        graph_item
            .provenance
            .get("graph_trace")
            .and_then(|value| value.get("hop_count"))
            .and_then(|value| value.as_u64()),
        Some(1),
    );
}
```

- [ ] **Step 3: Run the focused tests and confirm the graph-score helper and provenance trace are still missing**

Run:

```bash
cargo test graph_rank_weight_penalizes_deeper_matches --lib
cargo test test_assemble_context_graph_results_include_anchor_and_hop_trace --test service_acceptance -- --nocapture
```

Expected:
- the lib test fails because `graph_rank_weight()` does not exist yet;
- the acceptance test fails because graph items do not yet expose `graph_trace` provenance or anchor/hop rationale.

- [ ] **Step 4: Add hop-aware graph weighting and carry graph traces through `RankedContextFact` into `provenance`**

Update `src/service/context/ranking.rs` like this:

```rust
const TWO_HOP_GRAPH_WEIGHT: f64 = 0.72;
const DEEP_GRAPH_WEIGHT: f64 = 0.55;

#[derive(Debug, Clone)]
pub(crate) struct RankedContextFact {
    pub(crate) fact: Fact,
    pub(crate) rationale: String,
    pub(crate) retrieval_tier: RetrievalTier,
    pub(crate) fusion_score: f64,
    pub(crate) source_priority: u8,
    pub(crate) decayed_confidence: f64,
    pub(crate) query_alignment_factor: f64,
    pub(crate) grounding_score: f64,
    pub(crate) semantic_available: bool,
    pub(crate) matched_query_terms: Vec<String>,
    pub(crate) graph_trace: Option<crate::service::context::graph::GraphTrace>,
}

fn graph_rank_weight(rank: usize, hop_count: usize, origin_factor: f64) -> f64 {
    let hop_weight = match hop_count {
        0 | 1 => 1.0,
        2 => TWO_HOP_GRAPH_WEIGHT,
        _ => DEEP_GRAPH_WEIGHT,
    };
    reciprocal_rank(rank) * hop_weight * origin_factor.clamp(0.0, 1.0)
}
```

Then, update the `build_ranked_context_facts` function signature to accept graph candidates as the second parameter:

```rust
pub(crate) fn build_ranked_context_facts(
    lexical_facts: Vec<(Fact, RetrievalTier)>,
    graph_facts: Vec<GraphCandidate>,
    community_facts: Vec<(Fact, String, f64)>,
    semantic_facts: Vec<(Fact, String)>,
    query_opt: Option<&str>,
    semantic_available: bool,
    scope: &str,
    cutoff: DateTime<Utc>,
    decayed_fn: impl Fn(&Fact, DateTime<Utc>) -> f64,
) -> Vec<RankedContextFact> {
```

And insert graph candidates using `graph_rank_weight()` right after the lexical-fact ranking loop (before community/semantic):

```rust
for (rank, candidate) in graph_facts.into_iter().enumerate() {
    let fact_id = candidate.fact.fact_id.clone();
    let confidence = decayed_fn(&candidate.fact, cutoff);
    let query_alignment_factor = query_alignment(&candidate.fact);
    let grounding_score = grounding(&candidate.fact);
    let matched_terms = matched_terms_for_fact(query_opt, &candidate.fact);
    let weighted_rank = graph_rank_weight(
        rank,
        candidate.trace.hop_count,
        candidate.origin_factor,
    );

    if let Some(existing) = ranked_by_fact_id.get_mut(&fact_id) {
        existing.fusion_score += weighted_rank;
        existing.decayed_confidence = existing.decayed_confidence.max(confidence);
        existing.query_alignment_factor = existing
            .query_alignment_factor
            .max(query_alignment_factor);
        existing.grounding_score = existing.grounding_score.max(grounding_score);
        if existing.graph_trace.is_none()
            || existing
                .graph_trace
                .as_ref()
                .is_some_and(|trace| trace.hop_count > candidate.trace.hop_count)
        {
            existing.graph_trace = Some(candidate.trace.clone());
            existing.rationale = format!(
                "tier=graph anchor={} hops={} {}",
                candidate.trace.anchor_canonical_name,
                candidate.trace.hop_count,
                candidate.rationale,
            );
        }
        continue;
    }

    ranked_by_fact_id.insert(
        fact_id,
        RankedContextFact {
            rationale: format!(
                "tier=graph anchor={} hops={} {}",
                candidate.trace.anchor_canonical_name,
                candidate.trace.hop_count,
                candidate.rationale,
            ),
            fact: candidate.fact,
            retrieval_tier: RetrievalTier::GraphExpanded,
            fusion_score: weighted_rank,
            source_priority: 1,
            decayed_confidence: confidence,
            query_alignment_factor,
            grounding_score,
            semantic_available,
            matched_query_terms: matched_terms,
            graph_trace: Some(candidate.trace),
        },
    );
}
```

Add this helper near the existing query-term helpers in `ranking.rs`:

```rust
fn matched_terms_for_fact(query_opt: Option<&str>, fact: &Fact) -> Vec<String> {
    let Some(query) = query_opt else {
        return Vec::new();
    };

    let query_terms = unique_query_terms(&search_query_terms(query));
    let fact_terms = fact_term_set(fact);
    query_terms
        .into_iter()
        .filter(|term| fact_terms.contains(term.as_str()))
        .collect()
}
```

And update `src/service/context/scoring.rs` so the trace survives in the response provenance:

```rust
use serde_json::json;

pub(super) fn ranked_fact_to_item(
    ranked: ranking::RankedContextFact,
    cutoff: chrono::DateTime<chrono::Utc>,
    decay_fn: impl FnOnce(&Fact, chrono::DateTime<chrono::Utc>) -> f64,
) -> AssembledContextItem {
    let relevance = ranking::normalized_relevance_score(&ranked);
    let grounding = ranked.grounding_score;
    let semantic_available = ranked.semantic_available;
    let confidence = decay_fn(&ranked.fact, cutoff);
    let mut provenance = ranked.fact.provenance;

    if let Some(map) = provenance.as_object_mut() {
        if !ranked.matched_query_terms.is_empty() {
            map.insert(
                "matched_query_terms".to_string(),
                json!(ranked.matched_query_terms),
            );
        }
        if let Some(trace) = ranked.graph_trace.as_ref() {
            map.insert(
                "graph_trace".to_string(),
                json!({
                    "anchor_entity_id": trace.anchor_entity_id,
                    "anchor_canonical_name": trace.anchor_canonical_name,
                    "hop_count": trace.hop_count,
                    "path": trace.path,
                }),
            );
        }
    }

    AssembledContextItem {
        fact_id: ranked.fact.fact_id,
        content: ranked.fact.content,
        quote: ranked.fact.quote,
        source_episode: ranked.fact.source_episode,
        confidence,
        relevance: Some(relevance),
        grounding: Some(grounding),
        semantic_available: Some(semantic_available),
        provenance,
        rationale: ranked.rationale,
        retrieval_tier: Some(ranked.retrieval_tier.as_str().to_string()),
    }
}
```

- [ ] **Step 5: Run the focused tests again and verify graph ranking is now deterministic and traceable**

Run:

```bash
cargo test graph_rank_weight_penalizes_deeper_matches --lib
cargo test test_assemble_context_graph_results_include_anchor_and_hop_trace --test service_acceptance -- --nocapture
```

Expected:
- the lib test PASSes and proves one-hop graph matches are weighted above deeper ones;
- the acceptance test PASSes and the graph item carries both rationale text and `provenance.graph_trace` metadata.

- [ ] **Step 6: Commit the graph-ranking and provenance pass**

Run:

```bash
git add src/service/context/ranking.rs src/service/context/scoring.rs src/service/context/graph.rs tests/service_acceptance.rs
git commit -m "feat: add depth-aware graph ranking traces"
```

- [ ] **Step 7: Re-run retrieval evals and compare against baseline and post-Task-2 results**

Depth-aware graph weighting directly changes ranking scores. Verify no regression:

Run:

```bash
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-post-task3-retrieval.txt
MEMORY_MCP_EVAL_MAX_CASES=100 cargo test --test eval_external_retrieval -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-post-task3-retrieval.txt
```

Compare against `docs/superpowers/plans/baselines/2026-04-30-retrieval-baseline-summary.md` and `docs/superpowers/plans/baselines/2026-04-30-post-task2-retrieval.txt`:

- `recall_at_5` and `mrr` should not decrease vs baseline.
- `graph` tier pass rate (once fixtures exist in Task 4) should meet its target.
- `top1_hit_rate` should not decrease — hop-aware weighting must not push the best answer out of position 1.
- If any metric regresses, stop and diagnose before proceeding to Task 4.

Expected: all eval suites PASS at their existing target thresholds.

---

### Task 4: Persist retrieval diagnostics and extend eval slices

**Files:**
- Create: `migrations/020_query_log_retrieval_diagnostics.surql`
- Modify: `src/storage/migrations.rs`
- Modify: `src/service/context/logging.rs`
- Modify: `src/service/context.rs`
- Modify: `tests/service_integration.rs`
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Modify: `tests/eval_retrieval.rs`
- Modify: `tests/fixtures/evals/retrieval_cases.json`

- [ ] **Step 1: Write the failing integration test that `query_log` stores resolved view mode, query flags, and tier distribution**

Add this test to `tests/service_integration.rs`:

```rust
#[tokio::test]
async fn test_service_assemble_context_records_query_log_with_resolved_view_mode_and_flags() {
    let (mut service, db_client) = common::make_service_with_client_and_query_logging(true).await;
    service = service.with_query_log_retention_days(30);

    common::seed_fact_at(
        &service,
        "org",
        "Atlas budget increased in January 2026",
        "2026-01-10T09:00:00Z".parse().unwrap(),
    )
    .await;
    common::seed_fact_at(
        &service,
        "org",
        "Atlas launch confirmed in March 2026",
        "2026-03-10T09:00:00Z".parse().unwrap(),
    )
    .await;

    let _ = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "timeline of atlas changes in q1 2026".to_string(),
            scope: "org".to_string(),
            as_of: None,
            budget: 5,
            project: None,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
        })
        .await
        .expect("assemble context");

    let query_logs = db_client.select_table("query_log", "org").await.unwrap();
    let row = query_logs.first().expect("query_log row should exist");

    assert_eq!(
        row.get("resolved_view_mode").and_then(|value| value.as_str()),
        Some("timeline"),
    );
    let flags = row
        .get("query_flags")
        .and_then(|value| value.as_array())
        .expect("query_flags should be stored as an array");
    assert!(flags.iter().any(|value| value.as_str() == Some("timeline")));
    assert!(
        row.get("retrieval_tiers")
            .and_then(|value| value.as_object())
            .is_some(),
        "retrieval_tiers distribution should be recorded",
    );
}
```

- [ ] **Step 2: Write the failing eval coverage test for tagged retrieval slices and add the three concrete regression cases**

First, update `tests/eval_retrieval.rs` so cases can carry tags and the fixture must include at least one case for each new slice:

```rust
#[derive(Debug, Deserialize)]
struct RetrievalEvalCase {
    id: String,
    description: String,
    query: String,
    scope: String,
    #[serde(default)]
    project: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default = "default_budget")]
    budget: i32,
    facts: Vec<SeedFact>,
    #[serde(default)]
    entities: Vec<SeedEntity>,
    #[serde(default)]
    communities: Vec<SeedCommunity>,
    expected: RetrievalExpectation,
}

#[test]
fn retrieval_fixture_provides_graph_and_timeline_tag_coverage() {
    let cases = load_cases();

    for tag in ["timeline_auto", "graph_anchor", "first_person_rescue"] {
        let count = cases
            .iter()
            .filter(|case| case.tags.iter().any(|value| value == tag))
            .count();
        assert!(
            count >= 1,
            "expected at least one retrieval eval case tagged {tag}, got {count}"
        );
    }
}
```

Then add these three cases to `tests/fixtures/evals/retrieval_cases.json`:

```json
{
  "id": "timeline-auto-atlas-history",
  "description": "Auto timeline routing should return Atlas history in chronological order.",
  "query": "timeline of atlas changes in q1 2026",
  "scope": "org",
  "tags": ["timeline_auto"],
  "budget": 5,
  "facts": [
    {"content": "Atlas planning started", "t_valid": "2026-01-05T09:00:00Z"},
    {"content": "Atlas budget increased", "t_valid": "2026-02-10T09:00:00Z"},
    {"content": "Atlas launch confirmed", "t_valid": "2026-03-20T09:00:00Z"}
  ],
  "expected": {
    "tier": "temporal",
    "must_contain": ["Atlas planning started", "Atlas launch confirmed"],
    "min_recall_at_k": 1.0
  }
},
{
  "id": "graph-anchor-alice-neighbor",
  "description": "Named entity anchors should expand to connected facts even without lexical overlap.",
  "query": "Alice Stone",
  "scope": "org",
  "tags": ["graph_anchor"],
  "budget": 5,
  "entities": [
    {"entity_id": "entity:alice", "entity_type": "person", "canonical_name": "Alice Stone", "aliases": []},
    {"entity_id": "entity:bob", "entity_type": "person", "canonical_name": "Bob Chen", "aliases": []}
  ],
  "communities": [],
  "facts": [
    {
      "content": "Bob Chen owns the Atlas launch checklist.",
      "t_valid": "2026-04-30T12:00:00Z",
      "entity_links": ["entity:bob"]
    }
  ],
  "expected": {
    "tier": "graph",
    "must_contain": ["Atlas launch checklist"],
    "min_recall_at_k": 1.0
  }
},
{
  "id": "first-person-rescue-profile",
  "description": "First-person rescue should retain user-profile facts when they add unique grounding.",
  "query": "I'm planning a weekend getaway and want something creatively fulfilling",
  "scope": "personal",
  "tags": ["first_person_rescue"],
  "budget": 2,
  "facts": [
    {"content": "User: I am committing more time to original music so my creative work feels fulfilling.", "t_valid": "2026-04-13T09:00:00Z"},
    {"content": "Current user persona: spends weekends experimenting with music software and digital instruments.", "t_valid": "2026-04-12T09:00:00Z"}
  ],
  "expected": {
    "tier": "reasoning",
    "must_contain": ["Current user persona"],
    "min_recall_at_k": 1.0
  }
}
```

- [ ] **Step 3: Run the focused tests and confirm that query-log diagnostics and tag-aware eval coverage are still missing**

Run:

```bash
cargo test test_service_assemble_context_records_query_log_with_resolved_view_mode_and_flags --test service_integration -- --nocapture
cargo test retrieval_fixture_provides_graph_and_timeline_tag_coverage --test eval_retrieval -- --nocapture
```

Expected:
- the integration test fails because `query_log` rows do not yet store `resolved_view_mode`, `query_flags`, or `retrieval_tiers`;
- the eval test fails until the fixture model and JSON file both accept the new `tags` field.

- [ ] **Step 4: Add the migration, runtime query-log diagnostics, and tag-sliced eval summaries**

Create `migrations/020_query_log_retrieval_diagnostics.surql`:

```sql
-- Add richer retrieval diagnostics to assemble_context query analytics.

DEFINE FIELD OVERWRITE resolved_view_mode ON query_log TYPE option<string>;
DEFINE FIELD OVERWRITE query_flags ON query_log TYPE array<string>;
DEFINE FIELD OVERWRITE retrieval_tiers ON query_log TYPE option<object>;

DEFINE INDEX OVERWRITE query_log_scope_resolved_view_logged_at
    ON TABLE query_log COLUMNS scope, resolved_view_mode, logged_at;
```

Register it in `src/storage/migrations.rs`:

```rust
MigrationScript {
    file_name: "020_query_log_retrieval_diagnostics.surql",
    sql: include_str!("../../migrations/020_query_log_retrieval_diagnostics.surql"),
},
```

Add a `with_query_log_retention_days` builder-style setter to `MemoryService` (in `src/service/core/builder.rs`, on the `MemoryService` impl block, next to the existing `with_query_logging_enabled`):

```rust
pub fn with_query_log_retention_days(mut self, days: u32) -> Self {
    self.query_log_retention_days = days;
    self
}
```

Update `src/service/context/logging.rs` with an explicit diagnostics envelope. The updated functions are:

```rust
pub(crate) struct QueryLogDiagnostics<'a> {
    pub(crate) resolved_view_mode: Option<&'a str>,
    pub(crate) query_flags: &'a [String],
}

pub(crate) async fn record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    diagnostics: &QueryLogDiagnostics<'_>,
) -> Result<(), MemoryError> {
    let namespace = service.namespace_for_scope(&request.scope);
    let logged_at = crate::service::query::now();
    let project = request
        .project
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let retrieval_tier = results
        .iter()
        .filter_map(|item| item.retrieval_tier.as_deref())
        .map(str::trim)
        .find(|value| !value.is_empty());
    let retrieval_tiers = summarize_retrieval_tiers(results);
    let record_id = format!(
        "query_log:{}",
        crate::service::hash_prefix(&format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}",
            crate::service::normalize_text(&request.scope),
            crate::service::normalize_text(&request.query),
            crate::service::normalize_text(project.unwrap_or_default()),
            crate::service::normalize_text(diagnostics.resolved_view_mode.unwrap_or_default()),
            crate::service::normalize_text(&diagnostics.query_flags.join(",")),
            crate::service::normalize_text(retrieval_tier.unwrap_or_default()),
            results.len(),
            if cache_hit { "1" } else { "0" },
            crate::service::normalize_dt(logged_at),
        ))
    );

    let mut payload = serde_json::Map::from_iter([
        ("query_log_id".to_string(), json!(record_id.clone())),
        ("logged_at".to_string(), json!(crate::service::normalize_dt(logged_at))),
        ("scope".to_string(), json!(request.scope.clone())),
        ("query".to_string(), json!(request.query.clone())),
        ("result_count".to_string(), json!(results.len() as i64)),
        ("latency_ms".to_string(), json!(latency_ms)),
        ("cache_hit".to_string(), json!(cache_hit)),
    ]);

    if let Some(project) = project {
        payload.insert("project".to_string(), json!(project));
    }
    if let Some(resolved_view_mode) = diagnostics.resolved_view_mode {
        payload.insert("resolved_view_mode".to_string(), json!(resolved_view_mode));
    }
    if !diagnostics.query_flags.is_empty() {
        payload.insert("query_flags".to_string(), json!(diagnostics.query_flags));
    }
    if let Some(retrieval_tier) = retrieval_tier {
        payload.insert("retrieval_tier".to_string(), json!(retrieval_tier));
    }
    if retrieval_tiers.as_object().is_some_and(|value| !value.is_empty()) {
        payload.insert("retrieval_tiers".to_string(), retrieval_tiers);
    }

    service
        .db_client
        .create(&record_id, Value::Object(payload), &namespace)
        .await?;
    Ok(())
}
```

Also update the `maybe_record_query_log` signature to accept and forward diagnostics:

```rust
pub(crate) async fn maybe_record_query_log(
    service: &crate::service::MemoryService,
    request: &AssembleContextRequest,
    results: &[AssembledContextItem],
    cache_hit: bool,
    latency_ms: f64,
    access: &crate::models::AccessContext,
    diagnostics: &QueryLogDiagnostics<'_>,
) {
    if !service.is_query_logging_enabled() {
        // ... existing skip-logic unchanged ...
    }

    match record_query_log(service, request, results, cache_hit, latency_ms, diagnostics).await {
        // ... existing success/error handling unchanged ...
    }
}
```

Thread the diagnostics from `src/service/context.rs` into both cache-hit and cache-miss logging calls. Replace both existing `maybe_record_query_log(...)` calls with the new 7-argument form:

```rust
let query_flag_labels = query_flags.as_labels();
let query_log_diagnostics = logging::QueryLogDiagnostics {
    resolved_view_mode: resolved_view_mode.as_option_str(),
    query_flags: &query_flag_labels,
};

// cache-hit path
logging::maybe_record_query_log(
    service,
    &request,
    &cached,
    true,
    latency_ms,
    &access,
    &query_log_diagnostics,
)
.await;

// cache-miss path (at end of assemble_context)
logging::maybe_record_query_log(
    service,
    &request,
    &results,
    false,
    latency_ms,
    &access,
    &query_log_diagnostics,
)
.await;
```

Then extend eval summaries in `tests/eval_support/metrics.rs`:

```rust
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub expected_hits: usize,
    pub matched_hits: usize,
    pub reciprocal_rank_sum: f64,
    pub top_1_hits: usize,
    pub diversity_expected_cases: usize,
    pub diversity_passed_cases: usize,
    pub unique_source_episode_ratio_sum: f64,
    pub max_source_episode_share_sum: f64,
    pub expected_tier_totals: BTreeMap<String, usize>,
    pub expected_tier_passed_cases: BTreeMap<String, usize>,
    pub actual_tier_totals: BTreeMap<String, usize>,
    pub expected_tag_totals: BTreeMap<String, usize>,
    pub expected_tag_passed_cases: BTreeMap<String, usize>,
}

impl RetrievalSuiteSummary {
    pub fn expected_tag_pass_rate(&self, tag: &str) -> Option<f64> {
        let total = self.expected_tag_totals.get(tag).copied()?;
        if total == 0 {
            return Some(1.0);
        }
        let passed = self
            .expected_tag_passed_cases
            .get(tag)
            .copied()
            .unwrap_or(0);
        Some(passed as f64 / total as f64)
    }
}

pub(crate) fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    expected_tier: &str,
    expected_tags: &[String],
    matched_hits: usize,
    expected_hits: usize,
    min_recall_at_k: f64,
    diagnostics: RetrievalCaseDiagnostics<'_>,
) -> bool {
    summary.total_cases += 1;
    summary.expected_hits += expected_hits;
    summary.matched_hits += matched_hits;
    *summary
        .expected_tier_totals
        .entry(expected_tier.to_string())
        .or_insert(0) += 1;
    for tag in expected_tags {
        *summary.expected_tag_totals.entry(tag.clone()).or_insert(0) += 1;
    }

    for tier in diagnostics.actual_tiers {
        *summary
            .actual_tier_totals
            .entry((*tier).to_string())
            .or_insert(0) += 1;
    }

    if let Some(rank) = diagnostics.first_relevant_rank {
        summary.reciprocal_rank_sum += 1.0 / rank as f64;
        if rank == 1 {
            summary.top_1_hits += 1;
        }
    }

    let diversity_expected = diagnostics.min_unique_source_episodes.is_some()
        || diagnostics.max_source_episode_share.is_some();
    let diversity_passed = if diversity_expected {
        summary.diversity_expected_cases += 1;
        let diversity = source_episode_diversity(diagnostics.source_episodes).unwrap_or(
            SourceEpisodeDiversity {
                unique_source_episodes: 0,
                unique_source_episode_ratio: 0.0,
                max_source_episode_share: 1.0,
            },
        );
        summary.unique_source_episode_ratio_sum += diversity.unique_source_episode_ratio;
        summary.max_source_episode_share_sum += diversity.max_source_episode_share;

        let passed = diagnostics
            .min_unique_source_episodes
            .is_none_or(|minimum| diversity.unique_source_episodes >= minimum)
            && diagnostics
                .max_source_episode_share
                .is_none_or(|maximum| diversity.max_source_episode_share <= maximum);
        if passed {
            summary.diversity_passed_cases += 1;
        }
        passed
    } else {
        true
    };

    let recall = if expected_hits == 0 {
        1.0
    } else {
        matched_hits as f64 / expected_hits as f64
    };
    let passed = recall >= min_recall_at_k && diversity_passed;
    if passed {
        summary.passed_cases += 1;
        *summary
            .expected_tier_passed_cases
            .entry(expected_tier.to_string())
            .or_insert(0) += 1;
        for tag in expected_tags {
            *summary
                .expected_tag_passed_cases
                .entry(tag.clone())
                .or_insert(0) += 1;
        }
    }

    passed
}
```

Update `tests/eval_support/report.rs` so tag slices print in the summary:

```rust
for (tag, total) in &summary.expected_tag_totals {
    let passed = summary
        .expected_tag_passed_cases
        .get(tag)
        .copied()
        .unwrap_or(0);
    let pass_rate = summary.expected_tag_pass_rate(tag).unwrap_or(1.0);
    lines.push(format!(
        "expected_tag={} total={} passed={} pass_rate={:.2}",
        tag, total, passed, pass_rate
    ));
}
```

And update both call sites. In `tests/eval_retrieval.rs`:

```rust
let passed = record_retrieval_case(
    &mut summary,
    &case.expected.tier,
    &case.tags,
    matched_hits,
    expected_hits,
    case.expected.min_recall_at_k,
    RetrievalCaseDiagnostics {
        actual_tiers: &actual_tiers,
        first_relevant_rank,
        source_episodes: &source_episodes,
        min_unique_source_episodes,
        max_source_episode_share,
    },
);
```

In `tests/eval_external_retrieval.rs` (~line 715), external eval cases do not carry tags yet, so pass an empty slice:

```rust
let passed = record_retrieval_case(
    summary,
    &outcome.case.expected.tier,
    &[],  // external datasets don't use tags
    outcome.matched_hits,
    outcome.expected_hits,
    outcome.case.expected.min_recall_at_k,
    RetrievalCaseDiagnostics {
        actual_tiers: &actual_tier_refs,
        first_relevant_rank: first_relevant_rank(
            &retrieved_content_refs,
            &outcome.case.expected.must_contain,
        ),
        source_episodes: &source_episode_refs,
        min_unique_source_episodes: None,
        max_source_episode_share: None,
    },
);
```

- [ ] **Step 5: Run the focused tests again and verify both runtime diagnostics and eval slices now pass**

Run:

```bash
cargo test test_service_assemble_context_records_query_log_with_resolved_view_mode_and_flags --test service_integration -- --nocapture
cargo test retrieval_fixture_provides_graph_and_timeline_tag_coverage --test eval_retrieval -- --nocapture
cargo test render_retrieval_summary_includes_expected_and_actual_tiers --test eval_retrieval -- --nocapture
```

Expected:
- the service integration test PASSes and the `query_log` row stores `resolved_view_mode`, `query_flags`, and `retrieval_tiers`;
- the eval test PASSes with the tagged fixture cases;
- the retrieval-summary rendering still PASSes after the additional tag lines.

- [ ] **Step 6: Commit the retrieval diagnostics and eval slice work**

Run:

```bash
git add migrations/020_query_log_retrieval_diagnostics.surql src/storage/migrations.rs src/service/context/logging.rs src/service/context.rs tests/service_integration.rs tests/eval_support/metrics.rs tests/eval_support/report.rs tests/eval_retrieval.rs tests/fixtures/evals/retrieval_cases.json
git commit -m "feat: persist retrieval diagnostics in query_log"
```

- [ ] **Step 7: Run the full internal retrieval eval — all tagged fixture cases must now pass**

Now that the `timeline_auto`, `graph_anchor`, and `first_person_rescue` fixture cases exist, they must all pass:

Run:

```bash
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-post-task4-retrieval.txt
```

Verify in the output:

- `expected_tag=timeline_auto` reports `passed≥1` with `pass_rate=1.00`
- `expected_tag=graph_anchor` reports `passed≥1` with `pass_rate=1.00`
- `expected_tag=first_person_rescue` reports `passed≥1` with `pass_rate=1.00`
- All five existing tier pass rates still meet their targets.

If any tagged case fails, debug the specific case before proceeding to Task 5. Expected: all eval cases PASS including the three new tagged slices.

---

### Task 5: Final verification — full eval sweep and baseline comparison

**Files:**
- Modify: `README.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`
- Create: `docs/superpowers/plans/baselines/2026-04-30-retrieval-final-comparison.md` — before/after delta report
- Verify only: repository root

- [ ] **Step 1: Run the FULL eval sweep (no sampling) and capture the final numbers**

Run every eval suite at full size:

```bash
cargo test --test eval_retrieval -- --nocapture --test-threads=1 2>&1 | tee docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
cargo test --test eval_external_retrieval -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
cargo test --test eval_external_provenance -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
cargo test --test eval_external_full_datasets -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
cargo test --test eval_extraction -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
cargo test --test eval_latency -- --nocapture --test-threads=1 2>&1 | tee -a docs/superpowers/plans/baselines/2026-04-30-retrieval-final.txt
```

- [ ] **Step 2: Build the before/after delta report**

Create `docs/superpowers/plans/baselines/2026-04-30-retrieval-final-comparison.md` with this structure:

```md
# Retrieval Refinement — Final Delta Report (2026-04-30)

## Internal retrieval eval (eval_retrieval)

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | ____ | ____ | ____ | ✅/❌ |
| mrr | ____ | ____ | ____ | ✅/❌ |
| top1_hit_rate | ____ | ____ | ____ | ✅/❌ |
| direct pass_rate | ____ | ____ | ____ | ✅/❌ |
| alias pass_rate | ____ | ____ | ____ | ✅/❌ |
| temporal pass_rate | ____ | ____ | ____ | ✅/❌ |
| graph pass_rate | ____ | ____ | ____ | ✅/❌ |
| reasoning pass_rate | ____ | ____ | ____ | ✅/❌ |

### New tagged slices (post-refinement only)

| Tag | Total | Passed | Pass Rate |
|-----|-------|--------|-----------|
| timeline_auto | ____ | ____ | ____ |
| graph_anchor | ____ | ____ | ____ |
| first_person_rescue | ____ | ____ | ____ |

## External — LongMemEval (full)

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | ____ | ____ | ____ | ✅/❌ |
| mrr | ____ | ____ | ____ | ✅/❌ |
| top1_hit_rate | ____ | ____ | ____ | ✅/❌ |

## External — LoCoMo (full)

| Metric | Baseline | Final | Delta | Pass? |
|--------|----------|-------|-------|-------|
| recall_at_5 | ____ | ____ | ____ | ✅/❌ |
| mrr | ____ | ____ | ____ | ✅/❌ |
| top1_hit_rate | ____ | ____ | ____ | ✅/❌ |

## Extraction eval

| Metric | Baseline | Final | Pass? |
|--------|----------|-------|-------|
| (key metric) | ____ | ____ | ✅/❌ |

## Latency eval

| Metric | Baseline | Final | Pass? |
|--------|----------|-------|-------|
| (key metric) | ____ | ____ | ✅/❌ |

## Summary

- Regressions: (list or "none")
- Improvements: (list the metrics that improved)
- Verdict: ✅ SHIP / ⚠️ NEEDS WORK / ❌ ROLL BACK
```

Fill in all blanks from the baseline and final output files.

- [ ] **Step 3: Assert no regressions in the delta report**

Every metric in the delta report MUST satisfy:

- `recall_at_5` final ≥ baseline
- `mrr` final ≥ baseline
- `top1_hit_rate` final ≥ baseline
- All tier pass rates final ≥ their targets (direct ≥ 0.95, alias ≥ 0.85, temporal ≥ 0.80, graph ≥ 0.70, reasoning ≥ 0.60)
- Extraction and latency evals must still PASS

If any metric regressed below its target, investigate and fix before proceeding to docs/commit. If a metric improved, note it in the report.

- [ ] **Step 4: Update the user-facing docs to describe automatic timeline routing and query-log diagnostics**

Add a concise retrieval note to `README.md`:

```md
### Retrieval behavior

`assemble_context` remains lexical/BM25-first, but now applies deterministic query-mode routing before ranking results:

- explicit `view_mode` still wins;
- temporal-history queries such as "timeline of Atlas changes in Q1 2026" automatically resolve to timeline ordering when `view_mode` is omitted;
- named entity anchors can expand into 1-hop graph context (2 hops for explicit connection/path questions) without requiring semantic retrieval.

When `QUERY_LOGGING_ENABLED=true`, query analytics now store:

- `resolved_view_mode`
- `query_flags`
- `retrieval_tiers`
- `retrieval_tier`
- `latency_ms`, `result_count`, and `cache_hit`
```

Update `docs/MEMORY_SYSTEM_SPEC.md` by extending the context-assembly requirements section:

```md
**FR-CA-11**: `assemble_context` SHOULD auto-resolve timeline ordering for explicit temporal-history queries when callers leave `view_mode` empty. Explicit `view_mode` remains authoritative. Named entity anchors SHOULD expand into bounded graph context (1 hop for entity-centric queries, 2 hops for path/introduction queries) without making semantic retrieval mandatory.
**Status**: ✅ Done — implemented via deterministic query flags and bounded entity-anchor expansion in `src/service/context/query_mode.rs` and `src/service/context/graph.rs`.

**FR-CA-12**: When query logging is enabled, the system MUST persist `resolved_view_mode`, `query_flags`, and retrieval-tier distribution alongside existing latency/result-count analytics.
**Status**: ✅ Done — stored in `query_log` via migration `020_query_log_retrieval_diagnostics.surql`.
```

Update the Wave 4 section in `docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`:

```md
Wave 4 is complete when the default retrieval path uses deterministic query-mode flags, bounded entity-anchor graph expansion, and explainable graph traces in the assembled result provenance. Semantic retrieval may still exist as an optional later-stage source, but lexical/BM25 + graph must remain sufficient on their own.
```

- [ ] **Step 5: Run formatting and targeted verification before the broad suite**

Run:

```bash
cargo fmt --all --check
cargo test assemble_context_auto_timeline_orders_results_without_explicit_view_mode --test service_integration -- --nocapture
cargo test assemble_context_graph_expansion_returns_anchor_neighbor_fact --test service_integration -- --nocapture
cargo test test_assemble_context_graph_results_include_anchor_and_hop_trace --test service_acceptance -- --nocapture
cargo test test_service_assemble_context_records_query_log_with_resolved_view_mode_and_flags --test service_integration -- --nocapture
cargo test retrieval_fixture_provides_graph_and_timeline_tag_coverage --test eval_retrieval -- --nocapture
```

Expected:
- formatting check PASSes;
- the service/acceptance tests PASS with the new retrieval behavior;
- the eval coverage test PASSes with the tagged fixture cases.

- [ ] **Step 6: Run the repository-level verification command and then inspect the final diff**

Run:

```bash
cargo fmt --all --check && cargo test -q

git diff -- README.md docs/MEMORY_SYSTEM_SPEC.md docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md migrations/020_query_log_retrieval_diagnostics.surql src/storage/migrations.rs src/service/context.rs src/service/context/alias_expansion.rs src/service/context/graph.rs src/service/context/logging.rs src/service/context/params.rs src/service/context/pipeline.rs src/service/context/query_mode.rs src/service/context/ranking.rs src/service/context/scoring.rs tests/service_acceptance.rs tests/service_integration.rs tests/eval_retrieval.rs tests/eval_support/metrics.rs tests/eval_support/report.rs tests/fixtures/evals/retrieval_cases.json
```

Expected:
- the repository-level verification command PASSes;
- only the planned retrieval, logging, eval, and docs files are changed;
- no MCP tool surface or unrelated lifecycle files are modified.

- [ ] **Step 7: Commit the docs, delta report, and verified implementation**

Run:

```bash
git add README.md docs/MEMORY_SYSTEM_SPEC.md docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md docs/superpowers/plans/baselines/2026-04-30-retrieval-final-comparison.md
git commit -m "docs: describe local-first retrieval refinement with before/after eval delta"
```

---

## Self-review checklist

- [ ] The plan does **not** re-implement `index_keys`, access heat, timeline `view_mode`, or the LongMem acceptance harness.
- [ ] The plan keeps the public MCP surface unchanged.
- [ ] The plan works when semantic retrieval is disabled.
- [ ] Automatic timeline routing is deterministic and explicit `view_mode` still wins.
- [ ] Graph expansion is bounded to 1 hop by default and 2 hops for explicit path/introduction queries.
- [ ] Graph-expanded results carry deterministic anchor/hop trace metadata in both `rationale` and `provenance`.
- [ ] Query-log diagnostics remain optional behind `QUERY_LOGGING_ENABLED`.
- [ ] Eval additions cover `timeline_auto`, `graph_anchor`, and `first_person_rescue` slices.
- [ ] Baseline eval metrics are captured BEFORE any code changes (Task 0).
- [ ] Eval suites are re-run after every retrieval-changing task (Tasks 1, 2, 3, 4) and compared against baseline.
- [ ] Final delta report (`retrieval-final-comparison.md`) proves no regressions — all metrics at or above baseline.
- [ ] If any eval metric regressed, the plan requires stopping and diagnosing before continuing.
- [ ] Selective decay/persona-retention work remains out of scope for this plan.
- [ ] Embedding rebuild maintenance remains out of scope for this plan.
