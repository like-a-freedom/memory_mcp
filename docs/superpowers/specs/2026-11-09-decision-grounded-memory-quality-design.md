# Decision-grounded evaluation of current memory and retention

- **Date:** 2026-11-09
- **Status:** Proposed design; evaluation first, product policy changes conditional
- **Baseline:** `3622ceb3` (`v1.38.0`)
- **Scope:** Claim/currency read behavior and lifecycle decay/archival decisions on the existing eight-tool surface

## Intent

Measure whether the system supplies the right evidence and answer for a query at a given valid-time and transaction-time cutoff, including after memory is superseded, contradicted, decayed, or archived. Use the measurement to decide whether a typed retention policy is justified. Do not infer memory quality from the number of stored facts, relation accuracy alone, or a benchmark's self-reported consolidation score.

This combines two improvements because the same query outcomes test both: conflict/version selection determines **which** memory is current, while retention determines **whether** that evidence is still available. A separate LongMemEval-V2 specification covers the external benchmark and reader protocol; this specification defines the internal decision-quality contract and ablations.

## Verified baseline and research

| Observation | Design implication |
| --- | --- |
| [ADR-0010](../../adr/0010-persist-versioned-claim-relations.md) already defines durable, versioned `Duplicate`, `Supersession`, `Correction`, `Contradiction`, and `TemporalAmbiguity` relations. [ADR-0074](../../adr/0074-connect-the-reconciliation-relation-to-the-read-path.md) connects relations to retrieval. | No parallel `Belief` object, new relation vocabulary, or new `conflicts` MCP tool. |
| [`ClaimRolloutStage`](../../../crates/memory-mcp/src/config/claims.rs) defaults to `Shadow`; the read path exposes relation evidence only in `Evidence` stage. [`project_current_candidates`](../../../crates/memory-mcp/src/memory/retrieval/currency.rs) suppresses known predecessors for Current intent and can return `currency_unknown`. | Compare shadow and evidence on identical data; test safe uncertainty, not just relation creation. |
| [`run_decay_pass`](../../../crates/memory-mcp/src/memory/lifecycle_workers/decay.rs) uses one `half_life_days`, `access_count`/`last_accessed` heat, and retracts facts with derived claims. [`archival`](../../../crates/memory-mcp/src/memory/lifecycle_workers/archival.rs) can archive old episodes when no active facts remain. | Replay retention decisions with future query utility; access is a retrieval count, not proof of usefulness. |
| [ADR-0076](../../adr/0076-route-retrieval-by-intent-and-type-memory-by-retention-class.md) chooses the existing `FactType` axis (`Note`, `Decision`, `Metric`, `Promise`, `Experience`) for any typed policy. | No second taxonomy or learned volatility prior without evidence. |
| The [Epistemics of Agent Memory paper](https://arxiv.org/html/2609.33013) reports that its consolidation-quality score does not predict transfer accuracy in its measured panel (pooled Spearman ρ = −0.24, n = 12, confidence interval spans zero). | Use downstream query/answer outcomes and keep governance measures separate; do not import its score as an acceptance gate. |

The historical [2026-10-05 memory-quality review](2026-10-05-memory-quality-evidence-review.md) was written against v1.26.1 and contains later revision notes. Its old “relation invisible to retrieval” diagnosis is no longer the v1.38.0 baseline. Current code and accepted ADRs take precedence.

## Goals and boundaries

**Goals**

1. Establish a reproducible, leakage-resistant internal eval with fixed historical cutoffs and future queries, covering current value, change history, contradiction, uncertainty, procedural experience, and retrieval after decay/archival.
2. Report retrieval evidence, answer correctness, freshness/temporal correctness, abstention, latency and tokens separately. Keep per-case provenance and failure classification.
3. Compare current behavior with an identical-data no-decay counterfactual and a candidate policy based only on existing `FactType`, before any policy change.
4. Make conflict evidence decision-ready through existing retrieval responses where it demonstrably improves answers, with no eighth-tool-surface expansion.

**Non-goals:** A new MCP tool, auto-resolution of contested claims, an LLM that rewrites primary memory, physical erase, a new fact taxonomy, trained retention, or claim-confidence probabilities. A relation is evidence, not automatic authority to invalidate a source. This work does not promote `Evidence` rollout merely because relation precision is high.

## Evaluation design

### 1. Fixed corpus and truth labels

Construct a versioned corpus from existing pinned eval fixtures plus targeted new scenarios. Each case includes source episodes, ingest order, `t_ref`, `t_ingested`, query time and transaction cutoff, expected evidence IDs, answer or permitted abstention, and reason for the label. Include positive and adversarial pairs: a current successor vs an old predecessor, two genuinely conflicting live sources, temporal ambiguity, a missing or cyclic successor chain, an invalidated claim, identical-content duplicates, an archived episode needed later, a durable preference/experience, and a noisy short-lived metric. Labelers see source evidence but not candidate outputs. Hold out a slice for the final policy comparison; pre-register cases and scoring before running that slice.

For retention, replay only evidence that was available at each decision time. Future queries are labels, never input to the policy. Feed `access_count` and `last_accessed` only from a separately recorded, chronological **pre-decision** retrieval workload. If no such history exists, mark the case as cold rather than inventing accesses. Scored future queries run against isolated snapshots or read-only probes whose access counters cannot affect later policy decisions. Use isolated stores or controlled adapters, not mutation of live personal/SaaS data. Preserve the distinction between valid time and ingested time; a late-arriving correction must not appear in an earlier transaction-time answer. Record corpus license and privacy handling for any non-synthetic source.

### 2. Compared arms

| Arm | Purpose |
| --- | --- |
| Incumbent | Current rollout stage, retrieval and lifecycle settings, fixed at baseline commit/config. |
| No decay/archival counterfactual | Upper bound on availability, with added storage/context cost measured. It is not the proposed shipping policy. |
| Evidence-stage relation read | Same writes/data, relation evidence visible to Current intent, with no other retrieval changes. |
| Candidate `FactType` retention | Shadow policy over `Note`, `Decision`, `Metric`, `Promise`, `Experience`; use only if previous arms show a material retention failure. |

Keep embedding model, extraction model/config, reader, token budget, query set, cutoffs, hardware class, and random seeds fixed within a comparison. Compare relation-read stages first with retention fixed at the incumbent. Select the read stage for the next experiment based on that gate; then compare incumbent, no-decay, and `FactType` retention with the selected read stage fixed. An optional interaction arm can follow, but cannot replace these controlled contrasts. If a dependency is nondeterministic, save outputs and report variance over repeated runs rather than attributing noise to the policy. The baseline arm must be runnable from a pinned artifact.

### 3. Measures and gates

Per case, record whether the answer used the gold current source, a stale predecessor, both sides of a true live contradiction, or no supported evidence; whether it answered correctly or abstained; whether a necessary item was removed by decay/archival; and retrieval/reader latency and context tokens. **Primary outcome:** source-supported answer correctness, with appropriate abstention counted correct when the case has no answerable evidence. Evidence availability is a diagnostic and a safety constraint, not a substitute for the answer outcome. Aggregate by query intent, `FactType`, relation outcome, age bucket, and profile. Report denominators and confidence intervals; surface every hard regression as a case ID with source evidence.

Use two gate layers:

- **Safety invariants:** zero known predecessor presented as the sole current truth when a valid successor is visible at the cutoff; zero future-ingested evidence in a past transaction-time answer; zero fabricated conflict resolution; zero silent data loss from a failed replay. Curated adversarial cases must pass individually.
- **Comparative gate:** a proposed relation-read or retention change must improve the pre-registered downstream answer/availability objective on the held-out slice without a material regression in temporal correctness, abstention, latency, token budget, or another `FactType` stratum. Decide the numeric materiality thresholds from the measured incumbent distribution **before** inspecting the held-out results, and publish them in the run profile. If the sample is too small to distinguish improvement from noise, the decision is “insufficient evidence,” not promotion.

Do not use `claim_precision`/`claim_recall` alone as gates for answer quality. Reconcile the documented metrics in [`CLAIM_RECONCILIATION.md`](../../evals/CLAIM_RECONCILIATION.md) against executable metrics before referring to any threshold as implemented.

## Conditional product behavior

If the relation-read arm clears the gate, keep the existing `assemble_context`/`explain` family as the output surface. Return a concise, source-linked indication that a current candidate supersedes an older one, that live sources conflict, or that currency is unknown; distinguish these states from `t_invalid` and from a low ranking score. Do not create a new `contested` database status unless the existing relation model cannot express the necessary answer and that gap is demonstrated. A correction/retraction decision remains an explicit owner use case, audited and bi-temporal. Promotion from `Shadow` to `Evidence` is staged and reversible by config.

If retention experiments show a gain, implement one versioned, deterministic policy over existing `FactType` in the owning context. Record policy version and decision reason for each shadow pass and actual retraction so a replay can explain which policy would have kept or removed a fact. Preserve source episodes and the bi-temporal audit trail. Do not derive usefulness from `access_count` alone; it counts retrieval events, not successful use. If the evidence is equivocal, keep the scalar half-life and ship the eval only.

## Alternatives considered

| Approach | Decision | Reason |
| --- | --- | --- |
| Add `conflicts` as a ninth MCP tool | Reject for this scope | The eight-tool surface is frozen; current relation/read mechanisms already cover much of the need. A new tool needs a separate ADR and demonstrated gap. |
| Per-predicate volatility or learned priors now | Defer | No validated outcome traces, versioning, or comparative benefit yet. `FactType` is the accepted policy axis. |
| Adopt a published consolidation score as product metric | Reject | The cited study explicitly did not find transfer prediction. |
| Treat “no decay” as the final policy | Reject | It reveals false forgetting but omits cost and stale-noise tradeoffs. |

## Acceptance and sequence

1. Pin corpus, labels, profiles, cutoffs, reader and replay protocol. Show no leakage from future episodes or labels into policy/query inputs.
2. Run incumbent and no-decay arms; publish per-case failures and cost/latency. Confirm that the archival scenario actually traverses the real archival/retrieval path.
3. Run relation-read arm with the same data and demonstrate source-linked current/conflict/unknown responses in integration tests. Decide whether rollout promotion is warranted.
4. Only if the retention signal is clear, run a shadow `FactType` policy and review the held-out results. A proposed production policy needs independent integration tests for retraction of facts and derived claims, restart/replay idempotence, and historical cutoffs.
5. Publish an evaluation artifact with version hashes, case manifest, denominators, failures, variance/intervals, and the promotion decision. A smoke slice is labelled smoke, never external benchmark proof.

Before any product change ships, use the repository's narrow tests and required workspace clippy gate. This specification itself changes no policy or stored data.
