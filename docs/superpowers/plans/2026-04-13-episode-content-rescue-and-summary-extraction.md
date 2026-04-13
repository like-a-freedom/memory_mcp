# Episode Content Rescue and Summary Extraction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `assemble_context` return relevant episode-backed results when matching episodes exist but extracted facts are missing, and make `extract` produce at least one safe searchable fact for concise summary-like requirement/engagement episodes.

**Architecture:** Keep fact retrieval as the primary path, but add an episode-content rescue path that can beat weak noisy fact fallback when query-aligned episodes exist. In parallel, broaden extraction with a conservative `note` fallback for summary-like episodes that currently produce entities and links but `facts=0`, so newly ingested content stops becoming a retrieval dead end.

**Tech Stack:** Rust, Tokio, SurrealDB, existing `MemoryService` retrieval/extraction pipeline, current `tests/service_acceptance.rs` and `tests/service_integration.rs` coverage.

---

## Review assessment to carry into implementation

- The external review is **correct** that the current extraction pipeline is narrow: `src/service/episode/mod.rs::extract_facts()` only emits `metric`, `promise`, and `experience` facts, plus document-style action items via `src/service/statement_detection.rs`. Summary-like requirement/customer/task episodes can easily produce `entities > 0` and `facts = 0`.
- The external review is **incomplete** about retrieval: the system **does** have an episode-content fallback (`src/service/context/lexical.rs::select_episode_records_for_query()` + `src/service/context/views.rs::build_episode_fallback_items()`), but `src/service/context/mod.rs::assemble_default_context()` only consults it when `ranked_facts.is_empty()`. If weak noisy fact candidates exist, matching episodes never surface.
- Implementation should therefore fix **both** layers:
  1. retrieval rescue for stored episode content,
  2. safer extraction fallback for summary-like episodes,
  3. logging so future sessions show whether zero-fact extraction or rescue-path suppression happened.

### Task 1: Reproduce the two missing-data failure modes

**Files:**
- Modify: `tests/service_acceptance.rs`
- Modify: `tests/service_integration.rs`
- Read: `src/service/context/mod.rs`
- Read: `src/service/context/views.rs`
- Read: `src/service/episode/mod.rs`

- [ ] **Step 1: Write the failing retrieval regression for episode-content rescue**

```rust
#[tokio::test]
async fn test_query_prefers_matching_episode_content_over_irrelevant_fact_fallback() {
    use memory_mcp::models::IngestRequest;

    let service = common::make_service().await;
    let july = Utc.with_ymd_and_hms(2025, 7, 14, 10, 0, 0).unwrap();

    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "requirement".to_string(),
                source_id: "july-platform-planning".to_string(),
                content: "Platform planning notes July 2025: release scope, integrations, and response workflow updates.".to_string(),
                t_ref: july,
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest July episode");

    service
        .add_fact(
            "note",
            "October 2025 licensing update for an unrelated product area.",
            "October 2025 licensing update for an unrelated product area.",
            "episode:october-noise",
            Utc.with_ymd_and_hms(2025, 10, 13, 10, 0, 0).unwrap(),
            "org",
            0.9,
            vec![],
            vec![],
            serde_json::json!({"source_episode": "episode:october-noise"}),
        )
        .await
        .expect("seed unrelated fact noise");

    let items = service
        .assemble_context(memory_mcp::models::AssembleContextRequest {
            query: "Platform planning notes July 2025".to_string(),
            scope: "org".to_string(),
            as_of: Some(Utc.with_ymd_and_hms(2026, 4, 13, 12, 0, 0).unwrap()),
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

    let first = items.first().expect("expected at least one result");
    assert!(
        first.fact_id.starts_with("episode_fallback:"),
        "expected episode fallback item, got {first:?}"
    );
    assert_eq!(first.source_episode, episode_id);
    assert_eq!(first.retrieval_tier.as_deref(), Some("fallback"));
}
```

- [ ] **Step 2: Run the retrieval regression to verify it fails**

Run: `cargo test --test service_acceptance test_query_prefers_matching_episode_content_over_irrelevant_fact_fallback -- --nocapture`
Expected: FAIL because the current implementation keeps noisy fact fallback whenever `ranked_facts` is non-empty and never consults episode content.

- [ ] **Step 3: Write the failing extraction regression for summary-like episodes**

```rust
#[tokio::test]
async fn test_extract_generates_note_fact_for_summary_requirement_episode() {
    use memory_mcp::models::IngestRequest;

    let service = common::make_service().await;
    let episode_id = service
        .ingest(
            IngestRequest {
                source_type: "requirement".to_string(),
                source_id: "summary-requirement-1".to_string(),
                content: "July 2025 planning summary: platform integrations ready, stakeholder approvals pending, response workflow scoped.".to_string(),
                t_ref: Utc.with_ymd_and_hms(2025, 7, 10, 9, 0, 0).unwrap(),
                scope: "org".to_string(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest summary episode");

    let extraction = service
        .extract(&episode_id, None, None)
        .await
        .expect("extract summary episode");

    assert!(
        extraction.facts.iter().any(|fact| fact.fact_type == "note"),
        "expected summary-like requirement episode to produce a note fact, got {extraction:?}"
    );
}
```

