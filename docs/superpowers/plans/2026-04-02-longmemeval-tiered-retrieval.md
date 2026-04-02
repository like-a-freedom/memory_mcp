# LongMemEval Tiered Retrieval Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expand the LongMemEval benchmark to a larger sample, add tiered reporting so direct-retrieval quality can be measured separately from reasoning-heavy cases, and improve the retrieval pipeline so the direct-retrieval tier can credibly reach Recall@5 ≥ 0.97.

**Architecture:** The work is split into four focused waves: (1) tier metadata and reporting in the eval harness, (2) benchmark expansion via the existing Python converter, (3) retrieval candidate coverage improvements in `assemble_context`, and (4) ranking/fusion tuning after candidate coverage is richer. Each wave produces independently testable changes.

**Tech Stack:** Rust 2024, serde/serde_json, SurrealDB, Python 3 for fixture generation, existing eval harness (`tests/eval_support/`), `src/service/context.rs`, `src/service/core.rs`

---

## File Map

| File | Role in this plan |
|---|---|
| `tests/eval_support/dataset.rs` | Add `tier` field to `RetrievalEvalCase` and `RetrievalExpectation` |
| `tests/eval_support/metrics.rs` | Add tier-aware `RetrievalSuiteSummary` and per-tier aggregation |
| `tests/eval_support/report.rs` | Print tiered summaries alongside the aggregate score |
| `tests/eval_external_retrieval.rs` | Update LongMemEval runner to print tiered output |
| `tests/fixtures/evals/retrieval_longmemeval.json` | Regenerate with tier labels and larger sample |
| `scripts/convert_external_evals.py` | Add tier classification logic and `--max-cases` expansion |
| `src/service/context.rs` | Improve query normalization, alias expansion, and entity-graph expansion |
| `src/service/core.rs` | Verify `build_fact_index_keys` covers LongMemEval-style temporal markers |
| `src/service/episode.rs` | No changes expected unless extraction audit shows missing fact materialization |

---

## Wave 1 — Tier metadata and reporting in the eval harness

### Task 1: Add `tier` field to retrieval fixture schema

**Files:**
- Modify: `tests/eval_support/dataset.rs`

- [ ] **Step 1: Add `tier` field to `RetrievalExpectation`**

Add a new `tier` field to the `RetrievalExpectation` struct so each fixture case can declare which retrieval ability it exercises. The field should be a simple string enum with these values: `direct`, `alias`, `temporal`, `graph`, `reasoning`.

```rust
#[derive(Debug, Deserialize, Clone)]
pub struct RetrievalExpectation {
    pub must_contain: Vec<String>,
    pub must_not_contain: Vec<String>,
    pub expect_empty: bool,
    pub min_recall_at_k: f64,
    /// Retrieval ability tier for this case.
    /// One of: "direct", "alias", "temporal", "graph", "reasoning"
    #[serde(default = "default_tier")]
    pub tier: String,
}

fn default_tier() -> String {
    "direct".to_string()
}
```

- [ ] **Step 2: Add validation for tier values**

Extend `validate_retrieval_cases` in `tests/eval_support/dataset.rs` to reject unknown tier values:

```rust
const VALID_TIERS: &[&str] = &["direct", "alias", "temporal", "graph", "reasoning"];

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
        if !VALID_TIERS.contains(&case.expected.tier.as_str()) {
            return Err(format!(
                "retrieval case {} has invalid tier '{}', expected one of {:?}",
                case.id, case.expected.tier, VALID_TIERS
            ));
        }
    }
    Ok(())
}
```

- [ ] **Step 3: Run existing eval tests to verify schema change is backwards-compatible**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (existing fixtures without explicit `tier` should default to `"direct"`)

- [ ] **Step 4: Commit**

```bash
git add tests/eval_support/dataset.rs
git commit -m "feat: add tier field to retrieval eval schema with validation"
```

### Task 2: Add tier-aware metrics aggregation

**Files:**
- Modify: `tests/eval_support/metrics.rs`

- [ ] **Step 1: Add per-tier summary tracking**

Extend `RetrievalSuiteSummary` to track per-tier metrics:

