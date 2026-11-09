# Reproducible LongMemEval-V2 text-only evidence run

- **Date:** 2026-11-09
- **Status:** Proposed design; no benchmark result claimed
- **Baseline:** `3622ceb3` (`v1.38.0`)
- **Scope:** Existing `evals/longmemeval_v2/` adapter and the official upstream runner

## Intent

Run the official LongMemEval-V2 harness over a precisely defined text-capable subset of the Small tier using the real `ingest → extract → assemble_context` path. Publish enough provenance to reproduce answer accuracy, latency, cost and coverage. The first useful deliverable is a trustworthy baseline and failure analysis; a leaderboard claim or multimodal support is not assumed.

This is a separate specification because it depends on upstream data, reader and judge services, licensing, and a longer resource budget. The internal decision-quality evaluation is defined in the companion [memory-quality design](2026-11-09-decision-grounded-memory-quality-design.md).

## Verified baseline and source facts

| Evidence | Consequence |
| --- | --- |
| Existing [adapter README](../../../evals/longmemeval_v2/README.md) pins upstream commit `6f020ac2fc3275e46c706d3406e02c3ed79b7be2` and dataset revision `f152293e235517d504809563c833d7190b8c713b`; it explicitly limits coverage to text-capable Small. | Use these pins until a reviewed upgrade; do not call the subset “full Small.” |
| Existing [`memory_mcp_backend.py`](../../../evals/longmemeval_v2/memory_mcp_backend.py) registers a backend and calls CLI `ingest`, `extract`, `assemble-context`; `query_image` raises. `insert` assigns an in-process counter ID and current wall-clock `t_ref`. | Verify persistence/isolation and temporal fidelity against upstream trajectory semantics before interpreting dynamic-state results. |
| Existing [`run_pinned.sh`](../../../evals/longmemeval_v2/run_pinned.sh) offers smoke and integration-smoke, but no committed official-run result. | A passing smoke test is adapter wiring evidence, not benchmark evidence. |
| [Official LongMemEval-V2 README](https://github.com/xiaowu0162/LongMemEval-V2/blob/ef67f10aacd9080c75aeb2dd527a0af25dc26f1b/README.md) specifies `@register_memory`, `memory_type`, `insert(trajectory)`, `query(query, query_image=None)`, a list of text/image context items, and a private evaluator; `query` receives no question ID, gold answer or type. | Use the official runner and enforce a metadata-leakage boundary. |
| [LongMemEval-V2 paper](https://arxiv.org/abs/2605.12493) reports 451 questions, five abilities and large multimodal haystacks. The official repo measures answer accuracy and latency. | Report per-ability outcomes and coverage; paper percentages from other systems are not comparable to this text-only subset. |

## Goals, non-goals, constraints

**Goals:** Run a pinned, bounded, repeatable text-only Small-tier evaluation with the official reader/judge protocol; preserve trajectory order and the information legitimately visible at query time; publish a machine-checkable manifest and case-level outputs; quantify storage, ingest, extraction, query, reader, judge and latency costs separately.

**Non-goals:** Full Small/Medium or image understanding, a new MCP tool, direct database fact seeding, headline comparison to AgentRunbook-C/RAG paper scores, or automatic leaderboard submission. Medium is a later capacity decision after Small results. The benchmark may reveal memory formation limits; this design does not prescribe a new retrieval architecture.

Keep Python evaluation dependencies out of the Rust production profile. Do not modify upstream source or dataset in place; pin and verify them. The adapter must use public product surfaces or protocol-agnostic equivalents, not write benchmark answers or gold labels into memory. This design does **not** authorize sending dataset-derived material to an external model endpoint or incurring paid API charges. The run operator must approve the exact reader/judge destinations and a hard spend/runtime/egress budget before any pilot that uses them; local-only integration tests may proceed without that approval. The approved budget and stop behavior become part of the run profile.

## Chosen design

### 1. Freeze the evaluation protocol

Create a run profile recording: repository commit, dataset revision and content hashes; adapter and `memory_mcp` commits/build features; exact selection algorithm and selected question IDs in a sealed coverage manifest; reader model/version, prompt/config, endpoint behavior and decoding settings; judge model/version/config; hardware class; context cap; timeout/retry policy; random seeds; cost accounting and run timestamp. Keep secrets out of artifacts. A changed model, dataset, adapter, or selection algorithm is a new run series, not an in-place rerun of the old baseline.

Use the official `evaluation/harness.py` interface with `memory_type = memory_mcp`. The harness alone provides the question text and optional query image to `query`; answer, type and evaluator configuration remain inaccessible to retrieval. The adapter returns only valid non-empty text items. Ensure each context item's provenance maps back to a retrieved episode/fact and that the token cap is applied by the official harness. A local diagnostic wrapper may count calls and errors but must not alter prompts or scores.

### 2. Define a genuinely text-capable cohort

Before scoring, inspect the pinned dataset schema and create a deterministic filter for cases whose **required evidence and question** can be represented without screenshots or other image interpretation. A question lacking a `query_image` is insufficient if its answer depends on trajectory screenshots. Selection may inspect modality metadata and source trajectories; it must not inspect gold answers, model outputs or ability scores to choose easy cases. Have a small manual audit of inclusions/exclusions, then freeze the rule and case IDs. Publish total cases by domain and ability, included/excluded counts and reason codes. Stop if the dataset does not expose enough information to establish text sufficiency; do not silently relabel ambiguous cases as text-only.

Preserve every selected trajectory's available text and order. The current adapter serializes the whole trajectory object to JSON; verify that this does not drop semantically necessary text or accidentally insert benchmark-private fields. If an upstream field is a screenshot path, do not turn the path string into supposed visual evidence. Reject `query_image` cases explicitly.

### 3. Preserve temporal and identity semantics

An in-process counter and wall-clock `t_ref` are convenient for a smoke test but cannot establish historical validity for a temporal benchmark. Determine whether the pinned trajectory object contains trustworthy occurrence timestamps. Use them when present. If absent, assign a deterministic, monotonic surrogate timestamp from the official trajectory order, anchored to a fixed benchmark epoch, and label it as an ordering surrogate in the run artifact. Do not claim it is event time. Keep ingestion-time separate from valid-time, including late-arrival scenarios if upstream exposes them.

Each haystack receives an isolated namespace/storage instance or an equivalently proven reset, so source IDs cannot collide across cases or leak earlier cases into later questions. Identity is stable across retries; repeated `insert` does not create a second episode. The official runner's lifecycle (whether memory is built per question or shared) must be confirmed against the pinned code before choosing the reset scope. A failed insertion/extraction cannot be scored as a successful empty-memory query.

For each query, set the product's **valid-time `as_of`** to the end of that haystack's visible trajectory history (latest trustworthy occurrence time, or latest ordering surrogate plus one deterministic tick). This is a fixed evaluation-view time, not the wall clock and not the time asked about in the question. The **transaction view** is after all visible haystack inserts/extractions have completed in the isolated store; this benchmark does not claim to test historical transaction-time replay unless the pinned upstream lifecycle actually issues intermediate queries. Stamp benchmark `t_ingested` deterministically in insertion order where the product permits it. Any internal age/freshness calculation that still uses process time must receive a fixed evaluation clock through an existing or narrowly added eval seam; if the product cannot freeze the clock, report the run as non-reproducible and do not use it for a comparative claim. The adapter must not obtain private evaluator metadata to set either cutoff.

### 4. Run and report

Execute the full frozen text-capable Small cohort, not a hand-picked handful. Record official aggregated metrics and case-level answers, confidence/uncertainty if available, error/timeout counts, ingest/extract/query duration, reader and judge duration, tokens, cost, peak storage and memory, and dataset coverage. Distinguish product failures from reader/judge failures. An unresolved insert, extract, or query error remains in the frozen denominator as a failed case; a partial run can be published as diagnostic evidence but cannot clear the baseline acceptance gate. Report the five abilities and both domains separately where denominators permit. Keep a zero-memory or official baseline arm with the same reader/judge only if the upstream harness supports a comparable run without protocol changes.

Never compare its subset percentage to the paper's full-tier percentages as a ranking. If publishing externally, say “LongMemEval-V2 text-capable Small subset” with case manifest, protocol, model/judge, costs and exclusions. Promotion to a release claim needs a reviewed, repeatable artifact, not a single smoke run.

## Alternatives considered

| Approach | Decision | Reason |
| --- | --- | --- |
| Convert screenshots to text automatically | Defer | Changes benchmark modality and could leak or hallucinate visual evidence. Requires a separate multimodal design. |
| Seed gold facts directly into the store | Reject | Bypasses memory formation, the mechanism being evaluated. |
| Use only the existing smoke cases | Reject as result | They verify CLI wiring but cannot support accuracy claims. |
| Start with Medium or the full multimodal Small tier | Defer | Current adapter cannot handle `query_image`; first bound correctness and cost on the text cohort. |
| Compare against published 72.5%/48.5% figures | Reject | Different cohort, memory method and reader/judge conditions. |

## Acceptance evidence and sequence

1. Confirm the pinned upstream backend lifecycle and dataset fields. Save a schema/field map and coverage selection rule. Fail closed on private evaluator metadata in any `insert`/`query` input.
2. Integration tests run the adapter against the actual CLI and an isolated store. Assert stable identity on retry, no cross-case leakage, correct temporal order and fixed query-time behavior across two simulated run dates, image refusal, non-empty valid context items, and bounded subprocess lifetime.
3. Run a small upstream-harness pilot to validate reader/judge configuration and artifact shape. Label it pilot and use it only to settle resource budgets and errors.
4. Freeze the cohort and profile, run the full selected cohort, and independently verify the aggregate from case-level artifacts. Publish exclusions, failures, latency/cost and per-ability denominators. A rerun from the same pins should reproduce case IDs and deterministic ingest state; any model variance is reported.
5. Only after the baseline exists, decide whether memory-quality changes warrant a controlled before/after benchmark comparison. No code or live benchmark execution is performed by this specification.