- [ ] **Step 4: Run the extraction regression to verify it fails**

Run: `cargo test --test service_integration test_extract_generates_note_fact_for_summary_requirement_episode -- --nocapture`
Expected: FAIL with `facts=0` because current extraction only creates `metric`, `promise`, or `experience` facts.

- [ ] **Step 5: Commit the failing tests**

```bash
git add tests/service_acceptance.rs tests/service_integration.rs
git commit -m "test: capture missing fact and episode rescue gaps"
```

### Task 2: Add a conservative `note` extraction fallback for summary-like episodes

**Files:**
- Modify: `src/service/episode/mod.rs`
- Modify: `src/service/statement_detection.rs`
- Test: `tests/service_integration.rs`

- [ ] **Step 1: Add a helper that recognizes summary-like content worth storing as a `note` fact**

```rust
// src/service/statement_detection.rs
pub fn is_summary_like_note_candidate(content: &str) -> bool {
    let normalized_terms = crate::service::query::search_query_terms(content);
    normalized_terms.len() >= 6
}
```

- [ ] **Step 2: Add a source-type-aware fallback gate in extraction**

```rust
// src/service/episode/mod.rs
fn should_extract_note_fact(episode: &Episode, facts: &[ExtractedFact]) -> bool {
    if !facts.is_empty() {
        return false;
    }

    let supported_source_type = matches!(
        episode.source_type.as_str(),
        "requirement" | "task_tracking" | "stakeholder_mapping" | "customer_engagement" | "email"
    );

    supported_source_type && is_summary_like_note_candidate(&episode.content)
}
```

- [ ] **Step 3: Emit `FactType::Note` when the fallback gate passes**

```rust
// inside extract_facts(...)
if should_extract_note_fact(episode, &facts) {
    facts.push(
        add_extracted_fact(service, episode, FactType::Note.as_str(), &entity_links).await?,
    );
}
```

- [ ] **Step 4: Add a small debug log when note fallback fires**

```rust
service.logger.log(
    super::log_event(
        "extract.note_fallback",
        json!({
            "episode_id": episode.episode_id,
            "source_type": episode.source_type,
        }),
        json!({
            "content_chars": episode.content.chars().count(),
        }),
        None,
        None,
        None,
    ),
    LogLevel::Debug,
);
```

- [ ] **Step 5: Run the targeted extraction test and the current extraction suite**

Run: `cargo test --test service_integration test_extract_generates_note_fact_for_summary_requirement_episode -- --nocapture`
Expected: PASS

Run: `cargo test --test promise_detection -- --nocapture`
Expected: PASS (existing promise extraction still works)

- [ ] **Step 6: Commit the extraction fix**

```bash
git add src/service/episode/mod.rs src/service/statement_detection.rs tests/service_integration.rs
git commit -m "feat: extract note facts from summary-like episodes"
```

### Task 3: Let episode content compete with weak fact fallback in `assemble_context`

**Files:**
- Modify: `src/service/context/mod.rs`
- Modify: `src/service/context/lexical.rs`
- Modify: `src/service/context/views.rs`
- Test: `tests/service_acceptance.rs`

- [ ] **Step 1: Add a text-overlap helper that works for both facts and raw episode content**

```rust
// src/service/context/lexical.rs
pub(crate) fn lexical_query_overlap_for_text(text: &str, query_terms: &[String]) -> usize {
    let content_terms = search_query_terms(text);
    let content_terms = content_terms.iter().collect::<HashSet<_>>();

    query_terms
        .iter()
        .filter(|term| content_terms.contains(term))
        .count()
}
```

- [ ] **Step 2: Build episode fallback candidates before final return when a query exists**

```rust
// src/service/context/mod.rs
let episode_fallback_items = if let Some(query) = params.query_opt {
    let episode_records = select_episode_records_for_query(
        service,
        params.namespace,
        params.scope,
        params.cutoff_iso,
        Some(query),
        params.budget,
        params.project_opt,
    )
    .await?;

    build_episode_fallback_items(EpisodeFallbackParams {
        episodes: filtering::filter_episodes_by_constraints(
            episode_records,
            params.access,
            params.project_opt,
        ),
        query_opt: Some(query),
        scope: params.scope,
        cutoff: params.cutoff,
        window_start: params.window_start,
        window_end: params.window_end,
        timeline_mode: params.view_mode == Some("timeline"),
        budget: params.budget,
        fallback_rationale_fn: ranking::default_episode_fallback_rationale,
    })
} else {
    Vec::new()
};
```

- [ ] **Step 3: Prefer episode items when selected facts are weaker than episode content**

```rust
fn should_prefer_episode_content(
    selected_facts: &[ranking::RankedContextFact],
    episode_items: &[AssembledContextItem],
    query_terms: &[String],
) -> bool {
    if episode_items.is_empty() {
        return false;
    }

    let best_fact_overlap = selected_facts
        .iter()
        .map(|fact| lexical::lexical_query_overlap_for_fact(&fact.fact, query_terms))
        .max()
        .unwrap_or(0);

    let best_episode_overlap = episode_items
        .iter()
        .map(|item| lexical::lexical_query_overlap_for_text(&item.content, query_terms))
        .max()
        .unwrap_or(0);

    best_episode_overlap > best_fact_overlap
}
```

