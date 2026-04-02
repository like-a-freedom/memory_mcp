# Extract & Assemble Context Fix — Root Cause Analysis and Implementation Plan

> **Date:** 2026-04-02
> **Evidence:** `data/eval_external/Ingest_the_following_key_emails_from_February_2026_logs.chatreplay.json`
> **Goal:** Fix the broken ingest→extract→assemble_context pipeline so that freshly ingested content is both extractable and retrievable.

---

## Root Cause Analysis

### Methodology

Conducted a code-level trace of every function in the `ingest → extract → assemble_context` pipeline, cross-referenced with 69 tool-call log entries from the chatreplay evidence file. Each hypothesis from the external review was independently verified against the source code.

### Evidence Summary from Chatreplay

| Phase | Calls | Result |
|---|---|---|
| `ingest` (first pass, 11 emails) | logs 0–10 | All succeeded, 11 episode IDs returned |
| `extract` (first pass) | logs 11–21 | **facts: [] in ALL 11 calls**. Entities: 0–3 per episode |
| `assemble_context` (3 queries) | logs 43–45 | Returns 0 or 1 irrelevant result (old Etisalat email) |
| `ingest` (second pass, reformatted) | logs 46–56 | Agent reformatted with DECISION:/TASK: markers, re-ingested |
| `extract` (second pass) | logs 57–67 | **facts: [] again in ALL 11 calls**. Entities: 0–2 per episode |
| `assemble_context` (final) | log 68 | Still returns only the old Etisalat email |

---

### RC-1: `extract_facts` is a narrow heuristic stub — NOT a general purpose extractor (P0)

**File:** `src/service/episode.rs:261-330`

`extract_facts` has exactly two code paths:

```rust
if is_metric_statement(&episode.content) {
    // create "metric" fact → matches: ARR|MRR|NRR|revenue|churn|ROI|LTV|CAC|NPS|EBITDA|$\d
}
if is_promise_statement(&normalized) {
    // create "promise" fact → matches: "i will", "i'll", "will finish/deliver/do/..."
}
```

There is **no LLM fact extraction path**. There is **no general-purpose fact extractor**. The function returns `Ok(facts)` where `facts` is empty unless the content happens to contain SaaS metrics or English-language promise verbs.

For business emails about product releases, certification timelines, documentation issues, customer requirements, and engineering decisions — **none of these patterns match**.

**Impact:** Every episode ingested without metrics/promises produces zero facts. Since `assemble_context` searches exclusively in the `fact` table, these episodes become permanently invisible to retrieval.

**External review assessment:** The review speculated about "LLM fact extraction path that's broken or not activated." This is incorrect — there is no such path. The `LlmEntityExtractor` exists for _entity_ extraction, not _fact_ extraction. `extract_facts` has always been heuristic-only.

---

### RC-2: `assemble_context` searches ONLY the `fact` table — episodes are invisible (P0)

**File:** `src/service/context.rs:70-348`, `src/storage.rs:2192-2220`

The retrieval pipeline in `assemble_context`:

1. `select_fact_records_for_query` → FTS on `fact.content` and `fact.index_keys`
2. `collect_community_facts` → facts linked to community member entities
3. `collect_semantic_facts` → ANN vector search on `fact.embedding`
4. `collect_entity_expansion_facts` → facts linked to graph-resolved entities

All four channels search the `fact` table. There is **no fallback to search episode content**. The FTS index (`fact_content_search`, `fact_index_keys_search`) is defined on the `fact` table only.

When an episode has zero extracted facts, it is a dead node in the memory graph — ingested but unretrievable.

**The one result returned** (Etisalat email) had a pre-existing "metric" fact because its content contained "$1.3М", which matched `is_metric_statement`.

---

### RC-3: Entity extraction (Anno NER) has low recall for domain content (P1)

**File:** `src/service/anno_entity_extractor.rs`

Default NER provider is `Anno` (heuristic stacked NER, `anno` crate with `default-features = false`). From the chatreplay evidence:

| Episode content summary | Entities found by Anno |
|---|---|
| EDR 8.0.1/XDR 2.0.1 SIGNED (11 people mentioned) | **0 entities** |
| KIRA user agreement (Andrey Padalko, Gigachat, KSN) | OpenAI API, OpenAI-compatible API (missed Padalko) |
| OSMP versioning (Natalya Sveshnikova + 5 others) | Natalya Sveshnikova only |
| OSMP certification (8 people, 5 products) | ERROR (tx conflict), retry found 3 people |
| Nutanix XDR (Dmitry Berezin, Elena Kondratyeva, 3 products) | Dmitry Berezin only |

Anno NER finds some multi-word person names but systematically misses:
- Product names: EDR, XDR, OSMP, KIRA, KEDR, KATA, KUMA, KSC
- Abbreviated entity names: VM, CloudXDR, CloudEDR
- Russian-text person names when preceded by titles or inline in compound sentences
- Company/partner names not in standard PER/ORG training data

