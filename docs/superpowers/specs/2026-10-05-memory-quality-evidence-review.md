# Memory quality as the product result — critical review and landing plan

**Date:** 2026-10-05
**Status:** Proposed direction; nothing in this document is implemented
**Baseline:** `f0618fb` (`v1.26.1`)
**Inputs:** an external review of this repository proposing five capability areas
(benchmark leadership, a belief layer, typed multi-graph, typed memory classes,
semantic security, adaptive policy), each with citations to the literature.

**Purpose:** critically evaluate every point of that review against this codebase
and the cited papers, then land the surviving work on the current architecture.
This document records decisions and evidence discipline. It is not a claim that any
of it has been built.

**Revision note.** A second pass re-verified every load-bearing code fact by direct
inspection and every external claim against primary sources. Four assertions in the
first draft were wrong and are corrected in place; §8 records them. The most
significant change inverts the diagnosis: the "supersession does not close" defect
is real but is **not** a stale-value bug, and the entire claim layer is **invisible
to retrieval** — a larger and more useful finding than the one it replaced.

---

## 1. Verdict on the review as a whole

The review is **directionally right about the destination and wrong about the
route**. Its central recommendation — make memory quality the primary product result
— survives intact. Three of its specific proposals are already satisfied here. Two
are refuted by the very sources it cites. And it misses the single most important
defect in the repository, which is more fundamental than the one it names.

What survives:

- Memory quality, not control-plane surface area, should be the primary product
  result for the next cycle. Consistent with ADR-0016's freezing of the eight-tool
  surface.
- Retriever-only and end-to-end results must be separated. Already first-class here
  (§2.1) — the review asks for something this repository already has.
- A baseline ablation arm is missing. Correct, and the most urgent eval gap.
- Do not let a background LLM freely rewrite primary memory. Correct, and already
  policy (`CONTEXT.md`: never manufacture a corrective fact as a retrieval side
  effect).

What does not survive:

- The sixteen-type typed multi-graph at P1, recommended while arguing from the same
  table that graph structure is not automatically superior (§2.2).
- MEM1 as a consolidation model to imitate. It is not portable (§2.4).
- The implication that a `Belief` type, `disputed`/`uncertain` statuses and a new
  confidence field are needed. All three duplicate or revive settled decisions (§2.3).
- The implicit assumption that memory here is merely unproven rather than, in one
  specific and important respect, disconnected from the read path (§3.1).

---

## 2. Point-by-point assessment

### 2.1 Evaluation and benchmark leadership — **valid; the gap is narrower and stranger than stated**

The review's table is roughly right, with two corrections that change the shape of the
work.

| Reviewer asks for | Actual state |
|---|---|
| `Recall@5/10/20` | `recall_at_5` exists with a real formula, `hard_floor 0.90`, published `1.0`. Cutoff is configurable; other cutoffs are not gated. |
| `MRR` | Exists, `hard_floor 0.85`. Committed baselines say `0.9918`; the `0.9924` in the v3/v5 run reports is from an older 62-case suite and does not match current evidence. |
| `NDCG@10` | Absent everywhere. |
| Retriever-only vs end-to-end | Already first-class. Modes are `retrieval_only` / `end_to_end`, stamped per outcome and per suite summary. |
| LLM judge / answer quality | Absent. Zero judge-based graders. |
| BM25 / dense baseline arms | Absent. `bm25` appears only as a description of our own first retrieval tier; `evals/baselines/*.json` are prior *self*-runs. |
| LongMemEval, LoCoMo | Corpora pinned with revision, sha256, license, adapter version. |
| MemoryAgentBench, SecOps | Absent entirely. |

**Correction 1 — external results do exist, and they are weaker than "absent".** The
`pr` profile gates `external-retrieval / answer_presence_proxy_at_5` at `0.9`, and the
committed baseline artifact contains an `external-retrieval` suite with **two** cases,
both scoring `recall_at_5 = 1.0`. The dedicated `external_longmemeval.json` profile is
the one carrying `"gates": []`. So external evaluation currently runs through the PR
gate on a two-case smoke slice rather than on the pinned 500-case corpus. `target/` is
gitignored, so no run artifact is committed — every number in `docs/evals/` is a
transcription, not a machine-checkable artifact.

**Correction 2 — the saturation is worse than "a metric pinned at 1.0".**
`claim_precision` and `claim_recall` are both exactly `1.0` across 41 cases and both are
gated (floors `0.80` and `0.90`); the suite also computes `claim_f1`, which is not itself
gated. The six other metrics in the `docs/evals/CLAIM_RECONCILIATION.md` contract —
`supersession_recall`, `temporal_ambiguity_rate`, `projection_precision`, contradiction
recall as a distinct measure, projection latency, invalidated-relation count — have **no
implementing code and no metric key anywhere in the repository**. `docs/evals/CLAIM_RECONCILIATION.md`
therefore documents an evaluation that does not exist. That is a documentation-truth
defect of exactly the kind ADR-0073 was written to stop, and it is the strongest
argument for the reviewer's P0: the one subsystem whose entire purpose is conflict
resolution has no conflict-resolution metric.

The review's strongest argument, stated as a footnote, deserves the headline.
LongMemEval §5.5, under oracle retrieval: *"even with perfect retrieval, a suboptimal
reading strategy results in up to a 10-point absolute performance drop."* A retriever
scoring 1.0 tells you nothing about the system.

Cheap defect: `README.md:2141–2150` still documents `make eval-pr`, `eval-release` and
`eval-nightly`, removed from the Makefile (which now carries only `eval-response-size`
and `eval-ner-quality`, with a comment at `Makefile:26` recording the removal) and
inlined into CI (`ci.yml:105`, `evaluations.yml:34` run `memory-eval` directly). A new
contributor's first three eval commands are all broken.

### 2.2 Typed multi-graph — **not recommended; the reviewer's own citation refutes it**

Verified: `Edge.relation` is a free-text `String`, `triple.predicate` is a free-text
`String`, `MemoryService::relate` accepts an unvalidated `&str`. Zero of the sixteen
proposed types exist.

What the review cites as caution is the stronger argument against its own recommendation:

- **Mem0's own table** (`arxiv 2504.19413`, Table 1): graph memory *lowers* single-hop F1
  (38.72 → 38.09) and multi-hop F1 (28.64 → 24.32), helping only temporal and
  open-domain. Overall J rises 66.88 → 68.44 for roughly double the token cost
  (1764 → 3616 tokens per conversation).
- **MemoryAgentBench** Table 3, overall: BM25 41.5, HippoRAG-v2 41.6, MemoRAG 30.9,
  Contriever 29.8, RAPTOR 27.0, Zep 24.0, GraphRAG 23.4, Mem0 21.1. Every
  structure-augmented system except HippoRAG-v2 loses to plain BM25.
- **Cost here specifically**: converting `Edge.relation` to an enum is a wire-format
  break, a public API break on `relate`, and touches hardcoded SQL
  (`episode_context_store.rs:68` embeds `relation = 'involved_in'`), the extraction
  literals in `fact_extraction.rs`, edge versioning in `episode/edges.rs`, and the triple
  predicate vocabulary. Roughly ten files and two frozen contracts.

