# MCP Evals System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a repository-local eval system for `memory_mcp` that measures retrieval quality, extraction quality, and latency using real service code on fresh in-memory SurrealDB instances only.

**Architecture:** Implement the eval system as three ignored integration-test runners (`eval_extraction`, `eval_retrieval`, `eval_latency`) backed by a shared support layer for fixture loading, metric computation, and reporting. Reuse the existing in-memory test harness from `tests/common`, keep all DB state ephemeral, and print results to stdout by default so no run stores cross-session benchmark history.

**Tech Stack:** Rust 2024, Tokio, serde/serde_json, existing `MemoryService`, embedded in-memory SurrealDB via `SurrealDbClient::connect_in_memory_with_namespaces`, existing integration-test harness, markdown docs

---

## Scope check

This is one subsystem, not multiple independent projects. Dataset contracts, harness helpers, metric computation, latency measurement, and operator docs all serve the same eval system and should land in one plan.

## File structure

### New files

- `tests/eval_support/mod.rs` — shared async helpers for fixture loading, service setup, seeding, normalization, and result printing
- `tests/eval_support/dataset.rs` — typed JSON fixture structs and validation logic
- `tests/eval_support/metrics.rs` — retrieval, extraction, and latency metric calculators
- `tests/eval_support/report.rs` — stdout summary formatting and optional temp-file JSON serialization
- `tests/eval_contracts.rs` — fixture validation coverage and contradiction checks
- `tests/eval_extraction.rs` — ignored extraction-quality runner
- `tests/eval_retrieval.rs` — ignored retrieval-quality runner
- `tests/eval_latency.rs` — ignored latency runner using `std::time::Instant`
- `tests/fixtures/evals/extraction_cases.json` — extraction eval fixtures
- `tests/fixtures/evals/retrieval_cases.json` — retrieval eval fixtures
- `tests/fixtures/evals/latency_cases.json` — latency eval fixtures

### Modified files

- `README.md` — document how to run eval suites manually and explain the in-memory-only rule
- `docs/MEMORY_SYSTEM_SPEC.md` — add a short section describing the new metric eval harness alongside correctness tests

### Existing files to reuse, not duplicate

- `tests/common/mod.rs` — current in-memory service and seeding helpers
- `tests/embedded_support.rs` — embedded/in-memory service patterns
- `tests/longmem_acceptance.rs` — behavioral coverage to complement, not replace, metric evals

---

### Task 1: Add fixture contracts and shared eval support

**Status:** ✅ **Done**

**Files:**
- Create: `tests/eval_support/mod.rs`
- Create: `tests/eval_support/dataset.rs`
- Create: `tests/eval_support/metrics.rs`
- Create: `tests/eval_support/report.rs`
- Create: `tests/eval_contracts.rs`
- Create: `tests/fixtures/evals/extraction_cases.json`
- Create: `tests/fixtures/evals/retrieval_cases.json`
- Create: `tests/fixtures/evals/latency_cases.json`

- [x] **Step 1: Write the failing contract test**
- [x] **Step 2: Run the contract test to verify it fails**
- [x] **Step 3: Add typed fixture loaders, validators, and starter fixtures**
- [x] **Step 4: Run the contract test to verify it passes**
- [x] **Step 5: Commit**

Create `tests/eval_contracts.rs`:

```rust
mod eval_support;

#[test]
fn retrieval_dataset_rejects_contradictory_expectations() {
    let json = r#"
    [
      {
        "id": "ret-bad-001",
        "description": "contradictory case",
        "episodes": [],
        "query": { "query": "atlas", "scope": "personal", "budget": 5, "as_of": null },
        "expected": {
          "must_contain": ["Atlas"],
          "must_not_contain": [],
          "expect_empty": true,
          "min_recall_at_k": 1.0
        }
      }
    ]
    "#;

    let err = eval_support::dataset::parse_retrieval_cases(json).unwrap_err();
    assert!(err.to_string().contains("expect_empty"));
}
```

- [ ] **Step 2: Run the contract test to verify it fails**

Run: `cargo test --test eval_contracts retrieval_dataset_rejects_contradictory_expectations -- --nocapture`

Expected: FAIL with a compile error such as `file not found for module 'eval_support'` or `cannot find function 'parse_retrieval_cases'`.

- [ ] **Step 3: Add typed fixture loaders, validators, and starter fixtures**

Create `tests/eval_support/mod.rs`:

```rust
pub mod dataset;
pub mod metrics;
pub mod report;

#[path = "../common/mod.rs"]
pub mod common;
```