The `RegexEntityExtractor` fallback pattern `[\p{Lu}][\p{Ll}]+(?:\s+[\p{Lu}][\p{Ll}]+)+` would catch multi-word capitalized names but does NOT catch acronyms (EDR, XDR). It's also not the active provider.

**Note:** Even with perfect entity extraction, facts would still be empty due to RC-1. Improved entity recall is necessary but not sufficient.

---

### RC-4 (Non-issue): Temporal filter is NOT broken

**External review claimed:** "`window_start/end` in first request gave `total_count: 0` — temporal filter either not applied to episode.t_ref, or inverted."

**Actual finding:** The agent passed valid RFC 3339 timestamps (`2026-02-01T00:00:00Z` / `2026-02-28T23:59:59Z`). `parse_datetime` correctly parses these. `apply_time_window` in `context.rs` correctly filters by `ranked.fact.t_valid >= start && ranked.fact.t_valid <= end`.

The temporal filter works correctly but filters an **empty candidate set** because no facts exist (RC-1 + RC-2). `total_count: 0` is the correct result given zero facts.

---

### RC-5 (Non-issue): GLiNER is not the active NER provider

**External review focused on:** "GLiNER SpanRepresentationLayer architecture is broken."

**Actual finding:** Default `NerProviderKind` is `Anno` (see `config.rs:60`). GLiNER is only activated when `NER_PROVIDER=local-gliner` is set. The chatreplay session used the default Anno provider. GLiNER bugs (documented in repo memories) are real but irrelevant to this specific failure.

---

### RC-6 (Minor): FTS score is hardcoded to 1.0

**File:** `src/storage.rs:2213`

```sql
SELECT *, 1.0 AS ft_score FROM fact WHERE ... AND (content @1@ $query OR index_keys @1@ $query)
```

The `ft_score` is hardcoded as `1.0` rather than using SurrealDB's actual FTS relevance score (`search::score(1)`). This means all FTS matches have equal weight, disabling quality-based ranking. The `MIN_FT_SCORE_THRESHOLD` quality gate (0.5) always passes. This doesn't cause the zero-results bug but degrades ranking when facts do exist.

---

## Failure Chain

```
ingest ✅ (episodes stored correctly)
  └─► extract_facts ❌ (only metrics/promises heuristics → facts: [] for general content)
         ├─► 0 facts in DB → assemble_context FTS returns nothing
         ├─► few entity links → community/graph expansion has little to work with
         └─► no embeddings stored → semantic ANN search returns nothing
                └─► assemble_context returns only pre-existing old Etisalat fact
```

---

## Corrections to External Review

| External review claim | Verified? | Actual finding |
|---|---|---|
| "LLM fact extraction path is broken or not activated" | ❌ Wrong | No LLM fact extraction path exists. `extract_facts` is heuristic-only. |
| "GLiNER NER architecture is broken (SpanRepresentationLayer)" | N/A | Default NER is Anno, not GLiNER. GLiNER bugs are real but not triggered here. |
| "BM25 index not updated after ingest without facts" | Misleading | BM25 indexes only `fact` table by design. Episodes were never in the FTS index. |
| "`window_start/end` temporal filter inverted" | ❌ Wrong | Filter parses and applies correctly. Empty result because 0 facts exist. |
| "Entity recall ~20%" | ✅ Confirmed | Anno finds 0–3 entities per email vs 7–15 expected |
| "`facts: []` in all episodes" | ✅ Confirmed | Verified in all 22 extract calls across both passes |

---

## Implementation Plan

### Task 1 (P0): Implement general-purpose fact extraction in `extract_facts`

**Status:** done

**Problem:** `extract_facts` only creates facts for metrics and promises. All other content types produce zero facts.

**Approach:** Add heuristic extractors for common fact types (decision, task, status, specification, requirement) using keyword-based detection patterns, similar to the existing `is_metric_statement` / `is_promise_statement` approach. Additionally, add a catch-all "observation" fact type that creates a single summary fact from episode content when no specific patterns match, ensuring every extracted episode produces at least one searchable fact.

**Files to modify:**
- `src/service/episode.rs` — Add new detection functions and extend `extract_facts`
- `src/service/episode.rs` (tests) — Add unit tests for new patterns

**Detection patterns to add:**
- `is_decision_statement`: `\b(decided|decision|approved|rejected|agreed|signed|confirmed)\b` (case-insensitive) and structured markers like `DECISION:`, `РЕШЕНИЕ:`, `РЕШЕНО:`
- `is_task_statement`: `\b(task|action item|TODO|assigned to|needs to|must|should)\b` and structured markers `TASK:`, `ЗАДАЧА:`
- `is_status_statement`: `\b(status|update|completed|done|in progress|blocked|stalled|заглохла)\b`
- `is_requirement_statement`: `\b(requirement|req)\b|\b\d{5,}\b` (numeric issue/requirement IDs)

**Fallback:** If none of the specific patterns match and the content is non-trivial (>50 chars), create a single "observation" fact with the full episode content. This ensures every episode is searchable via FTS.