**MAGMA, read in full, is weaker support than the review implies.** The review credits
MAGMA with showing that intent-conditioned relational views reduce
semantically-similar distractors. MAGMA's Table 4 ablation does report that removing the
traversal policy costs the most (judge 0.700 → 0.637), which is genuine support for
*routing*. But three facts cut against adopting its representation:

1. Its LoCoMo overall advantage (0.700) comes overwhelmingly from one column:
   **adversarial 0.742**, against A-MEM 0.616 and full-context 0.205. Excluding that
   column MAGMA is *last* of five on single-hop (0.528, below A-MEM 0.495, MemoryOS 0.674,
   Nemori 0.764, full-context 0.630) and mid-table on multi-hop (0.528, below Nemori 0.569
   and MemoryOS 0.552). Its temporal result, the one the multi-graph design targets, is a
   0.001 margin over Nemori (0.650 vs 0.649).
2. MAGMA's default embedding model is **all-MiniLM-L6-v2** — a 22M-parameter distilled
   encoder — while this repository runs a local bi-encoder stack with Metal/Accelerate
   execution. Its absolute retrieval numbers are not comparable to ours.
3. Its own Limitations section concedes that graph quality *"depends on the reasoning
   fidelity of the underlying LLM used during asynchronous consolidation"*, that erroneous
   relations *"may still arise and propagate to downstream retrieval"*, and that the
   multi-graph substrate *"may introduce additional storage and engineering complexity"*.
   That is a precise description of the semantic-drift risk this repository has already
   ruled out by ADR-0016 and `CONTEXT.md`.

So: take the routing, which MAGMA's own ablation isolates as the single largest
contributor; skip the vocabulary, which its own limitations section flags as the fragile
part. That is Phase 3, and it is cheap.

### 2.3 Belief layer — **the concern is real, the proposed shape is wrong, and all of it is already decided here**

No `Belief` type, no confidence on `Claim`, no `supported_by`; grep for `disputed`,
`uncertain`, `conflict_set` returns nothing. On the surface the review is right.

But four specifics are settled by accepted ADRs, and re-deciding them would be a
regression:

1. **A typed epistemic relation axis already exists.** `ClaimRelationOutcome` is
   `{ Duplicate, Supersession, Correction, Contradiction, TemporalAmbiguity }`,
   versioned append-only per ADR-0010, covering `SUPPORTS`/`CONTRADICTS`/`SUPERSEDES`
   plus two more. A parallel `Belief.supports`/`contradicts` vocabulary duplicates it.
   `GLOSSARY.md` states its terms win on conflict.
2. **No confidence may be invented.** The review asks for "calibrated probability, not
   arbitrary score". The only calibrated machinery here is the Beta posterior in
   `procedure.rs` (`beta_posterior_mean`). `Fact.confidence` is not calibrated at all — it
   is a literal `0.9` / `0.85` written at the two extraction sites in
   `fact_extraction.rs:391,417`, multiplied by a two-valued half-life decay. Any new
   confidence field must be traceable to an observed outcome or it is precisely the
   arbitrary score the review warns against.
3. **`disputed`/`uncertain` has no substrate.** The corresponding reason code is dead:
   `_TemporalAmbiguity` is underscore-prefixed in `reconcile.rs:58`, `ValidityRelation`
   has no `Unknown` arm, and disjoint validity now yields `Coexist`. The state was
   deliberately retired. Reviving it requires an ADR arguing the retirement was wrong.
4. **A whole belief layer would be inert.** See §3.1 — the entire claim/relation layer is
   currently unreachable from `assemble_context`. Adding a materialized interpretation on
   top of a substrate the read path cannot see produces a more sophisticated unused
   structure.

The genuinely valid part of §2.3 is the warning that **invalidation is not conflict
resolution**. That is true here and is Phase 1 — but the mechanism differs from the first
draft's, and is more serious.

### 2.4 Consolidation and adaptive policy — **correct as diagnosis, wrong as remedy**

The diagnosis holds: no worker derives higher-level knowledge from repeated episodes.
Every background worker maintains an index (communities, claims, embeddings), marks
invalid (decay), or archives. There is no `rederive`, no replay of episodes into a
derived layer, no rollback of a derived artifact. The only rebuildable derived state is
communities, via `run_community_rebuild_pass` — the right template if this is ever built.

Three corrections to the proposed remedies:

- **MEM1 is not portable.** Verified against `arxiv 2506.15841`: the 3.5× quality / 3.7×
  memory figures are MEM1-7B — RL-trained from Qwen2.5-7B *Base* on 4×H100/H200 with a
  masked-trajectory PPO objective and verifiable per-answer rewards — versus
  Qwen2.5-14B-Instruct on a 16-objective composite QA task. The mechanism is that the
  model *rewrites its own prompt* each turn. An external memory substrate has no such
  control; the agent owns its context window. The review half-concedes this; in fact it
  cannot be ported at all without model-level control. The paper also reports RL-trained
  MEM1 collapsing relative to SFT past six objectives (0.088 vs 1.630), a further reason
  not to treat it as a general consolidation method.
- **A-MEM's numbers argue against LLM-driven memory evolution, not for it.** In Mem0's
  re-run A-MEM scores 48.38 overall J against Mem0's 66.88. Letting an LLM rewrite memory
  is measurably worse, not merely risky.
- **LongMemEval argues against summary-based consolidation.** §5.2: replacing sessions or
  rounds with extracted summaries or facts *"negatively impacts QA performance due to
  information loss"*, with one exception — multi-session reasoning. MAGMA's efficiency
  table makes the same point from the other direction: A-MEM has the lowest token cost
  (2.62k) and the *second-worst* overall judge score (0.580), because summarization
  discarded the "violin" detail. The review's consolidation proposal creates summaries.

On adaptive policy: collect signals before learning — correct, but the signal list cannot
be sourced from what exists. `access_count` counts *retrievals*, incremented before the
model has used anything, so it cannot distinguish a used memory from a wasted one. Worse,
it returns to ranking as a **novelty penalty** (`novelty_factor`, `ranking.rs:486–488`),
systematically demoting memories that were correctly and repeatedly needed. The inversion
must be fixed before signals can be collected, let alone learned from.

### 2.5 Typed memory classes and type-specific retention — **valid in principle, wrong as a taxonomy**

`FactType` is `{ Note, Decision, Metric, Promise, Experience }`. Type-specific policy is
one ranking boost (`Experience × 1.12`) and a binary half-life split: `Metric | Promise |
Decision` → 365 days, everything else → 180 (`domain.rs:125–137`). The review's complaint
is correct, and the current split makes it worse: `Experience` — the type carrying
preferences and first-person operational context — gets the *shorter* half-life.

But the six-class taxonomy is a poor fit. Episodic is already a table (`Episode`), not a
tag. Semantic is already `triple`, not a tag. Procedural is already a separately gated
context projected through `FactType::Experience` by ADR-0016. Identity/preference has no
representation, and inventing a class without a write gate produces exactly the
never-decays failure the table warns about. Working memory is not a persisted class; the
context cache is a cache.

A second classification axis parallel to `FactType` is what the
`no_duplicate_implementations` guard exists to catch. The valid, smaller move is **to
make the existing type-specific policy real and correct** — per-type retention expressed in
the owning context, no new axis.

### 2.6 Semantic security — **valid, mostly already present, misdiagnosed as absent**