```rust
#[derive(Debug, Default, Clone)]
pub struct RetrievalSuiteSummary {
    pub total_cases: usize,
    pub passed_cases: usize,
    pub recall_at_k_sum: f64,
    pub precision_at_k_sum: f64,
    pub reciprocal_rank_sum: f64,
    pub empty_when_irrelevant_hits: usize,
    /// Per-tier breakdowns: tier_name -> (total, passed, recall_sum)
    pub tier_totals: std::collections::BTreeMap<String, (usize, usize, f64)>,
}
```

- [ ] **Step 2: Update `record_retrieval_case` to track tier**

Add a `tier` parameter to `record_retrieval_case` and update the per-tier tracking:

```rust
pub fn record_retrieval_case(
    summary: &mut RetrievalSuiteSummary,
    must_contain: &[String],
    must_not_contain: &[String],
    expect_empty: bool,
    contents: &[String],
    tier: &str,
) -> bool {
    // ... existing recall/precision/reciprocal_rank computation unchanged ...

    // Update per-tier tracking
    let entry = summary
        .tier_totals
        .entry(tier.to_string())
        .or_insert((0, 0, 0.0));
    entry.0 += 1;
    if case_ok {
        entry.1 += 1;
    }
    entry.2 += recall;

    case_ok
}
```

- [ ] **Step 3: Add tier summary computation helper**

Add a helper function to compute per-tier recall:

```rust
pub fn tier_recall(summary: &RetrievalSuiteSummary, tier: &str) -> Option<f64> {
    summary
        .tier_totals
        .get(tier)
        .filter(|(total, _, _)| *total > 0)
        .map(|(total, _, recall_sum)| recall_sum / *total as f64)
}

pub fn tier_pass_rate(summary: &RetrievalSuiteSummary, tier: &str) -> Option<f64> {
    summary
        .tier_totals
        .get(tier)
        .filter(|(total, _, _)| *total > 0)
        .map(|(total, passed, _)| *passed as f64 / *total as f64)
}
```

- [ ] **Step 4: Run existing eval tests to verify backwards compatibility**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (callers will need updating in Task 3)

- [ ] **Step 5: Commit**

```bash
git add tests/eval_support/metrics.rs
git commit -m "feat: add per-tier metrics tracking to retrieval eval harness"
```

### Task 3: Update eval runners to pass tier parameter

**Files:**
- Modify: `tests/eval_retrieval.rs`
- Modify: `tests/eval_external_retrieval.rs`

- [ ] **Step 1: Update `eval_retrieval.rs` to pass tier from fixture**

Update the `record_retrieval_case` call in `run_retrieval_evals` to include the tier:

```rust
let case_ok = eval_support::metrics::record_retrieval_case(
    &mut summary,
    &case.expected.must_contain,
    &case.expected.must_not_contain,
    case.expected.expect_empty,
    &contents,
    &case.expected.tier,
);
```

- [ ] **Step 2: Update `eval_external_retrieval.rs` to pass tier from fixture**

Update the `record_retrieval_case` call in `run_external_retrieval_suite` to include the tier:

```rust
let case_ok = eval_support::metrics::record_retrieval_case(
    &mut summary,
    &case.expected.must_contain,
    &case.expected.must_not_contain,
    case.expected.expect_empty,
    &contents,
    &case.expected.tier,
);
```

- [ ] **Step 3: Run both eval test files to verify compilation**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Run: `cargo test --test eval_external_retrieval -- --test-threads=1`
Expected: Both compile and pass (LongMemEval will still show ~0.49 aggregate until Wave 2)

- [ ] **Step 4: Commit**

```bash
git add tests/eval_retrieval.rs tests/eval_external_retrieval.rs
git commit -m "feat: pass tier parameter from fixtures to metrics recorder"
```

### Task 4: Add tiered report output

**Files:**
- Modify: `tests/eval_support/report.rs`

- [ ] **Step 1: Update `print_retrieval_summary` to print tier breakdowns**

Add tier breakdown printing after the aggregate summary:

```rust
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

    // Print per-tier breakdowns
    if !summary.tier_totals.is_empty() {
        println!("--- tier breakdown ---");
        for (tier, (total, passed, recall_sum)) in &summary.tier_totals {
            let recall = recall_sum / *total.max(1) as f64;
            let pass_rate = *passed as f64 / *total.max(1) as f64;
            println!(
                "  tier={tier} total={total} passed={passed} recall_at_5={recall:.3} pass_rate={pass_rate:.3}"
            );
        }
    }
}
```