**Risk:** Over-extraction (too many facts per episode). Mitigate by limiting to one fact per detected type per episode.

**Verification:** Existing tests must still pass (they test metrics + promises). New tests should cover the chatreplay email patterns.

---

### Task 2 (P0): Add episode content FTS as fallback in `assemble_context`

**Status:** done

**Problem:** When no facts exist for an episode, `assemble_context` cannot find it. Even after Task 1, there may be edge cases where fact extraction fails, and the raw episode content should still be discoverable.

**Approach:** Add a fallback retrieval channel in `assemble_context` that searches episode content directly when the fact-based channels return insufficient results. This requires:

1. **New migration** (`015_episode_content_fts.surql`): Define FTS index on `episode.content`
2. **New storage method**: `select_episodes_by_content` — FTS query against episode content
3. **Context assembly change**: When fact channels return < budget results, fill remainder from episode FTS matches, converted to synthetic `AssembledContextItem` entries

**Files to modify:**
- `src/migrations/015_episode_content_fts.surql` — New migration
- `src/storage.rs` — Add `select_episodes_by_content` to `DbClient` trait + implementations
- `src/service/context.rs` — Add episode fallback channel after fact retrieval
- `tests/` — Integration test: ingest episode (no facts), verify `assemble_context` finds it

**Schema for episode FTS:**
```sql
DEFINE ANALYZER IF NOT EXISTS memory_fts TOKENIZERS class FILTERS lowercase, ascii, snowball(english);
DEFINE INDEX IF NOT EXISTS episode_content_search ON TABLE episode COLUMNS content FULLTEXT ANALYZER memory_fts;
```

**Risk:** Episode content is raw and unstructured — may return noisy results. Mitigate by ranking episode-derived items lower than fact-derived items (source_priority 3) and limiting to `budget / 2` episode results.

---

### Task 3 (P1): Improve entity extraction recall with chained extractors

**Status:** done (implemented as low-recall regex enrichment in `extract_entities` instead of introducing a new config surface)

**Problem:** Anno NER misses most entities (product names, some person names, requirement numbers).

**Approach:** Create a `ChainedEntityExtractor` that runs Anno first, then enriches with the regex fallback extractor, deduplicating by normalized canonical name. Additionally, introduce a configurable product-name gazetteer for domain-specific entity recognition.

**Files to modify:**
- `src/service/entity_extraction.rs` — Add `ChainedEntityExtractor`
- `src/service/entity_extraction.rs` — Add product/acronym gazetteer matching
- `src/config.rs` — Add `NER_PRODUCT_NAMES` env var for product whitelist
- `src/service/entity_extraction.rs` (factory) — Wire up chaining in `create_entity_extractor`

**Chaining strategy:**
1. Run primary extractor (Anno/GLiNER)
2. Run regex extractor on same content
3. Merge results, preferring primary extractor's type classification when both find the same span
4. Run gazetteer match for known product acronyms (configurable via env)

**Default product gazetteer:** Could be empty or set per-deployment. Not hardcoded to Kaspersky product names.

---

### Task 4 (P1): Use real FTS relevance score instead of hardcoded 1.0

**Status:** done

**Problem:** `build_select_facts_filtered_query` uses `1.0 AS ft_score`, discarding SurrealDB's actual relevance ranking. This prevents quality-based filtering and sorting.

**Files to modify:**
- `src/storage.rs` — Replace `1.0 AS ft_score` with `search::score(1) AS ft_score`
- `src/storage.rs` (tests) — Update query assertion tests

**Risk:** SurrealDB's `search::score()` may return different ranges than expected. The `MIN_FT_SCORE_THRESHOLD` (0.5) may need tuning.

---

### Task 5 (P2): Add end-to-end test for chatreplay scenario

**Status:** done

**Problem:** Existing tests only use content with metrics/promises (by design). There's no test that verifies the pipeline works for general business content.

**Files to create/modify:**
- `tests/eval_extraction.rs` or new `tests/general_content_extraction.rs` — Test with email-like content that has no metrics/promises but contains decisions, tasks, and person names
- Verify: `extract` returns non-empty facts AND `assemble_context` finds the content

---

## Priority Matrix

| Task | Priority | Impact | Effort | Dependencies |
|---|---|---|---|---|
| Task 1: General fact extraction | P0 | Fixes zero-fact problem | Medium | None |
| Task 2: Episode FTS fallback | P0 | Makes episodes discoverable | Medium | New migration |
| Task 3: Chained entity extraction | P1 | Improves entity recall 2-5x | Medium | None |
| Task 4: Real FTS score | P1 | Better ranking quality | Small | None |
| Task 5: E2E test for general content | P2 | Prevents regression | Small | Task 1 |

**Recommended execution order:** Task 1 → Task 5 → Task 2 → Task 4 → Task 3

Task 1 alone would fix the primary failure. Task 2 provides defense-in-depth. Task 3 improves quality but is not blocking.