The review believes frontier risk is inside memory and proposes `origin`, `trust_level`,
quarantine, data/instruction separation, a poisoning suite. Verified state:

- `TrustClass { AgentInference, LifecycleEvidence, OperatorApproved, LegacyUnknown,
  UntrustedExternal }`, `InvocationOrigin`, `CaptureDisposition` including `Quarantined`,
  and `TrustPolicy::may_derive` — an exhaustive non-ordered relation with the doc note
  "do not derive a total ordering for trust".
- ADR-0016 AD-3: trust derives from the invocation channel and is *never*
  caller-controlled. ADR-0038 preserves origin, trust and quarantine while removing
  routing fields.
- A gated `lifecycle-poisoning` suite with `poisoning_pass_rate`, asserting the
  memory-is-data envelope and non-elevation of trust.
- `CaptureReasonCode::AcceptedCorrection` — correction capture already exists.

So origin, trust, quarantine, non-elevation and a poisoning gate all exist. The real gaps
are narrower:

1. **Trust is a capture-time gate, not a persisted attribute.** Checked in
   `agent_memory/policy.rs`, then discarded. Nothing carries `TrustClass` onto a `Claim` or
   an `Edge`, so a derived artifact cannot inherit a trust floor it was never given.
2. **`Provenance.source_confidence: Option<f64>` conflates source trust with extraction
   confidence**, and exists only on `Fact` and `Edge` — not on `Claim` or `Episode`. The
   review's "source_trust separate from extraction confidence" is a genuine, small,
   well-scoped gap.
3. **No propagation rule**, so "derived trust ≤ min(trust of bases)" is unstated and
   unenforced.

Real work, and the security item worth doing — but an extension of an existing model, not
an introduction.

---

## 3. What the review missed entirely

### 3.1 The claim layer is invisible to retrieval — the largest defect in the repository

This is the finding that changes the plan, and it is stronger than anything the review
proposed. Verified exhaustively rather than inferred:

- `assemble_context` reads `fact` and `edge`. Grepping `memory/retrieval/`,
  `memory/retrieval.rs` and `memory/api.rs` for `claim` returns **no read path** — only
  `close_claims_for_fact` and `claim_pipeline_is_wired` on the invalidation port.
- `ClaimReconciliationMetadata` on `AssembledContextItem` — the field meant to tell a
  reader that an item was superseded, contradicted or duplicated — is
  `reconciliation: None` at **every** construction site: `retrieval/budget.rs:154`,
  `retrieval/scoring.rs:67`, `retrieval/views.rs:101`, `retrieval/logging.rs:275`,
  `agent_memory/recall.rs:474`. There is no `reconciliation: Some(..)` anywhere in the
  codebase.
- `select_claims_for_facts` and `select_relations_for_facts` have exactly three production
  callers: the claim worker, and two sites in `fact_extraction.rs` that only build a
  `ContradictionWarning` at extract time.

Consequence: the entire bi-temporal claim pipeline — the project's most substantial
intellectual property, 41 evaluated cases, a schema with relation versioning — has **no
effect on what a reader is ever shown**. Reconciliation is write-only. A supersession is
detected, persisted, scored at 1.0 by the gate, and then never consulted.

This reframes the review's headline bug. It is *not* "the predecessor's validity interval
stays open, so a stale value is returned." `valid_to` **is** never written — every
`ClaimDraft` sets `valid_to: None` — so the `(None, None)` arm of `validity_relation`
always fires and `Disjoint` is structurally unreachable. But that is a *precision* defect in
reconciliation classification, not the mechanism of a stale read. The stale read happens one
layer up, because no read path consults reconciliation at all. Fixing `valid_to` alone would
change no user-visible behaviour.

It also invalidates the passing acceptance test as evidence. `longmem_acceptance.rs:134` is
named `assemble_context_when_newer_fact_supersedes_older_one_then_latest_view_prefers_active_fact`
— but its body calls `InvalidateCapability::invalidate_from_service`. That is **retraction**,
ADR-0009's deliberate opposite of supersession. The test proves invalidate-then-prefer-active;
it does not exercise supersession, and cannot, because nothing does.

And it explains the gate. `claim_f1 = 1.0` measures whether persisted relations match expected
*labels*, read back through the feature-gated `eval_support` seam. It never measures whether
those relations change retrieval. Perfect scores on a write-only subsystem are exactly what
a saturated regression suite looks like.

Ordering consequence: the read path must be connected **before** conflict resolution is made
more precise, and before a `Belief` layer is contemplated. Otherwise the plan optimizes a
component nobody reads.

### 3.2 Supersession records a relation but never closes the predecessor

`ADR-0009` requires that confirmed supersession close only the earlier claim's real-world
validity interval, and ADR-0015 that correction and supersession remain distinct in which
interval they close. The `claim` table has `valid_to option<datetime>`; the close protocol
has a single owner, `CloseStoreClient` (ADR-0039); and neither is used. `close_claims_for_fact`
sets only `t_invalid_ingested` on every claim of a fact — whole-fact cascade, which is
retraction semantics, and is exactly what ADR-0009 forbids as an implementation of automatic
supersession.

Two latent non-conformities in the same area. `supersedes_relation_id` is declared in
ADR-0010 for relation versioning and is always `None` in production (`worker.rs:300`), so
relation history cannot be walked. And `pair_fingerprint` is a raw `"{left}:{right}"`
concatenation (`worker.rs:288`) rather than a deterministic fingerprint — though the schema
*does* carry a separate `context_fingerprint` column which is properly populated from
`draft.context_fingerprint` (`worker.rs:276,298`). So the ADR-0013 determinism requirement is
arguably already met by that column, and the correct fix is to populate or drop
`pair_fingerprint` rather than invent a hash.

### 3.3 Procedural memory is fully built and entirely unwired

`migration 028_procedural_memory.surql`, `models/procedure.rs` with a Beta posterior,
`procedure_store.rs`, `procedures_service/{ranking,review}.rs`,
`docs/evals/PROCEDURAL_MEMORY.md`, and `tests/procedural_memory_e2e.rs` all exist.
`create_candidate`, `rank_candidates` and `review_candidate` have **zero production
callers** — grep returns only their definitions and unit tests.

Simultaneously the largest dead end in the codebase and the cheapest delivery of a
reviewer-requested capability. Procedural memory already *is* "outcome-validated, retained by
success rate" — the exact row of the review's memory-types table — implemented, tested and
documented, awaiting a capture hook and a retrieval tier. Building a consolidator to
approximate it would be strictly worse engineering.

### 3.4 The exposure-trace hook exists and is hardcoded empty

ADR-0016 designed `ExposureTrace` and `LifecycleTraceLink` for exactly the outcome signal the
review asks for, and AD-7 draws the line honestly: exposure proves exposure, not causal use.
`MemoryEventRecord` already carries `trace_retrieval_fingerprint` and
`trace_selected_fact_ids`. And `capture.rs:341–342` writes `None` and `Vec::new()` —
unconditionally.

The reviewer's "start by collecting trainable signals" step, already authorized by an accepted
ADR, requiring no schema change and no new table. One field write starts accumulating real data.

### 3.5 The honest positioning is different from the review's

Three results from the cited literature constrain any public claim:

- **LongMemEval** Table 2: full-context scores 72.90 overall J; Mem0 scores 66.88. Full
  context wins on answer quality and loses only on latency and tokens.