- [ ] **Step 2: Run eval tests to verify tiered output appears**

Run: `cargo test --test eval_retrieval -- --test-threads=1 --nocapture`
Expected: Output includes `--- tier breakdown ---` section with per-tier stats

- [ ] **Step 3: Commit**

```bash
git add tests/eval_support/report.rs
git commit -m "feat: print per-tier breakdowns in retrieval eval summary"
```

---

## Wave 2 — Benchmark expansion via Python converter

### Task 5: Add tier classification to LongMemEval converter

**Files:**
- Modify: `scripts/convert_external_evals.py`

- [ ] **Step 1: Add tier classification function**

Add a function that classifies LongMemEval cases into retrieval tiers based on question patterns:

```python
import re

def classify_longmemeval_tier(question: str, answer: str, sessions: list) -> str:
    """Classify a LongMemEval case into a retrieval tier.
    
    Returns one of: "direct", "alias", "temporal", "graph", "reasoning"
    """
    q_lower = question.lower()
    
    # Temporal reasoning: explicit date arithmetic or ordering questions
    temporal_patterns = [
        r"how many days?\s+(before|after|between)",
        r"how long\s+(had|did|have)",
        r"how many months?\s+(before|after|between)",
        r"how many weeks?\s+(before|after|between)",
        r"which.*first",
        r"which.*last",
        r"which.*earlier",
        r"which.*later",
        r"what.*date.*when",
        r"what time.*on",
    ]
    for pattern in temporal_patterns:
        if re.search(pattern, q_lower):
            return "temporal"
    
    # Multi-session / graph: questions that require connecting facts across sessions
    # or involve relationships between entities
    if len(sessions) > 2:
        # Heavy multi-session cases are more likely reasoning-heavy
        multi_session_indicators = [
            r"who.*knows",
            r"who.*introduce",
            r"relationship",
            r"connected",
            r"introduction",
        ]
        for pattern in multi_session_indicators:
            if re.search(pattern, q_lower):
                return "graph"
    
    # Alias / name resolution: questions about specific named entities
    # where the answer contains proper nouns not directly in the question
    answer_words = set(w.lower() for w in re.findall(r'\b\w+\b', answer) if len(w) > 3)
    question_words = set(w.lower() for w in re.findall(r'\b\w+\b', question) if len(w) > 3)
    new_entity_words = answer_words - question_words
    
    # If answer introduces significant new named entities, likely alias resolution
    if len(new_entity_words) >= 2 and any(w[0].isupper() for w in new_entity_words):
        return "alias"
    
    # Direct retrieval: question terms appear in answer or context
    # This is the default for cases where query terms overlap with expected content
    return "direct"
```

- [ ] **Step 2: Integrate tier classification into `convert_longmemeval`**

Update the case generation loop to include the tier:

```python
def convert_longmemeval(input_path: str, output_dir: str, max_cases: int = 50):
    """Convert LongMemEval oracle instances into retrieval eval fixtures."""
    with open(input_path, "r") as f:
        data = json.load(f)

    priority_types = {"knowledge-update", "temporal-reasoning", "multi-session"}
    priority = [d for d in data if d["question_type"] in priority_types]
    abstention = [d for d in data if d["question_id"].endswith("_abs")]
    other = [d for d in data if d not in priority and d not in abstention]
    ordered = (priority + abstention + other)[:max_cases]
    cases = []

    for item in ordered:
        qid = item["question_id"]
        qtype = item["question_type"]
        question = item["question"]
        answer = item["answer"]
        sessions = item["haystack_sessions"]
        haystack_dates = item.get("haystack_dates", [])

        # ... existing episode generation unchanged ...

        tier = classify_longmemeval_tier(question, answer, sessions)

        cases.append({
            "id": f"lme-{qid}",
            "description": f"LongMemEval {qtype}: {question[:80]}",
            "episodes": episodes,
            "query": {
                "query": question,
                "scope": "personal",
                "budget": 10,
                "as_of": None,
            },
            "expected": {
                "must_contain": must_contain,
                "must_not_contain": [],
                "expect_empty": is_abstention,
                "min_recall_at_k": 1.0,
                "tier": tier,
            },
        })

    # ... existing file write unchanged ...
```