- [ ] **Step 4: Switch the return path in `assemble_default_context()` when rescue wins**

```rust
let selected_ranked = select_ranked_context_facts(
    ranked_facts,
    params.budget.max(1) as usize,
    temporal_focus,
    params.query_terms.to_vec(),
);

if should_prefer_episode_content(&selected_ranked, &episode_fallback_items, params.query_terms) {
    return Ok(episode_fallback_items);
}

Ok(selected_ranked
    .into_iter()
    .map(|ranked| ranked_fact_to_item(ranked, params.cutoff))
    .collect())
```

- [ ] **Step 5: Run the retrieval regression and the existing acceptance file**

Run: `cargo test --test service_acceptance test_query_prefers_matching_episode_content_over_irrelevant_fact_fallback -- --nocapture`
Expected: PASS

Run: `cargo test --test service_acceptance -- --nocapture`
Expected: PASS

- [ ] **Step 6: Commit the retrieval rescue**

```bash
git add src/service/context/mod.rs src/service/context/lexical.rs src/service/context/views.rs tests/service_acceptance.rs
git commit -m "feat: rescue query results from matching episode content"
```

### Task 4: Make zero-fact extraction and rescue-path decisions visible in logs

**Files:**
- Modify: `src/service/episode/mod.rs`
- Modify: `src/service/context/mod.rs`
- Modify: `src/mcp/handlers.rs`
- Test: `tests/service_acceptance.rs`

- [ ] **Step 1: Expand extract logs with fields that explain `facts=0` cases**

```rust
// include in extract_from_episode.done / extract.done result payloads
json!({
    "entities": entities.len(),
    "facts": facts.len(),
    "warnings": warnings.len(),
    "source_type": episode.source_type,
    "content_chars": episode.content.chars().count(),
    "note_fallback_used": facts.iter().any(|fact| fact.fact_type == "note"),
})
```

- [ ] **Step 2: Log whether episode rescue candidates existed and whether they won**

```rust
service.logger.log(
    super::log_event(
        "assemble_context.episode_rescue",
        json!({"scope": params.scope, "query": params.query_opt}),
        json!({
            "episode_candidate_count": episode_fallback_items.len(),
            "selected_fact_count": selected_ranked.len(),
            "episode_rescue_used": prefer_episode_content,
        }),
        Some(params.access),
        None,
        None,
    ),
    LogLevel::Debug,
);
```

- [ ] **Step 3: Run a targeted command and confirm logs become explanatory**

Run: `cargo test --test service_acceptance test_query_prefers_matching_episode_content_over_irrelevant_fact_fallback -- --nocapture`
Expected: PASS with logs showing `episode_candidate_count > 0` and `episode_rescue_used = true`

- [ ] **Step 4: Commit logging improvements**

```bash
git add src/service/episode/mod.rs src/service/context/mod.rs src/mcp/handlers.rs
git commit -m "chore: log zero-fact extraction and episode rescue decisions"
```

### Task 5: Full verification and doc touch-up

**Files:**
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Read: `tests/eval_extraction.rs`
- Read: `tests/service_acceptance.rs`
- Read: `tests/service_integration.rs`

- [ ] **Step 1: Update extraction status in the spec**

```md
- **FR-EX-02**: System MUST extract facts/items: `Promise`, `Task`, `Metric`, `Decision`, `Opinion`/`Preference`, `Relationship`.
- **Status**: ⚠️ Partial — extraction now covers `metric`, `promise`, `experience`, and conservative `note` fallback for summary-like episodes; decision/task/relationship extraction remains future work.
```

- [ ] **Step 2: Run focused verification**

Run: `cargo test --test service_integration -- --nocapture`
Expected: PASS

Run: `cargo test --test service_acceptance -- --nocapture`
Expected: PASS

Run: `cargo test --lib -- --nocapture`
Expected: PASS

- [ ] **Step 3: Run the extraction eval file to ensure no silent regression in extraction coverage**

Run: `cargo test --test eval_extraction -- --nocapture`
Expected: PASS

- [ ] **Step 4: Commit verification and docs**

```bash
git add docs/MEMORY_SYSTEM_SPEC.md
git commit -m "docs: record note fallback extraction coverage"
```

## Self-review checklist

- The plan covers both defects surfaced by the review: `facts=0` extraction and episode-content suppression by noisy fact fallback.
- Every proposed code change has a matching regression test.
- The plan stays within existing architecture: no new tables, no schema migration, no external dependencies.
- Tests use synthetic, non-sensitive fixtures only.

## Recommended execution order

1. Task 1 — reproduce both failures
2. Task 2 — fix extraction dead ends
3. Task 3 — fix retrieval suppression of matching episode content
4. Task 4 — improve observability
5. Task 5 — verify and document