- **MemoryAgentBench** §4.2: long-context models win test-time learning and long-range
  understanding outright; memory systems win only accurate retrieval.
- **MemoryAgentBench** §4.2, selective forgetting: *"all methods fail on the multi-hop
  situation (with achieving at most 28% accuracy)"*; the best long-context agent reaches 53%
  on single-hop.

A memory system cannot honestly claim to be *better*. It can claim to be **cheaper at
comparable quality**, plus properties no benchmark column measures: bi-temporal correctness,
evidence preservation, invalidate-never-delete, and local-first operation with no required API
key or model. And the hardest unsolved problem in the field — selective forgetting — is
precisely the one this codebase's architecture is built for.

The defensible positioning is therefore not "benchmark-proven memory kernel". It is **the
memory substrate whose updates are provably correct, evaluated on the one axis nobody has
solved, in the one deployment mode nobody else supports.** The review's instinct —
benchmark-first — is right. Its proposed benchmark framing is the one thing that cannot be
delivered honestly.

---

## 4. Landing plan

Sequenced by evidence value per unit of blast radius. The ordering constraint is strict and
load-bearing: **reconciliation must be connected to the read path before conflict resolution
is made more precise, and both before anything is measured externally.** Measuring a
subsystem that cannot affect the answer produces a number that cannot move.

### Phase 0 — Connect reconciliation to the read path (production change, prerequisite for everything)

**Goal:** make the project's largest subsystem observable before making it clever.

1. Populate `ClaimReconciliationMetadata` on the assembled item — **at the single point after
   the view-mode dispatch**, not at the item's construction sites. `AssembledContextItem` is
   built in production at twelve sites across `views.rs`, `scoring.rs` and `experience.rs`
   (`budget.rs`'s and `rescue.rs`'s constructors are all inside `#[cfg(test)]` modules), and
   patching each `build_*_view` arm means touching twelve places and missing one silently. The
   lookup itself belongs to the **knowledge** context — the declared owner of claims and
   relations under ADR-0058 — exposed as a narrow named method on a `knowledge` store, per
   ADR-0044. `memory` may consume it, but `memory/retrieval` must not own claim SQL, and
   `src/service/` must not gain business logic (ADR-0058, ADR-0066). The existing
   `ContextRetrievalPort` remains the *seam* being decorated, not the owner of the query.
   Surface the field under ADR-0022 compact-mode `skip_serializing_if` gating, or the
   response-size gate regresses.
2. Surface supersession outcome in the read view: when a fact's claim is superseded, the
   assembled item carries the successor's identifier and reason code, so a reader is never
   handed a superseded value without being able to see that it was. Direction is read from
   `claim_relation.predecessor_claim_id`/`successor_claim_id`, which already records it once —
   never recomputed by the reader.
3. Rank a superseded fact **strictly below** its successor when both are present in the pack;
   when no successor is present, do not demote it. `Duplicate` is never demoted: a duplicate is
   redundancy, not staleness, and demoting it can remove the only surviving copy once decay
   retires one of the pair. This is a post-assembly reordering, not a scoring term — successor
   presence is unknown until candidates are selected — so it adds no constant to tune and no
   second candidate axis. [ADR-0074](../../adr/0074-connect-the-reconciliation-relation-to-the-read-path.md)
4. Remove `ClaimRelationSummary.counterpart_fact_id` in a **separate commit ahead of this
   phase**. It is never populated (its only occurrence in the tree is its own declaration), and
   `claim_relation` already stores direction, so keeping it duplicates the truth. Removing a
   field that never carried data is not a behavior change, and isolating it keeps Phase 0's
   diff attributable if anything regresses.
5. Extend the acceptance surface with the case that has no test today: a genuine correction,
   distinguished from retraction, asserting the latest view returns the corrected value. If the
   ADR elects to project relations into `explain`, assert it there too; otherwise assert the
   relation is reachable through the same named knowledge method the read view uses, so the
   evidence does not depend on a second public surface.
6. **Gate disclosure on the claim rollout stage.** `docs/evals/CLAIM_RECONCILIATION.md` states
   that the default `shadow` stage "projects claims but does not expose relations in
   `assemble_context`", and ties promotion to `evidence` to precision/recall thresholds — while
   ADR-0074's Decision says an item *must* expose its relations. Both hold if the read path
   serves relations only at `evidence`. The gate belongs in the `knowledge`-owned adapter behind
   `RelationReadPort`, never in `memory/retrieval`, so no deployment can be talked into
   disclosure from the retrieval layer; and because `demote_superseded` reads the rows the gate
   withholds, one gate covers the metadata *and* the reordering. `MemoryService::
   with_claim_rollout_stage` is the seam, mirroring `MEMORY_CLAIM_ROLLOUT_STAGE` for callers that
   build a container directly. The promotion thresholds remain the operator's to verify — this
   phase changes where relations surface, not when an operator may turn them on.

**Evidence.** What was required, and what was actually obtained, on 2026-10-05:

- *A correction scenario observed failing before the fix* — obtained.
  `corrected_fact_supersedes_the_stale_value_in_the_latest_view` replaces the
  retraction test that used to sit under this name; disabling `demote_superseded` fails its
  first assertion with `left: "ARR is legacy"`.
- *The ranking rule asserted in both halves* — obtained. Six unit tests cover the policy on
  constructed items with no database; the successor-present and successor-absent halves are
  separate cases, and the successor-absent one asserts the predecessor both keeps its rank and
  survives the budget.
- *The retrieval and response-size gates still green* — obtained: 117/117 cases, 9/9 gates,
  `RESULT: PASSED` against the committed baseline.
- *A Phase 2 external run whose knowledge-update number is no longer structurally incapable of
  moving* — **not obtained, and not obtainable yet.** Two blockers, reported rather than
  explained away. First, `eval-harness` built its service without a claim rollout stage, so
  every run measured the feature's absence at `shadow`; it now runs at `evidence`. Second, and
  structural: **the number does not exist.** No key matching knowledge / update / temporal /
  supersession appears anywhere in the artifact, because `CLAIM_RECONCILIATION.md` lists seven
  metrics and only `claim_precision`/`recall`/`f1` are implemented — `supersession_recall`,
  `temporal_ambiguity_rate` and `projection_precision` have neither code nor key. The retrieval
  suites also seed facts without the lineage that produces relations, so nothing there is
  positioned to move.

The honest statement of Phase 0 today: the production code is complete and mutation-verified
in-process — a relation demonstrably changes which fact leads a pack. The external evaluation
evidence this phase promised depends on a metric Phase 2 adds, and Phase 0 is therefore
**partially** evidenced, not complete. Saying otherwise would repeat the mistake this document
exists to correct.

**ADR:** one — [ADR-0074](../../adr/0074-connect-the-reconciliation-relation-to-the-read-path.md).
This changes what `assemble_context` returns and is therefore visible on a frozen surface;
ADR-0022 re-froze the eight tools and the compact default, so this needs an explicit record
rather than being treated as an implementation detail.

**Explicitly not in this phase:** writing `valid_to`. Connecting a write-only relation to the
read path does not require the write to be more precise, and doing both at once makes any
failure unattributable.

### Phase 1 — Correct conflict resolution (production change)

**Goal:** make the relation that Phase 0 exposes actually resolve.