- [ ] **Step 3: Run the converter to regenerate fixtures**

Run: `python3 scripts/convert_external_evals.py --longmemeval data/eval_external/longmemeval_oracle.json --max-cases 50`
Expected: `tests/fixtures/evals/retrieval_longmemeval.json` is regenerated with `tier` fields

- [ ] **Step 4: Verify fixture validity**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (fixtures parse correctly with tier field)

- [ ] **Step 5: Commit**

```bash
git add scripts/convert_external_evals.py tests/fixtures/evals/retrieval_longmemeval.json
git commit -m "feat: add tier classification to LongMemEval fixture generation"
```

### Task 6: Expand benchmark sample size

**Files:**
- Modify: `scripts/convert_external_evals.py` (already supports `--max-cases`)
- Modify: `tests/fixtures/evals/retrieval_longmemeval.json`

- [ ] **Step 1: Generate expanded fixture**

Run: `python3 scripts/convert_external_evals.py --longmemeval data/eval_external/longmemeval_oracle.json --max-cases 200`
Expected: Larger fixture file with more cases across all tiers

- [ ] **Step 2: Verify expanded fixture parses correctly**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS

- [ ] **Step 3: Commit**

```bash
git add tests/fixtures/evals/retrieval_longmemeval.json
git commit -m "feat: expand LongMemEval fixture to 200 cases for statistical confidence"
```

---

## Wave 3 — Retrieval candidate coverage improvements

### Task 7: Improve query normalization for temporal and multi-word queries

**Files:**
- Modify: `src/service/context.rs`

- [ ] **Step 1: Identify the current query preprocessing function**

Locate `preprocess_search_query` in `src/service/core.rs` or `src/service/query.rs` and review its current behavior.

- [ ] **Step 2: Add temporal synonym expansion**

Extend query preprocessing to add temporal synonyms for common date expressions. For example, "last month" should expand to include the actual month name and year based on `as_of` context:

```rust
fn expand_temporal_synonyms(query: &str, as_of: Option<DateTime<Utc>>) -> Vec<String> {
    let mut expansions = vec![query.to_string()];
    let q_lower = query.to_lowercase();
    
    if let Some(as_of_dt) = as_of {
        // "last month" -> actual month name
        if q_lower.contains("last month") {
            let last_month = as_of_dt - chrono::Duration::days(30);
            let month_name = last_month.format("%B").to_string();
            let year = last_month.format("%Y").to_string();
            let expanded = query
                .replace("last month", &format!("{} {}", month_name, year))
                .replace("Last month", &format!("{} {}", month_name, year));
            expansions.push(expanded);
        }
        
        // "this month" -> current month name
        if q_lower.contains("this month") {
            let month_name = as_of_dt.format("%B").to_string();
            let year = as_of_dt.format("%Y").to_string();
            let expanded = query
                .replace("this month", &format!("{} {}", month_name, year))
                .replace("This month", &format!("{} {}", month_name, year));
            expansions.push(expanded);
        }
    }
    
    expansions
}
```

- [ ] **Step 3: Integrate temporal expansion into `assemble_context`**

Update the query preparation in `assemble_context` to use temporal expansion before FTS:

```rust
let cleaned_query = super::preprocess_search_query(&request.query);
let temporal_expansions = expand_temporal_synonyms(&cleaned_query, request.as_of);

// Use all expansions as candidate queries
let all_queries = if temporal_expansions.len() > 1 {
    temporal_expansions
} else {
    vec![cleaned_query.clone()]
};
```

- [ ] **Step 4: Run retrieval tests to verify no regression**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (same or better recall)

- [ ] **Step 5: Commit**

```bash
git add src/service/context.rs
git commit -m "feat: add temporal synonym expansion to query preprocessing"
```

### Task 8: Improve alias expansion for entity-centric queries

**Files:**
- Modify: `src/service/context.rs`

- [ ] **Step 1: Review current `expand_query_with_aliases` function**

The function already exists at `src/service/context.rs:531`. Review its behavior and identify gaps:
- It only expands when entities are found in the DB
- It uses n-gram phrases from the query
- It may miss partial name matches

- [ ] **Step 2: Add partial name matching for alias expansion**