Create `tests/eval_support/dataset.rs`:

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalEvalCase {
    pub id: String,
    pub description: String,
    pub episodes: Vec<EvalEpisode>,
    pub query: RetrievalQuery,
    pub expected: RetrievalExpectation,
}

#[derive(Debug, Deserialize, Clone)]
pub struct EvalEpisode {
    pub source_type: String,
    pub source_id: String,
    pub content: String,
    pub t_ref: String,
    pub scope: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalQuery {
    pub query: String,
    pub scope: String,
    pub budget: i32,
    pub as_of: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalExpectation {
    pub must_contain: Vec<String>,
    pub must_not_contain: Vec<String>,
    pub expect_empty: bool,
    pub min_recall_at_k: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExtractionEvalCase {
    pub id: String,
    pub description: String,
    pub content: String,
    pub scope: String,
    pub t_ref: String,
    pub expected: ExtractionExpectation,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExtractionExpectation {
    pub entities: Vec<ExpectedEntity>,
    pub fact_types: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ExpectedEntity {
    pub entity_type: String,
    pub canonical_name: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct LatencyEvalCase {
    pub id: String,
    pub description: String,
    pub query: String,
    pub scope: String,
    pub episode_count: usize,
    pub warmup_iterations: usize,
    pub measured_iterations: usize,
}

pub fn parse_retrieval_cases(input: &str) -> Result<Vec<RetrievalEvalCase>, String> {
    let cases: Vec<RetrievalEvalCase> = serde_json::from_str(input).map_err(|err| err.to_string())?;
    validate_retrieval_cases(&cases)?;
    Ok(cases)
}

pub fn parse_extraction_cases(input: &str) -> Result<Vec<ExtractionEvalCase>, String> {
    let cases: Vec<ExtractionEvalCase> = serde_json::from_str(input).map_err(|err| err.to_string())?;
    if cases.iter().any(|case| case.expected.fact_types.is_empty()) {
        return Err("extraction case must declare expected.fact_types".to_string());
    }
    Ok(cases)
}

pub fn parse_latency_cases(input: &str) -> Result<Vec<LatencyEvalCase>, String> {
    let cases: Vec<LatencyEvalCase> = serde_json::from_str(input).map_err(|err| err.to_string())?;
    if cases.iter().any(|case| case.measured_iterations == 0) {
        return Err("latency case must declare measured_iterations > 0".to_string());
    }
    Ok(cases)
}

fn validate_retrieval_cases(cases: &[RetrievalEvalCase]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    for case in cases {
        if !ids.insert(case.id.clone()) {
            return Err(format!("duplicate retrieval case id: {}", case.id));
        }
        if case.expected.expect_empty && !case.expected.must_contain.is_empty() {
            return Err(format!(
                "retrieval case {} cannot set expect_empty=true and must_contain simultaneously",
                case.id
            ));
        }
        if case.query.budget <= 0 {
            return Err(format!("retrieval case {} must use budget > 0", case.id));
        }
    }
    Ok(())
}
```

Create `tests/eval_support/metrics.rs`:

```rust
#[derive(Debug, Default, Clone)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub recall_at_k_sum: f64,
    pub precision_at_k_sum: f64,
    pub reciprocal_rank_sum: f64,
    pub empty_when_irrelevant_hits: usize,
}

#[derive(Debug, Default, Clone)]
pub struct ExtractionSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub entity_true_positive: usize,
    pub entity_false_positive: usize,
    pub entity_false_negative: usize,
    pub fact_type_hits: usize,
    pub fact_type_total: usize,
}

#[derive(Debug, Default, Clone)]
pub struct LatencySuiteSummary {
    pub ingest_ms: Vec<f64>,
    pub extract_ms: Vec<f64>,
    pub assemble_ms: Vec<f64>,
}

pub fn percentile_ms(values: &[f64], percentile: f64) -> f64 {
    assert!(!values.is_empty());
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((sorted.len() - 1) as f64 * percentile).round() as usize;
    sorted[rank]
}
```

Create `tests/eval_support/report.rs`:

```rust
use super::metrics::{percentile_ms, ExtractionSuiteSummary, LatencySuiteSummary, RetrievalSuiteSummary};

pub fn print_retrieval_summary(summary: &RetrievalSuiteSummary) {
    let total = summary.total_cases.max(1) as f64;
    println!(
        "suite=eval_retrieval total={} passed={} recall_at_5={:.3} precision_at_5={:.3} mrr={:.3} empty_when_irrelevant={:.3}",
        summary.total_cases,
        summary.passed_cases,
        summary.recall_at_k_sum / total,
        summary.precision_at_k_sum / total,
        summary.reciprocal_rank_sum / total,
        summary.empty_when_irrelevant_hits as f64 / total,
    );
}

pub fn print_extraction_summary(summary: &ExtractionSuiteSummary) {
    let tp = summary.entity_true_positive as f64;
    let fp = summary.entity_false_positive as f64;
    let fn_ = summary.entity_false_negative as f64;
    let precision = if (tp + fp) == 0.0 { 0.0 } else { tp / (tp + fp) };
    let recall = if (tp + fn_) == 0.0 { 0.0 } else { tp / (tp + fn_) };
    let f1 = if (precision + recall) == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) };
    let fact_type_accuracy = if summary.fact_type_total == 0 {
        0.0
    } else {
        summary.fact_type_hits as f64 / summary.fact_type_total as f64
    };

    println!(
        "suite=eval_extraction total={} passed={} entity_precision={:.3} entity_recall={:.3} entity_f1={:.3} fact_type_accuracy={:.3}",
        summary.total_cases,
        summary.passed_cases,
        precision,
        recall,
        f1,
        fact_type_accuracy,
    );
}

pub fn print_latency_summary(summary: &LatencySuiteSummary) {
    println!(
        "suite=eval_latency ingest_p50_ms={:.2} ingest_p95_ms={:.2} extract_p50_ms={:.2} extract_p95_ms={:.2} assemble_p50_ms={:.2} assemble_p95_ms={:.2}",
        percentile_ms(&summary.ingest_ms, 0.50),
        percentile_ms(&summary.ingest_ms, 0.95),
        percentile_ms(&summary.extract_ms, 0.50),
        percentile_ms(&summary.extract_ms, 0.95),
        percentile_ms(&summary.assemble_ms, 0.50),
        percentile_ms(&summary.assemble_ms, 0.95),
    );
}
```

Create `tests/fixtures/evals/retrieval_cases.json`:

```json
[
  {
    "id": "ret-001",
    "description": "Promise across sessions found without travel noise",
    "episodes": [
      {
        "source_type": "chat",
        "source_id": "sess-1",
        "content": "Alice will send the Atlas deck by Friday.",
        "t_ref": "2026-03-01T09:00:00Z",
        "scope": "personal"
      },
      {
        "source_type": "chat",
        "source_id": "sess-2",
        "content": "We discussed unrelated travel plans.",
        "t_ref": "2026-03-01T10:00:00Z",
        "scope": "personal"
      }
    ],
    "query": { "query": "alice atlas deck", "scope": "personal", "budget": 5, "as_of": null },
    "expected": {
      "must_contain": ["Atlas deck"],
      "must_not_contain": ["travel plans"],
      "expect_empty": false,
      "min_recall_at_k": 1.0
    }
  }
]
```

Create `tests/fixtures/evals/extraction_cases.json`:

```json
[
  {
    "id": "ext-001",
    "description": "Metric and promise in one episode",
    "content": "ARR grew to $3M. I will send the update by Friday.",
    "scope": "org",
    "t_ref": "2026-03-01T09:00:00Z",
    "expected": {
      "entities": [
        { "entity_type": "metric", "canonical_name": "ARR" }
      ],
      "fact_types": ["metric", "promise"]
    }
  }
]
```

Create `tests/fixtures/evals/latency_cases.json`:

```json
[
  {
    "id": "lat-001",
    "description": "Small personal-memory retrieval workload",
    "query": "project status",
    "scope": "personal",
    "episode_count": 25,
    "warmup_iterations": 3,
    "measured_iterations": 10
  }
]
```

- [ ] **Step 4: Run the contract test to verify it passes**

Run: `cargo test --test eval_contracts retrieval_dataset_rejects_contradictory_expectations -- --nocapture`

Expected: PASS with `test retrieval_dataset_rejects_contradictory_expectations ... ok`.

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support tests/eval_contracts.rs tests/fixtures/evals
git commit -m "test: add eval fixture contracts and shared support"
```

### Task 2: Add the extraction-quality eval runner

**Status:** ✅ **Done**

**Files:**
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Create: `tests/eval_extraction.rs`
- Modify: `tests/fixtures/evals/extraction_cases.json`

- [x] **Step 1: Write the failing extraction eval**
- [x] **Step 2: Run the extraction eval to verify it fails**
- [x] **Step 3: Implement entity matching, fact-type accuracy, and pass accounting**
- [x] **Step 4: Run the extraction eval to verify it passes**
- [x] **Step 5: Commit**

Create `tests/eval_extraction.rs`:

```rust
mod eval_support;

use chrono::{DateTime, Utc};
use eval_support::dataset::parse_extraction_cases;
use eval_support::metrics::ExtractionSuiteSummary;
use eval_support::report::print_extraction_summary;
use memory_mcp::models::IngestRequest;

#[tokio::test]
#[ignore = "eval: manual extraction quality run"]
async fn run_extraction_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/extraction_cases.json").unwrap();
    let cases = parse_extraction_cases(&raw).unwrap();
    let mut summary = ExtractionSuiteSummary::default();

    for case in cases {
        summary.total_cases += 1;
        let service = eval_support::common::make_service().await;
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "eval".to_string(),
                    source_id: case.id.clone(),
                    content: case.content.clone(),
                    t_ref: case.t_ref.parse::<DateTime<Utc>>().unwrap(),
                    scope: case.scope.clone(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap();

        let result = service.extract(&episode_id, None).await.unwrap();
        let actual_fact_types: std::collections::BTreeSet<_> =
            result.facts.iter().map(|fact| fact.fact_type.clone()).collect();

        for expected in &case.expected.fact_types {
            summary.fact_type_total += 1;
            if actual_fact_types.contains(expected) {
                summary.fact_type_hits += 1;
            }
        }
    }

    print_extraction_summary(&summary);
    assert!(summary.fact_type_total > 0, "extraction suite must evaluate fact types");
    assert!(summary.fact_type_hits == summary.fact_type_total, "fact type accuracy regressed");
}
```

- [ ] **Step 2: Run the extraction eval to verify it fails**

Run: `cargo test --test eval_extraction run_extraction_evals -- --ignored --nocapture --test-threads=1`

Expected: FAIL because entity matching and pass accounting are not implemented yet, or because the suite reports `fact_type_accuracy=0.000`.

- [ ] **Step 3: Implement entity matching, fact-type accuracy, and pass accounting**

Update `tests/eval_support/metrics.rs`:

```rust
pub fn normalize_label(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

pub fn record_extraction_case(
    summary: &mut ExtractionSuiteSummary,
    expected_entities: &[(String, String)],
    actual_entities: &[(String, String)],
    expected_fact_types: &[String],
    actual_fact_types: &std::collections::BTreeSet<String>,
) -> bool {
    let expected_entity_set: std::collections::BTreeSet<_> = expected_entities
        .iter()
        .map(|(entity_type, name)| (normalize_label(entity_type), normalize_label(name)))
        .collect();
    let actual_entity_set: std::collections::BTreeSet<_> = actual_entities
        .iter()
        .map(|(entity_type, name)| (normalize_label(entity_type), normalize_label(name)))
        .collect();

    let mut case_ok = true;

    for item in &expected_entity_set {
        if actual_entity_set.contains(item) {
            summary.entity_true_positive += 1;
        } else {
            summary.entity_false_negative += 1;
            case_ok = false;
        }
    }

    for item in &actual_entity_set {
        if !expected_entity_set.contains(item) {
            summary.entity_false_positive += 1;
            case_ok = false;
        }
    }

    for fact_type in expected_fact_types {
        summary.fact_type_total += 1;
        if actual_fact_types.contains(fact_type) {
            summary.fact_type_hits += 1;
        } else {
            case_ok = false;
        }
    }

    if case_ok {
        summary.passed_cases += 1;
    }

    case_ok
}
```

Update `tests/eval_extraction.rs` to use it:

```rust
let actual_entities: Vec<_> = result
    .entities
    .iter()
    .map(|entity| (entity.entity_type.clone(), entity.canonical_name.clone()))
    .collect();
let expected_entities: Vec<_> = case
    .expected
    .entities
    .iter()
    .map(|entity| (entity.entity_type.clone(), entity.canonical_name.clone()))
    .collect();

let case_ok = eval_support::metrics::record_extraction_case(
    &mut summary,
    &expected_entities,
    &actual_entities,
    &case.expected.fact_types,
    &actual_fact_types,
);
assert!(case_ok, "extraction eval case failed: {}", case.id);
```

Update the final assertions in `tests/eval_extraction.rs`:

```rust
print_extraction_summary(&summary);

let tp = summary.entity_true_positive as f64;
let fp = summary.entity_false_positive as f64;
let fn_ = summary.entity_false_negative as f64;
let precision = if (tp + fp) == 0.0 { 0.0 } else { tp / (tp + fp) };
let recall = if (tp + fn_) == 0.0 { 0.0 } else { tp / (tp + fn_) };
let f1 = if (precision + recall) == 0.0 { 0.0 } else { 2.0 * precision * recall / (precision + recall) };
let fact_type_accuracy = summary.fact_type_hits as f64 / summary.fact_type_total as f64;

assert!(fact_type_accuracy >= 0.85, "fact_type_accuracy dropped below 0.85");
assert!(f1 >= 0.75, "entity_f1 dropped below 0.75");
```

Add more extraction fixtures to `tests/fixtures/evals/extraction_cases.json` covering:

```json
{
  "id": "ext-002",
  "description": "Promise-only sentence",
  "content": "I will finish the integration by next Monday.",
  "scope": "org",
  "t_ref": "2026-03-02T09:00:00Z",
  "expected": {
    "entities": [],
    "fact_types": ["promise"]
  }
}
```

```json
{
  "id": "ext-003",
  "description": "Metric-only sentence",
  "content": "Revenue reached $5M in Q4.",
  "scope": "org",
  "t_ref": "2026-03-03T09:00:00Z",
  "expected": {
    "entities": [],
    "fact_types": ["metric"]
  }
}
```

- [ ] **Step 4: Run the extraction eval to verify it passes**

Run: `cargo test --test eval_extraction run_extraction_evals -- --ignored --nocapture --test-threads=1`

Expected: PASS with a summary line such as `suite=eval_extraction ... fact_type_accuracy=...` and `test run_extraction_evals ... ok`.

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support tests/eval_extraction.rs tests/fixtures/evals/extraction_cases.json
git commit -m "test: add extraction quality eval runner"
```

### Task 3: Add the retrieval-quality eval runner

**Status:** ✅ **Done**

**Files:**
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Create: `tests/eval_retrieval.rs`
- Modify: `tests/fixtures/evals/retrieval_cases.json`

- [x] **Step 1: Write the failing retrieval eval**
- [x] **Step 2: Run the retrieval eval to verify it fails**
- [x] **Step 3: Implement Recall@K, Precision@K, MRR, abstention, and case accounting**
- [x] **Step 4: Run the retrieval eval to verify it passes**
- [x] **Step 5: Commit**

Create `tests/eval_retrieval.rs`:

```rust
mod eval_support;

use chrono::{DateTime, Utc};
use eval_support::dataset::parse_retrieval_cases;
use eval_support::metrics::RetrievalSuiteSummary;
use eval_support::report::print_retrieval_summary;
use memory_mcp::models::{AssembleContextRequest, IngestRequest};

#[tokio::test]
#[ignore = "eval: manual retrieval quality run"]
async fn run_retrieval_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/retrieval_cases.json").unwrap();
    let cases = parse_retrieval_cases(&raw).unwrap();
    let mut summary = RetrievalSuiteSummary::default();

    for case in cases {
        summary.total_cases += 1;
        let service = eval_support::common::make_service().await;

        for episode in &case.episodes {
            let episode_id = service
                .ingest(
                    IngestRequest {
                        source_type: episode.source_type.clone(),
                        source_id: episode.source_id.clone(),
                        content: episode.content.clone(),
                        t_ref: episode.t_ref.parse::<DateTime<Utc>>().unwrap(),
                        scope: episode.scope.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
            service.extract(&episode_id, None).await.unwrap();
        }

        let items = service
            .assemble_context(AssembleContextRequest {
                query: case.query.query.clone(),
                scope: case.query.scope.clone(),
                as_of: case.query.as_of.as_ref().map(|ts| ts.parse::<DateTime<Utc>>().unwrap()),
                budget: case.query.budget,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap();

        assert!(!items.is_empty(), "retrieval case {} returned no items", case.id);
    }

    print_retrieval_summary(&summary);
    assert!(summary.passed_cases == summary.total_cases, "retrieval suite regressed");
}
```

- [ ] **Step 2: Run the retrieval eval to verify it fails**

Run: `cargo test --test eval_retrieval run_retrieval_evals -- --ignored --nocapture --test-threads=1`

Expected: FAIL because metrics and pass accounting are not implemented yet.

- [ ] **Step 3: Implement Recall@K, Precision@K, MRR, abstention, and case accounting**

Update `tests/eval_support/metrics.rs`:

```rust
pub fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    must_contain: &[String],
    must_not_contain: &[String],
    expect_empty: bool,
    contents: &[String],
) -> bool {
    let top_k = contents.len().max(1) as f64;
    let contains = |needle: &str| {
        let needle = normalize_label(needle);
        contents.iter().any(|item| normalize_label(item).contains(&needle))
    };

    if expect_empty {
        if contents.is_empty() {
            summary.passed_cases += 1;
            summary.empty_when_irrelevant_hits += 1;
            summary.precision_at_k_sum += 1.0;
            summary.recall_at_k_sum += 1.0;
            summary.reciprocal_rank_sum += 1.0;
            return true;
        }
        return false;
    }

    let hits = must_contain.iter().filter(|needle| contains(needle)).count();
    let forbidden_present = must_not_contain.iter().any(|needle| contains(needle));
    let recall = if must_contain.is_empty() {
        1.0
    } else {
        hits as f64 / must_contain.len() as f64
    };
    let precision = if contents.is_empty() {
        0.0
    } else {
        hits as f64 / top_k
    };
    let reciprocal_rank = must_contain
        .iter()
        .find_map(|needle| {
            let normalized = normalize_label(needle);
            contents.iter().position(|item| normalize_label(item).contains(&normalized))
        })
        .map(|index| 1.0 / (index as f64 + 1.0))
        .unwrap_or(0.0);

    summary.recall_at_k_sum += recall;
    summary.precision_at_k_sum += precision;
    summary.reciprocal_rank_sum += reciprocal_rank;

    let case_ok = recall >= 1.0 && !forbidden_present;
    if case_ok {
        summary.passed_cases += 1;
    }
    case_ok
}
```

Update `tests/eval_retrieval.rs` to use it:

```rust
let contents: Vec<String> = items.iter().map(|item| item.content.clone()).collect();
let case_ok = eval_support::metrics::record_retrieval_case(
    &mut summary,
    &case.expected.must_contain,
    &case.expected.must_not_contain,
    case.expected.expect_empty,
    &contents,
);
assert!(case_ok, "retrieval eval case failed: {}", case.id);
```

Add final threshold assertions:

```rust
print_retrieval_summary(&summary);

let total = summary.total_cases as f64;
let recall_at_5 = summary.recall_at_k_sum / total;
let empty_when_irrelevant = summary.empty_when_irrelevant_hits as f64 / total;

assert!(recall_at_5 >= 0.80, "recall_at_5 dropped below 0.80");
assert!(empty_when_irrelevant >= 0.90, "empty_when_irrelevant dropped below 0.90");
```

Extend `tests/fixtures/evals/retrieval_cases.json` with one abstention case:

```json
{
  "id": "ret-002",
  "description": "Unknown passport query should abstain",
  "episodes": [
    {
      "source_type": "chat",
      "source_id": "sess-3",
      "content": "We discussed launch readiness and deployment timing.",
      "t_ref": "2026-03-01T11:00:00Z",
      "scope": "personal"
    }
  ],
  "query": { "query": "what is Bob's passport number", "scope": "personal", "budget": 5, "as_of": null },
  "expected": {
    "must_contain": [],
    "must_not_contain": ["launch readiness"],
    "expect_empty": true,
    "min_recall_at_k": 1.0
  }
}
```

- [ ] **Step 4: Run the retrieval eval to verify it passes**

Run: `cargo test --test eval_retrieval run_retrieval_evals -- --ignored --nocapture --test-threads=1`

Expected: PASS with a summary line such as `suite=eval_retrieval ... recall_at_5=...` and `test run_retrieval_evals ... ok`.

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support tests/eval_retrieval.rs tests/fixtures/evals/retrieval_cases.json
git commit -m "test: add retrieval quality eval runner"
```

### Task 4: Add the latency eval runner with in-memory-only execution

**Status:** ✅ **Done**

**Files:**
- Modify: `tests/eval_support/metrics.rs`
- Modify: `tests/eval_support/report.rs`
- Create: `tests/eval_latency.rs`
- Modify: `tests/fixtures/evals/latency_cases.json`

- [x] **Step 1: Write the failing latency eval**
- [x] **Step 2: Run the latency eval to verify it fails**
- [x] **Step 3: Implement warm-up, measured loops, and explicit in-memory-only comments**
- [x] **Step 4: Run the latency eval to verify it passes**
- [x] **Step 5: Commit**

Create `tests/eval_latency.rs`:

```rust
mod eval_support;

use chrono::{Duration, TimeZone, Utc};
use eval_support::dataset::parse_latency_cases;
use eval_support::metrics::LatencySuiteSummary;
use eval_support::report::print_latency_summary;
use memory_mcp::models::{AssembleContextRequest, IngestRequest};
use std::time::Instant;

#[tokio::test]
#[ignore = "eval: manual latency run"]
async fn run_latency_evals() {
    let raw = std::fs::read_to_string("tests/fixtures/evals/latency_cases.json").unwrap();
    let cases = parse_latency_cases(&raw).unwrap();
    let mut summary = LatencySuiteSummary::default();

    for case in cases {
        let service = eval_support::common::make_service().await;
        for index in 0..case.episode_count {
            let t_ref = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap() + Duration::minutes(index as i64);
            let ingest_started = Instant::now();
            let episode_id = service
                .ingest(
                    IngestRequest {
                        source_type: "eval".to_string(),
                        source_id: format!("{}-{}", case.id, index),
                        content: format!("Project Atlas status note {}", index),
                        t_ref,
                        scope: case.scope.clone(),
                        t_ingested: None,
                        visibility_scope: None,
                        policy_tags: vec![],
                    },
                    None,
                )
                .await
                .unwrap();
            summary.ingest_ms.push(ingest_started.elapsed().as_secs_f64() * 1000.0);

            let extract_started = Instant::now();
            service.extract(&episode_id, None).await.unwrap();
            summary.extract_ms.push(extract_started.elapsed().as_secs_f64() * 1000.0);
        }

        let assemble_started = Instant::now();
        let _items = service
            .assemble_context(AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: None,
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap();
        summary.assemble_ms.push(assemble_started.elapsed().as_secs_f64() * 1000.0);
    }

    print_latency_summary(&summary);
    assert!(!summary.ingest_ms.is_empty(), "latency suite must record ingest timings");
}
```

- [ ] **Step 2: Run the latency eval to verify it fails**

Run: `cargo test --test eval_latency run_latency_evals -- --ignored --nocapture --test-threads=1`

Expected: FAIL because the runner does not yet respect warm-up iterations, measured-iteration loops, or report full p99 values.

- [ ] **Step 3: Implement warm-up, measured loops, and explicit in-memory-only comments**

Update `tests/eval_support/report.rs`:

```rust
pub fn print_latency_summary(summary: &LatencySuiteSummary) {
    println!(
        "suite=eval_latency ingest_p50_ms={:.2} ingest_p95_ms={:.2} ingest_p99_ms={:.2} extract_p50_ms={:.2} extract_p95_ms={:.2} extract_p99_ms={:.2} assemble_p50_ms={:.2} assemble_p95_ms={:.2} assemble_p99_ms={:.2}",
        percentile_ms(&summary.ingest_ms, 0.50),
        percentile_ms(&summary.ingest_ms, 0.95),
        percentile_ms(&summary.ingest_ms, 0.99),
        percentile_ms(&summary.extract_ms, 0.50),
        percentile_ms(&summary.extract_ms, 0.95),
        percentile_ms(&summary.extract_ms, 0.99),
        percentile_ms(&summary.assemble_ms, 0.50),
        percentile_ms(&summary.assemble_ms, 0.95),
        percentile_ms(&summary.assemble_ms, 0.99),
    );
}
```

Update `tests/eval_latency.rs`:

```rust
// Important: this runner must use the existing in-memory test service only.
// Do not switch to RocksDB, remote SurrealDB, or Criterion baseline storage.

for case in cases {
    let service = eval_support::common::make_service().await;

    for index in 0..case.episode_count {
        let t_ref = Utc.with_ymd_and_hms(2026, 3, 1, 9, 0, 0).unwrap() + Duration::minutes(index as i64);
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "eval".to_string(),
                    source_id: format!("{}-seed-{}", case.id, index),
                    content: format!("Project Atlas status note {}", index),
                    t_ref,
                    scope: case.scope.clone(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap();
        service.extract(&episode_id, None).await.unwrap();
    }

    for _ in 0..case.warmup_iterations {
        let _ = service
            .assemble_context(AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: None,
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap();
    }

    for iteration in 0..case.measured_iterations {
        let ingest_started = Instant::now();
        let episode_id = service
            .ingest(
                IngestRequest {
                    source_type: "eval".to_string(),
                    source_id: format!("{}-measure-{}", case.id, iteration),
                    content: format!("Measured Atlas event {}", iteration),
                    t_ref: Utc::now(),
                    scope: case.scope.clone(),
                    t_ingested: None,
                    visibility_scope: None,
                    policy_tags: vec![],
                },
                None,
            )
            .await
            .unwrap();
        summary.ingest_ms.push(ingest_started.elapsed().as_secs_f64() * 1000.0);

        let extract_started = Instant::now();
        service.extract(&episode_id, None).await.unwrap();
        summary.extract_ms.push(extract_started.elapsed().as_secs_f64() * 1000.0);

        let assemble_started = Instant::now();
        let _ = service
            .assemble_context(AssembleContextRequest {
                query: case.query.clone(),
                scope: case.scope.clone(),
                as_of: None,
                budget: 5,
                view_mode: None,
                window_start: None,
                window_end: None,
                access: None,
            })
            .await
            .unwrap();
        summary.assemble_ms.push(assemble_started.elapsed().as_secs_f64() * 1000.0);
    }
}

print_latency_summary(&summary);
assert!(!summary.ingest_ms.is_empty());
assert!(!summary.extract_ms.is_empty());
assert!(!summary.assemble_ms.is_empty());
```

Extend `tests/fixtures/evals/latency_cases.json`:

```json
{
  "id": "lat-002",
  "description": "Medium org-memory retrieval workload",
  "query": "atlas status",
  "scope": "org",
  "episode_count": 100,
  "warmup_iterations": 5,
  "measured_iterations": 20
}
```

- [ ] **Step 4: Run the latency eval to verify it passes**

Run: `cargo test --test eval_latency run_latency_evals -- --ignored --nocapture --test-threads=1`

Expected: PASS with printed `ingest_p50_ms`, `extract_p95_ms`, and `assemble_p99_ms` values and `test run_latency_evals ... ok`.

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support tests/eval_latency.rs tests/fixtures/evals/latency_cases.json
git commit -m "test: add in-memory latency eval runner"
```

### Task 5: Document the eval system and verify the repository

**Status:** ✅ **Done**

**Files:**
- Modify: `README.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Verify only: workspace-wide

- [x] **Step 1: Update `README.md` with manual eval commands and the in-memory-only rule**
- [x] **Step 2: Update `docs/MEMORY_SYSTEM_SPEC.md` to mention metric eval coverage**
- [x] **Step 3: Run formatting and the standard repository verification pipeline**
- [x] **Step 4: Run each eval suite once manually**
- [x] **Step 5: Commit**

Add a section like this:

```md
## Metric eval suites

The repository includes three manual metric eval runners:

- `cargo test --test eval_extraction -- --ignored --nocapture --test-threads=1`
- `cargo test --test eval_retrieval -- --ignored --nocapture --test-threads=1`
- `cargo test --test eval_latency -- --ignored --nocapture --test-threads=1`

Important constraints:

- All DB-backed evals run on embedded in-memory SurrealDB only.
- Eval runs must not persist benchmark state or DB artifacts across sessions.
- Normal `cargo test` remains the correctness suite; metric evals are opt-in.
```

- [ ] **Step 2: Update `docs/MEMORY_SYSTEM_SPEC.md` to mention metric eval coverage**

Add a short subsection under testing/acceptance such as:

```md
### Metric eval harness

In addition to correctness tests, the repository provides manual metric eval runners for:

- retrieval quality (`tests/eval_retrieval.rs`),
- extraction quality (`tests/eval_extraction.rs`),
- latency (`tests/eval_latency.rs`).

All DB-backed eval runs must use embedded in-memory SurrealDB. Eval runners are ignored by default and print summaries to stdout without storing cross-session benchmark history.
```

- [ ] **Step 3: Run formatting and the standard repository verification pipeline**

Run: `cargo fmt --all`

Expected: exits successfully with no diff-producing errors.

Run: `cargo check`

Expected: finishes successfully.

Run: `cargo clippy --all-targets -- -D warnings`

Expected: finishes successfully with zero warnings.

Run: `cargo test`

Expected: correctness suite passes; ignored eval tests remain skipped.

- [ ] **Step 4: Run each eval suite once manually**

Run: `cargo test --test eval_extraction -- --ignored --nocapture --test-threads=1`

Expected: PASS and prints extraction metrics.

Run: `cargo test --test eval_retrieval -- --ignored --nocapture --test-threads=1`

Expected: PASS and prints retrieval metrics.

Run: `cargo test --test eval_latency -- --ignored --nocapture --test-threads=1`

Expected: PASS and prints p50/p95/p99 latency summaries.

- [ ] **Step 5: Commit**

```bash
git add README.md docs/MEMORY_SYSTEM_SPEC.md
git commit -m "docs: document metric eval system"
```

---

## Self-review checklist

- [x] The plan covers all three eval domains from the design: retrieval, extraction, latency.
- [x] The plan explicitly keeps all DB-backed eval and benchmark runs on in-memory SurrealDB only.
- [x] The plan avoids Criterion and persistent benchmark baselines in Phase 1.
- [x] The plan keeps metric evals manual and ignored by default.
- [x] The plan reuses existing repository helpers instead of inventing a second test stack.
- [x] The plan updates operator-facing docs.
- [x] No new public MCP tool is introduced.
- [x] No new database backend is introduced.
- [x] No step uses placeholders such as TBD/TODO.
- [x] Commands and expected outcomes are explicit.