1. Add the per-claim validity close ADR-0009 mandates, as a new named intent on
   `CloseStoreClient` — the single owner of the close protocol per ADR-0039. Never compose
   close SQL elsewhere. **No migration: `valid_to` already exists.**
2. Populate `supersedes_relation_id` so relation history is walkable (ADR-0010).
3. Resolve `pair_fingerprint`: populate it deterministically, or drop it in favour of the
   already-correct `context_fingerprint`. Do not add a second fingerprint.
4. Replace the evidence sniff in `reconcile.rs` — currently qualifier string keys
   (`"correction"`, `"transition"`, `"supersedes"`, `"replaces"`) — with typed relation
   evidence, so supersession stops depending on a caller spelling a word in a qualifier map.
5. Restore the six unimplemented metrics from the `CLAIM_RECONCILIATION.md` contract, or amend
   the contract to describe what is actually measured. Report supersession recall and
   temporal-ambiguity rate as distinct gated metrics rather than folded into one `claim_f1`.

**Evidence:** a supersession scenario where the predecessor's `valid_to` closes and the fact
stays valid per ADR-0002; a `validity_relation` unit test covering all four arms, `Disjoint`
currently being unreachable; and the six contract metrics either implemented or the contract
corrected.

**ADR:** one, implementing ADR-0009's existing requirement. No new table, no migration.

**Explicitly deferred:** `Belief` as a type, `disputed`/`uncertain` statuses, any new
confidence field. §3.1 makes these premature: they would be layered on a subsystem the read
path only just started to consult.

### Phase 2 — Make the comparison real (no production change)

**Goal:** the first honest external number, and metrics that can actually fail.

1. Add baseline retrieval arms to `eval-harness`: lexical-only (BM25 over the existing index)
   and dense-only (embedding ANN). Two corrections to the obvious approach, both verified:
   `ContextRetrievalPort` is `pub` but the harness **never names it** — it drives the product
   through `AssembleContextCapability::assemble_context_from_service` over a real
   `MemoryService`, so an arm implemented as a trait impl would be unreachable from the harness.
   And ADR-0040 fixes `RetrievalContext` as a concrete crate-private struct over an enumerated
   infrastructure set, explicitly rejecting unrelated new fields.

   `AssembleContextRequest` is also the wrong place: it derives `JsonSchema` and is the MCP tool
   schema (`tools/context.rs`, `tools/assemble_context.rs:37`), so a new field would alter the
   frozen eight-tool surface. It already demonstrates the correct pattern — `access` carries
   `#[serde(skip_serializing)]` + `#[schemars(skip)]`. Follow that exactly: the arm selector is
   schema-skipped and never reachable from a tool call, thread it through the shim to the
   retrieval operation, and each arm reuses the existing reducers, gates and metric names
   unchanged so the comparison is like-for-like rather than a parallel metric path.
2. Add NDCG to `metrics.rs` alongside `recall_at_k`/`mrr`, in the single formula home
   ADR-0025 requires.
3. Replace the two-case `external-retrieval` smoke slice in the PR gate with the pinned
   500-case `longmemeval-cleaned` corpus, or gate the smoke slice explicitly as a smoke test
   and stop reporting it as external evidence. Commit at least one real artifact under
   `evals/results/`; `target/` is gitignored, so today nothing is machine-checkable.
4. Relabel the saturated local metrics. Either make the fixture set adversarial enough that
   `recall_at_5` and `claim_f1` can fall below 1.0, or rename them to what they are —
   regression gates — and stop quoting them as quality figures.
5. Fix the README `make eval-*` drift.

**Evidence:** a committed artifact comparing this pipeline against BM25-only and dense-only on
the same corpus and reader, retriever-only and end-to-end reported separately, with the report
stating its label-trust class per ADR-0020 and what it may not claim.

**ADR:** one, amending ADR-0019's evaluation policy with baseline arms and NDCG.

**Non-goal:** no new suite framework, no new artifact schema version, no LLM judge in this
phase.

### Phase 3 — Intent-routed retrieval, measured before adopted

**Goal:** test whether fixed fusion leaves quality on the table, per MAGMA's own ablation.

1. Add a pure `intent` detector in `memory/retrieval/`, alongside the existing `query_mode.rs`.
   No I/O, no new ports, no reordering. `resolve_view_mode`'s explicit-over-auto precedence is
   pinned by a test and must be honoured.
2. Emit intent as a label in `query_log` and in profile artifacts **before** letting it
   influence selection.
3. Only then, and only where Phase 2 shows a measured per-intent deficit, let intent adjust tier
   weights. Keep `CacheKey` unchanged while routing stays a pure function of the query; extend it
   before any routing that depends on mutable state.

**Evidence:** per-intent retrieval metrics from the Phase 2 arms, routed versus fixed fusion on
the same corpus, with existing retrieval gates green and response-size at ≥30% reduction.

**ADR:** one — [ADR-0076](../../adr/0076-route-retrieval-by-intent-and-type-memory-by-retention-class.md),
which covers both this phase's intent routing and Phase 4's retention classes, and records
that the intent is a schema-skipped field (never reachable from a tool call) and does not
reuse or replace `view_mode`. It also records that intent selects a traversal plan, not a type
predicate — `fact_types` already exists as the caller-supplied type filter. If routing is
measured and rejected, ADR-0076 is superseded for that half; the retention half stands.

### Phase 4 — Retrieval and retention made type-aware, in place

**Goal:** the valid half of §2.5, without a second classification axis.

1. Make retention type-aware: move half-life into the owning context as pure functions over the
   existing `FactType`, per ADR-0066. Retention is **not** currently per-type at all —
   `LifecyclePolicy.decay_half_life_days` is one scalar (`365.0`) applied to every fact in
   `run_decay_pass`, so this introduces the table rather than correcting one. Independent of every
   other phase — land it first within this phase.
2. Give per-type write gates at the existing extraction seam, where type selection already
   happens (`summary_parser.rs:209`, `classify_structured_summary_fact_type`) — not as a
   parallel gate.
3. **Deferred to Phase 5**, not done here: replacing the `access_count` novelty penalty with a
   use-based signal. It needs trace data that Phase 5 produces, so attempting it in Phase 4
   would either block on a later phase or ship a guessed substitute for "used". Until then the
   honest position is that `access_count` measures *retrievals*, not usefulness, and the plan
   says so rather than pretending a proxy exists.

**Evidence:** a measured change in which facts survive retrieval-heavy and retrieval-light
workloads, reported per type, with decay- and archival-worker behaviour tests.

### Phase 5 — Outcome signals, then the unwired procedural pipeline

**Goal:** accumulate the raw material, then switch on the feature that already exists.

1. Populate `trace_retrieval_fingerprint` and `trace_selected_fact_ids` in `capture.rs`.
   Authorized by ADR-0016; no schema change, no new ADR needed.
2. Join `query_log` ↔ `explain` citations ↔ capture events into a durable interaction trace, with
   an explicit retention decision — the 30-minute `TRACE_TTL_SECS` is deliberately short and the
   durable shape needs its own record.
3. Wire the procedural pipeline: a capture hook calling `ProcedureStore::create_candidate`, an
   operator path to `review_candidate`, and a retrieval tier surfacing promoted candidates
   through the existing `FactType::Experience` seam per ADR-0016.