Extend the entity lookup to also try partial matches when exact phrase matches fail:

```rust
async fn expand_query_with_aliases(
    service: &crate::service::MemoryService,
    query: &str,
    namespace: &str,
) -> Vec<String> {
    let terms: Vec<&str> = query.split_whitespace().collect();
    if terms.is_empty() {
        return Vec::new();
    }

    // ... existing n-gram phrase collection unchanged ...

    // Add partial name matching: try individual capitalized words as entity names
    let mut partial_names: Vec<String> = Vec::new();
    for term in &terms {
        if term.len() >= 3 && term.chars().next().map(|c| c.is_uppercase()).unwrap_or(false) {
            partial_names.push(super::normalize_text(term));
        }
    }

    // Combine with existing phrase lookups
    let all_lookup_names: Vec<String> = normalized_names
        .into_iter()
        .chain(partial_names)
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    // ... existing batch lookup and expansion unchanged, using all_lookup_names ...
}
```

- [ ] **Step 3: Run retrieval tests to verify alias expansion improvement**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (same or better recall, especially on alias-tier cases)

- [ ] **Step 4: Commit**

```bash
git add src/service/context.rs
git commit -m "feat: add partial name matching to alias expansion for entity queries"
```

### Task 9: Add 2-hop graph expansion for relationship queries

**Files:**
- Modify: `src/service/context.rs`

- [ ] **Step 1: Review current `collect_entity_expansion_facts` function**

The function at `src/service/context.rs:935` already does 1-hop expansion. Identify where to add 2-hop for relationship-shaped queries.

- [ ] **Step 2: Add relationship query detection**

Add a helper to detect relationship queries that warrant 2-hop expansion:

```rust
fn is_relationship_query(query: &str) -> bool {
    let q_lower = query.to_lowercase();
    q_lower.contains("who knows")
        || q_lower.contains("introduce")
        || q_lower.contains("connected to")
        || q_lower.contains("relationship")
        || q_lower.contains("knows who")
        || q_lower.contains("mutual")
        || q_lower.contains("connection")
}
```

- [ ] **Step 3: Extend `collect_entity_expansion_facts` for 2-hop**

Update the function to do 2-hop expansion when the query is relationship-shaped:

```rust
// After existing 1-hop neighbor collection:
let do_2hop = is_relationship_query(request.query);
if do_2hop {
    let first_hop_ids: Vec<String> = all_entity_ids.clone();
    for entity_id in first_hop_ids {
        for direction in [GraphDirection::Incoming, GraphDirection::Outgoing] {
            let neighbors = service
                .db_client
                .select_edge_neighbors(request.namespace, &entity_id, request.cutoff_iso, direction)
                .await
                .unwrap_or_default();

            for neighbor in neighbors {
                if let Some(neighbor_id) = neighbor.get("neighbor_id").and_then(|v| v.as_str()) {
                    let neighbor_str = neighbor_id.to_string();
                    if !all_entity_ids.contains(&neighbor_str) {
                        all_entity_ids.push(neighbor_str);
                    }
                }
            }
        }
    }
}
```

- [ ] **Step 4: Run retrieval tests to verify graph expansion**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (same or better recall, especially on graph-tier cases)

- [ ] **Step 5: Commit**

```bash
git add src/service/context.rs
git commit -m "feat: add 2-hop graph expansion for relationship queries"
```

---

## Wave 4 — Ranking tuning and verification

### Task 10: Retune RRF fusion after candidate coverage improvements

**Files:**
- Modify: `src/service/context.rs`

- [ ] **Step 1: Review current RRF parameters**

The current `RECIPROCAL_RANK_FUSION_K` is `10.0` at `src/service/context.rs:14`. Review whether this value is optimal after the candidate pool has been enriched.

- [ ] **Step 2: Add source-aware RRF weighting**

Update `build_ranked_context_facts` to apply different RRF weights based on retrieval source priority:

```rust
fn build_ranked_context_facts(
    direct_facts: Vec<crate::models::Fact>,
    community_facts: Vec<(crate::models::Fact, String)>,
    semantic_facts: Vec<(crate::models::Fact, String)>,
    query_opt: Option<&str>,
    scope: &str,
    cutoff: chrono::DateTime<chrono::Utc>,
) -> Vec<RankedContextFact> {
    let mut ranked_by_fact_id = std::collections::HashMap::<String, RankedContextFact>::new();

    // Direct lexical matches get full RRF weight
    for (rank, fact) in direct_facts.into_iter().enumerate() {
        let fact_id = fact.fact_id.clone();
        let confidence = super::decayed_confidence(&fact, cutoff);
        let rrf_weight = 1.0; // Full weight for direct matches
        ranked_by_fact_id
            .entry(fact_id)
            .and_modify(|candidate| {
                candidate.fusion_score += reciprocal_rank(rank) * rrf_weight;
                candidate.source_priority = 0;
                candidate.decayed_confidence = candidate.decayed_confidence.max(confidence);
            })
            .or_insert_with(|| RankedContextFact {
                rationale: default_direct_rationale(query_opt, scope, cutoff),
                fact,
                fusion_score: reciprocal_rank(rank) * rrf_weight,
                source_priority: 0,
                decayed_confidence: confidence,
            });
    }

    // Community facts get reduced weight (0.7) since they're indirect
    let community_weight = 0.7;
    for (rank, (fact, rationale)) in community_facts.into_iter().enumerate() {
        // ... existing logic with community_weight applied ...
    }

    // Semantic/graph facts get reduced weight (0.5) since they're further indirect
    let semantic_weight = 0.5;
    for (rank, (fact, rationale)) in semantic_facts.into_iter().enumerate() {
        // ... existing logic with semantic_weight applied ...
    }

    ranked_by_fact_id.into_values().collect()
}
```

- [ ] **Step 3: Run retrieval tests to verify ranking improvement**

Run: `cargo test --test eval_retrieval -- --test-threads=1`
Expected: PASS (same or better recall with improved ranking)

- [ ] **Step 4: Commit**

```bash
git add src/service/context.rs
git commit -m "feat: add source-aware RRF weighting for retrieval fusion"
```

### Task 11: Run full benchmark and verify tiered output

**Files:**
- No code changes — verification only

- [ ] **Step 1: Run the LongMemEval eval with tiered reporting**

Run: `cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1`
Expected: Output shows aggregate Recall@5 and per-tier breakdowns

- [ ] **Step 2: Verify direct-retrieval tier Recall@5 ≥ 0.97**

Check the tiered output for the `direct` tier. If it's below 0.97, iterate on Tasks 7-10 until it reaches the target.

- [ ] **Step 3: Run the full repository quality gate**

Run: `cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps`
Expected: All checks pass

- [ ] **Step 4: Commit final state**

```bash
git add .
git commit -m "feat: LongMemEval tiered retrieval reaches 0.97 direct-recall target"
```

---

## Self-Review Checklist

- **Spec coverage:** This plan covers all requirements from the session plan: tier metadata (Task 1-2), benchmark expansion (Task 5-6), retrieval candidate improvements (Task 7-9), ranking tuning (Task 10), and verification (Task 11). The current 50-case oracle is preserved as the baseline, and the expanded fixture adds statistical confidence.

- **Placeholder scan:** No `TODO`, `TBD`, or implicit 'figure it out later' steps remain. Each task names files, concrete code snippets, commands, and expected outcomes.

- **Type consistency:** The plan preserves the public MCP contract and existing retrieval types. New `tier` field is additive with a default value, so existing fixtures without explicit tier continue to work. The `RetrievalSuiteSummary` extension adds a new field but doesn't break existing consumers.

- **Backwards compatibility:** All changes are backwards-compatible:
  - `tier` field defaults to `"direct"` for existing fixtures
  - `RetrievalSuiteSummary` adds a new field with `Default` implementation
  - `record_retrieval_case` gains a new parameter but all callers are updated in the same wave
  - Retrieval pipeline changes are additive (new expansions, not replacements)

- **Test strategy:** Each wave includes verification steps. The tiered reporting allows measuring whether improvements come from direct retrieval gains or from benchmark reshaping.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-04-02-longmemeval-tiered-retrieval.md`. Two execution options:

**1. Subagent-Driven (recommended)** - I dispatch a fresh subagent per task, review between tasks, fast iteration

**2. Inline Execution** - Execute tasks in this session using executing-plans, batch execution with checkpoints

Which approach?