4. With item 2's trace in hand, complete the item Phase 4 deferred: replace the
   `access_count` novelty penalty (`ranking.rs:486–488`) with a use-based signal. This is the
   only point at which such a signal can be built from observed use rather than a proxy, and it
   is the reason the work sits here and not in Phase 4.

**Evidence:** an end-to-end procedure lifecycle — candidate created from a real outcome, promoted
by an operator, retrieved and cited, with the Beta posterior responding to observed success and
failure. `tests/procedural_memory_e2e.rs` is the shape; it must stop being the only path.

**Deferred:** learned routing, reranking, write gating. Nothing is learned until there is a
quality outcome signal and a correction log — the review's own sequencing, which is correct.

### Phase 6 — Trust propagation onto knowledge

**Goal:** close the three narrow gaps from §2.6.

1. Carry `TrustClass` from capture onto the derived `Claim` and `Edge`. **Reuse the existing
   enum** rather than introducing a domain-specific trust vocabulary: two vocabularies would
   make the propagation rule ("derived trust is the minimum over its bases") expressible only by
   two policies that can drift. On a claim or edge, `TrustClass` means trust in the source that
   produced the record — not trust in a memory event, and not extraction confidence.
   [ADR-0075](../../adr/0075-carry-the-existing-trustclass-on-claims-and-edges.md)
2. Separate `source_trust` from extraction confidence on the provenance record, where it is
   currently conflated in `Provenance.source_confidence`.
3. State and enforce the propagation rule: derived trust is the minimum over bases, never
   greater, and never elevated by summarization. `TrustPolicy::may_derive` already encodes the
   exhaustive relation and the rule that external content can never promote itself; this phase
   extends it rather than adding a parallel check.

**Evidence:** a poisoning scenario proving a low-trust source cannot produce a high-trust
derived artifact through any consolidation path, extending the existing `poisoning_pass_rate`
gate (`evals/profiles/release.json:88`, currently `1.0`). **ADR:** one —
[ADR-0075](../../adr/0075-carry-the-existing-trustclass-on-claims-and-edges.md), which *narrows*
ADR-0016 rather than extending it: AD-3 there governs trust **authority** (derived from the
invocation channel, never from tool arguments), a different question from where a derived
record's trust comes from. Neither `TrustClass` nor `may_derive` was recorded in any ADR before
this one.

**Migration:** this is the one phase that requires one. `claim` is declared
`DEFINE TABLE claim SCHEMAFULL` (migration 029), so a new field is rejected unless the schema
declares it; `edge` is `TYPE RELATION` and likewise needs the field defined. Adding
`053_memory_trust.surql` to the base registry therefore trips
`latest_registered_migration_is_expected`, which must be updated in the same change. `Edge`
carries no `namespace` field, so ADR-0038 is not engaged — the trust class is an attribute of
the record, not a storage-context selector.

### Explicitly not doing

- **The sixteen-type edge vocabulary.** Refuted by Mem0's own table and MemoryAgentBench's
  rankings; a wire and API break for no measured gain. MAGMA's own limitations section names
  LLM-inferred graph quality as its principal fragility.
- **A `Belief` type, `disputed`/`uncertain` statuses, or a new confidence field.** Duplicates
  `ClaimRelationOutcome`, revives a deliberately retired state, and would be layered on a
  subsystem the read path cannot currently see.
- **A consolidation worker.** Contradicted by LongMemEval §5.2 and MAGMA's own efficiency table
  on summaries; Phase 5 delivers more reviewer-requested capability from code that already
  exists.
- **Anything modelled on MEM1.** Requires control of the agent's context window.
- **RL or learned retrieval policy.** No outcome signal, no correction log.
- **New NER backends, control-plane or UI surface.** Correctly excluded by the review;
  entity-resolution error is still unmeasured on our target corpora.
- **Any new MCP tool.** Eight-tool surface frozen by ADR-0016, ADR-0022, ADR-0047, ADR-0052;
  `public_surface_snapshot` must stay green throughout.

---

## 5. Constraints this plan inherits

| Constraint | Source |
|---|---|
| Eight-tool MCP surface frozen; new surface needs a separate ADR and evidence gate | ADR-0016, ADR-0022, ADR-0047, ADR-0052 |
| Compact responses are the default; new response fields must be `skip_serializing_if`-gated or the ≥30% byte-reduction gate regresses | ADR-0022 |
| Business policy in the owning context's `api.rs` as pure functions over caller state | ADR-0066 |
| One owner for bi-temporal close; new code expresses intent, never composes close SQL | ADR-0039 |
| Stores expose named methods only; no generic CRUD forwarding | ADR-0044 |
| New long-lived workers must register a cancellation token and join handle; spawning happens in `bootstrap/` | ADR-0046, ADR-0067 |
| Deterministic identity for new semantic artifacts; immutable payloads; single monotonic close | ADR-0013 |
| Contradiction never invalidates a fact; supersession, correction, retraction, erasure stay separate operations | ADR-0002, ADR-0009, ADR-0015 |
| Automatic supersession requires same-lineage continuity or scoped authority; a value difference, recency or higher confidence alone may never authorize correction | ADR-0008, ADR-0015 |
| One formula home for eval metrics; batch metrics live in artifacts, not Prometheus | ADR-0025, ADR-0048 |
| Weak-label corpora may not gate a release; a corpus benchmark is not release evidence until pinned data is prepared and an artifact exists | ADR-0020, `docs/evals/README.md` |
| Label trust is a closed set (`Official` / `Reviewed` / `Weak`); LongMemEval-cleaned and LoCoMo are pinned with revision, sha256, license and case count, so their class must be stated per artifact, not assumed | ADR-0020, `evals/corpora/*.json` |
| Test behavior, not document inventory; directory presence and hand-mirrored status tables are not evidence | ADR-0073 |
| Migrations are append-only | ADR-0011, ADR-0012 |
| Historical ADRs 0001–0057 and prior specs are not rewritten | ADR-0058 |

Migrations are append-only, and `latest_registered_migration_is_expected`
(`knowledge/claims.rs:792`) asserts the last entry of `versioned_migrations()` is
`039_filesystem_ingestion.surql`. Files `040`–`052` exist on disk but are registered by
HTTP/feature-scoped paths rather than the base registry, so the tripwire is accurate today.
Any phase that adds a base-registry migration must update that assertion in the same change.

Phases 0–5 need **no** migration: `valid_to` already exists on `claim`, and nothing else in
those phases alters a schema. **Phase 6 does** — `claim` is `SCHEMAFULL` and `edge` is
`TYPE RELATION`, so persisting `TrustClass` on either requires a new declared field.

Numbering is clean but the convention matters: every file from `040` to `052` exists on disk,
yet `versioned_migrations()` still ends at `039` — the later files are registered by
HTTP-scoped paths. `053` is therefore the next free number for either registry, and whichever
registry takes it must update `latest_registered_migration_is_expected` in the same change if it
is the base one.

CI gate every phase must pass:

```
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
cargo run --locked -p xtask -- check-toolchain-pin
cargo test --workspace --lib --bins --tests --locked
cargo run -p eval-harness --bin memory-eval --locked -- run \
  --profile evals/profiles/<profile>.json \
  --artifact target/evals/<profile>.json \
  --baseline evals/baselines/one-active-namespace-<profile>.json
make eval-response-size
```

## 6. Acceptance evidence

Completion is proven by committed artifacts and named scenarios, not by suite success:

1. An assembled context that is not silently stale for a corrected fact — observed failing
   before the fix — with the supersession or correction relation reachable from the read path,
   and from `explain` only if the ADR elected to project it there.
2. A genuine correction scenario added to `longmem_acceptance.rs`, distinguished from
   retraction, replacing the current test that names supersession but calls `invalidate`.
3. A committed artifact comparing this pipeline against BM25-only and dense-only on the same
   corpus and reader, retriever-only and end-to-end reported separately, stating its label-trust
   class and its limits.
4. A `FactConsolidation` adaptation — the MemoryAgentBench selective-forgetting corpus — reported
   alongside, never merged with, the LongMemEval and LoCoMo numbers.

## 6a. Decisions settled before implementation

The design tree was walked before implementation and these are settled, not open. Each is
recorded in an ADR or in `GLOSSARY.md` so it does not have to be re-derived.

| Decision | Choice | Where |
|---|---|---|
| Phase 0 stance | Mark and demote; never exclude. The fact stays retrievable with its relation attached | ADR-0074 |
| Which outcomes demote | `Supersession` and `Correction` only. `Duplicate` is redundancy, not staleness | ADR-0074, glossary |
| Demotion magnitude | Binary and conditional, not a coefficient: demote only when a successor is present. A demoted fact with no successor present is the least-stale value a reader has | ADR-0074 |
| Direction source | `claim_relation.predecessor_claim_id`/`successor_claim_id`, recorded once, never recomputed | ADR-0074 |
| Two pointers to the replacement | Collapse to one. `counterpart_fact_id` is never populated and duplicates stored direction. The one pointer that survives is **`superseded_by_fact_id: Option<String>`** on `ClaimRelationSummary`, added by Phase 0 so the reorder rule can name the successor's fact; direction still originates in `claim_relation` and is projected, never recomputed | ADR-0074, Phase 0 item 4 |
| When to remove it | A separate commit ahead of Phase 0, so Phase 0's diff stays attributable | Phase 0 item 4 |
| ADR-0074's rationale needs correcting | Its Consequences justify "no schema change" by pointing at `counterpart_fact_id` — the field this plan deletes. Phase 0 **does** add one field (`superseded_by_fact_id`), so that clause is amended in the same commit as the deletion. The decision itself (post-assembly reorder, no constant, no new candidate axis) stands unchanged | Phase 0 item 4, plan Global Constraints |
| Who may disclose relations | `evidence` only. `CLAIM_RECONCILIATION.md` forbids disclosure at `shadow`; ADR-0074 demands it unconditionally. Reconciled by gating the read path, with the threshold promotion left to the operator | Phase 0 item 6, ADR-0074 Consequences |
| Where the gate lives | The `knowledge`-owned adapter behind `RelationReadPort`, never in `memory/retrieval` — so no deployment can be talked into disclosure from the retrieval layer | Phase 0 item 6 |
| One gate covers both halves | Metadata and demotion are gated together, because `demote_superseded` reads the rows the gate withholds. Gating only one would produce either an unexplained reorder or metadata that names a winner the reader is not served | Phase 0 item 6 |
| Intent-routing surface | A new schema-skipped field. `view_mode` is a public string meaning "shape of presentation"; intent means "what to look for" | ADR-0076 |
| Intent vs `fact_types` | Intent selects a traversal plan, not a type predicate — `fact_types` already exists and is pinned as intentional | ADR-0076 |
| Trust vocabulary | Reuse `TrustClass`; two vocabularies would split the min-over-bases rule | ADR-0075 |
| Number of new ADRs | Three, not five. The arm seam is harness plumbing and the precision fix is a bug fix; neither earns a record | ADR-0074/75/76 |
| Glossary | The canonical glossary contained zero memory-domain terms while the plan distinguishes five relation outcomes | `GLOSSARY.md` |

One was decided by the architect rather than posed as a question: demotion magnitude. A fixed
coefficient was rejected as arbitrary and tunable against the metric; demoting by group needs a
`comparison_key` classification axis on the hot path; excluding was already rejected. The
conditional rule is the only one that has no constant and answers the question that actually
matters — what to do when the successor is not in the pack.

**What remains unverified after this plan:** real-client interoperability against a live
deployment is still not certified (carried from the 2026-10-03 follow-up); cross-tenant leakage
is gated by HTTP isolation tests but not measured against an adaptive attacker; MAGMA's
intent-routing claim stays unreproduced until Phase 3 has per-intent numbers; and nothing here is
evidence that the system beats full-context reading, which the literature says it currently does
not.

## 7. Non-goals

No ninth MCP tool. No shared service container given to a capability. No business logic in
`src/service/`. No new dependency without approval. No hard delete of facts. No manufactured
corrective fact as a retrieval side effect. No public quality claim unsupported by a committed
artifact. No metric quoted as a quality figure that cannot fail.

## 8. Corrections to the first draft

Recorded because the first draft's errors are instructive, and because this document is the audit
trail.

1. **"The claim layer has no read path" was missing entirely** — and it inverts the Phase 1
   diagnosis. The first draft said supersession leaves `valid_to` open and therefore returns
   stale values. True but insufficient: `valid_to` is never written *and* nothing reads the
   relations, so fixing the write changes no user-visible behaviour. The passing acceptance
   test does not test supersession at all — it calls `invalidate`, which is retraction.
   Reconciliation is currently write-only. This is now §3.1 and drives the reordering of Phases
   0 and 1.
2. **"No committed external artifact" was overstated.** An `external-retrieval` suite *is*
   gated in `pr.json` and present in the committed baseline — on a two-case smoke slice, with
   `pr` scoring 1.0 on it. The accurate statement is that it is a two-case smoke slice gated as if
   it were external evidence, and that `target/` being gitignored means no run artifact is
   committed at all.
3. **"The claim-reconciliation metrics exist."** They do not, apart from
   `claim_precision`/`claim_recall`/`claim_f1`. Six of the seven metrics in the
   `CLAIM_RECONCILIATION.md` contract have no implementing code and no metric key. The first
   draft repeated the documentation as fact — precisely the failure mode ADR-0073 exists to
   prevent.
4. **MAGMA was characterized from its abstract before being read.** In full, its LoCoMo advantage
   is dominated by the adversarial column (0.742 vs A-MEM 0.616); excluding it, MAGMA is *last*
   of five on single-hop (0.528), its temporal margin over Nemori is 0.001, its default embedding
   model is a 22M-parameter distilled encoder rather than anything comparable to this
   repository's stack, and its own limitations section names LLM-inferred graph quality as its
   principal fragility. The conclusion — take routing, skip the vocabulary — survives; the
   supporting argument is now accurate.

Retained after re-verification: the 039 migration tripwire (040–052 are registered outside the
base registry); `MEM1`'s numbers and their non-portability; Mem0's graph ablation direction and
magnitude; MemoryAgentBench's structure-augmented rankings; LongMemEval §5.2 on summaries and
§5.5 on reading; the procedural-memory and exposure-trace dead ends; the `access_count` novelty
inversion; and the `README.md:2141–2150` `make eval-*` drift.

### Third pass

A third round audited the second draft's own claims. Four more errors, three of them
introduced or inherited in round two:

1. **`mrr 0.9918` was asserted from the wrong source.** The committed baselines
   (`one-active-namespace-{pr,release}.json`) both record `0.9918`; the `0.9924` figure in the
   v3/v5 run reports belongs to an older 62-case suite and does not match current evidence.
   Anyone reconciling the artifact against the report would have found a spurious discrepancy.
2. **Phase 0 named the wrong owner.** It said to join claims "through the existing
   `ContextRetrievalPort` seam", which would put claim SQL inside `memory/retrieval`. ADR-0058
   assigns claims and relations to `knowledge`; the lookup belongs there behind a named method
   (ADR-0044), consumed by `memory`. The seam is what gets decorated, not what owns the query.
3. **Phase 2 would have broken ADR-0040.** It proposed per-arm retriever overrides without
   saying how they enter production code. `RetrievalContext` is a concrete crate-private struct
   over an enumerated infrastructure set, and ADR-0040 explicitly rejects adding unrelated
   fields to it. An arm must be a parameter of the retrieval operation, or a separate
   `ContextRetrievalPort` implementation in the harness — never a new context field.
4. **"No external cases exist" was nearly repeated.** A first pass at this round appeared to
   show zero `external-retrieval` outcomes in the baseline, because `outcomes[].suite_id` is not
   a field — the suite id lives inside `case_key`. The two cases are real, pass, and score 1.0.
   Round two's "Correction 1" was right and survives; this is recorded because the false
   negative nearly reversed it.

Two claims were re-checked and confirmed rather than corrected: the decay worker does delegate
to `ClaimService::retract_fact_and_claims` with an explicit ADR-0039 citation and composes no
close SQL; and supersession appears in integration tests only as a persisted label, never as an
assertion about read-path behaviour, so §3.1's claim that no test exercises it stands.

### Fourth pass

A fourth round targeted Phases 4–6 and the plumbing Phase 2 depends on. Three errors, all in
the plan's own implementation guidance:

1. **Phase 2's fallback was unreachable.** It offered "a separate `ContextRetrievalPort`
   implementation in the harness" as the escape hatch. `ContextRetrievalPort` is `pub`, but the
   harness never names it — it drives the product through
   `AssembleContextCapability::assemble_context_from_service` over a real `MemoryService`, so a
   trait impl outside that path would never be exercised.
2. **Phase 2 would have broken the frozen tool schema.** The corrected instruction first said
   "a new field on the request". `AssembleContextRequest` derives `JsonSchema` and *is* the MCP
   tool schema (`tools/context.rs`, `tools/assemble_context.rs:37`); a new field there changes
   the frozen eight-tool surface. The struct already shows the correct pattern — `access` uses
   `#[serde(skip_serializing)]` plus `#[schemars(skip)]` — and the plan now requires exactly
   that, so the selector is invisible to any tool call.
3. **"No phase in this plan requires a migration" was false.** Phase 6 persists `TrustClass`,
   and `claim` is `DEFINE TABLE claim SCHEMAFULL` while `edge` is `TYPE RELATION`; both reject
   an undeclared field. Phase 6 now carries an explicit migration requirement and names `053`.
   A related imprecision — that HTTP migrations occupy "041 and 045–052" — was also corrected:
   every file `040`–`052` exists on disk, and none is in the base registry.

Confirmed in this round, unchanged: `TrustClass` appears nowhere in `models/claim.rs` or
`models/domain.rs`, so Phase 6's gap is real; `Edge` carries no `namespace` field, so ADR-0038
is not engaged by adding trust; ADR-0051's `candidate`/`known_good` split with promotion
gated to the next start does exist and remains the only in-repo rollback template;
`TrustPolicy::may_derive` is an exhaustive relation with the explicit rule that external
content can never promote itself, so Phase 6 item 3 extends rather than invents it; and type
selection at the extraction seam is real (`summary_parser.rs:209`,
`classify_structured_summary_fact_type`), which is what makes Phase 4's "gates at the existing
seam" landable without a parallel mechanism.

### Fifth pass

A fifth round looked for internal contradictions rather than external facts, since §2.1–2.6 and
Phases 0–6 had each been checked against source. One real defect, and it was a dependency
inversion between two phases:

1. **Phase 4 depended on Phase 5.** Phase 4 item 2 required replacing the `access_count`
   novelty penalty with a use-based signal, then admitted that this "requires Phase 5's trace
   data" — while sitting *before* Phase 5. As written, either the phase blocks on a later one or
   someone ships a guessed proxy for "used". The work now lives in Phase 5 item 4, where the
   trace it depends on is produced two lines above it, and Phase 4 keeps only its two
   independent items with an explicit deferral. An off-by-one slip in the first fix (writing
   "Phase 6" while placing the item in Phase 5) was caught and corrected in the same pass.

Verified in this round, unchanged: the three quotations the document attributes to repository
files are verbatim — `CONTEXT.md:386` on manufacturing corrective facts, `docs/evals/README.md:42`
on not being a leaderboard score, and `GLOSSARY.md:3–4` on terms being canonical with that file
winning on conflict. All three were inherited from a subagent report rather than read directly,
which is exactly the path that produced four of the earlier errors.
### Sixth pass — found during implementation, not during review

Five rounds of reading the spec against source missed a conflict that only surfaced once the
code was being written. Every round asked *"is this claim about the repository true?"*; none
asked *"does this phase contradict a contract the repository already enforces?"*

1. **Phase 0 as written violated a rollout contract.** `docs/evals/CLAIM_RECONCILIATION.md`
   states that the default `shadow` stage "projects claims but does not expose relations in
   `assemble_context`" and ties promotion to `evidence` to precision/recall thresholds. ADR-0074,
   written the same day as the first draft, says an item *must* expose its relations and never
   mentions the stage. The plan implemented ADR-0074 unconditionally, which would have shipped
   relations to every deployment before the thresholds that gate them were verified. Resolved
   with Phase 0 item 6: the read path serves relations only at `evidence`, the gate sits in the
   `knowledge`-owned adapter, and one gate covers the metadata and the reordering together
   because the reordering reads the rows the gate withholds. The contract is now true instead of
   vacuously true — it was previously unverifiable, since `assemble_context` never exposed
   relations at any stage.

2. **The evaluation harness measured the feature's absence.** `eval-harness` built its service
   with no claim rollout stage, so it ran at `shadow` on every profile. A run like that reports
   the *absence* of the read path as evidence about the read path. It now runs at `evidence`,
   with a comment recording that this does not promote the deployment default.

3. **Phase 0's promised external evidence is unattainable, and saying otherwise would repeat
   the first draft's central mistake.** The phase promised "a Phase 2 external run whose
   knowledge-update number is no longer structurally incapable of moving." There is no such
   number: no knowledge / update / temporal / supersession key appears in the artifact, and
   three of the seven metrics `CLAIM_RECONCILIATION.md` contracts have neither code nor key.
   This was already recorded as a correction in the third pass — but Phase 0's evidence list was
   never revised to match, so the document still promised what it had already proven absent.
   The evidence list now states what was obtained, what was not, and why.

The lesson is narrow and worth stating: verifying claims about a system and verifying that a
plan is *consistent with* the system are different checks, and only the second one was being
run. Contracts written in `docs/` count as constraints whether or not an ADR mentions them.
