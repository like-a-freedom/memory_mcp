# Truthful Evaluation System Design

Status: Proposed for review  
Date: 2026-07-28  
Scope: evaluation architecture and migration planning; no production API changes

## 1. Purpose

This design turns the current collection of evaluation tests into a truthful,
bounded, reproducible evaluation system for `memory_mcp`.

The priorities are:

1. Results reflect the real product path and never convert missing evidence,
   infrastructure failure, or benchmark-specific shortcuts into success.
2. The normal release evaluation completes within 20 minutes without hiding
   quality loss; the pull-request profile targets 10 minutes or less.
3. Evaluation code has an explicit domain model, one orchestration boundary,
   and idiomatic Rust ownership.
4. Results are reproducible, attributable to exact corpora and configuration,
   and useful for regression diagnosis.

This document describes the target system. Implementation is intentionally
deferred to separately reviewed plans.

## 2. Evidence from the current system

The review found several places where the reported result is more optimistic
than the behavior being measured:

- `recall_at_5` counts a relevant item at any returned rank rather than
  restricting evaluation to the first five results.
- Empty suites produce perfect aggregate values instead of an invalid run.
- Claim-reconciliation matching accepts a warning when a source ID appears as
  a substring, substitutes the current time for an invalid timestamp, and
  reports expected skip codes as isolation violations.
- Several ignored eval runners print threshold failures but do not fail their
  process.
- External retrieval seeds production extraction and then inserts the expected
  text directly as a `note` fact. This pays the extraction cost while masking
  extraction failures, so it is neither a clean retrieval benchmark nor a
  truthful end-to-end benchmark.
- Strict external evaluation forces query concurrency to one and processes
  context batches sequentially. Strictness and scheduling are coupled even
  though they are independent concerns.
- Lifecycle action-grounding, capacity, and poisoning evals exercise proxy
  functions or count expected policy outcomes instead of observing the wired
  lifecycle behavior required by ADR-0017.
- The Makefile baseline omits several suites, is sequential, and compares
  unstructured stdout while ignoring comparison failures.
- Correctness tests contain machine-dependent wall-clock assertions, while the
  benchmark environment, warm-up, model, and configuration are not
  fingerprinted.
- External datasets are downloaded from mutable branch URLs without a pinned
  revision or checksum, and samples are selected by prefix rather than stable
  stratification.

Observed diagnostic runs reinforce the distinction between a runnable suite and
a valid gate:

- Extraction ran 9 cases with 7 case passes, entity precision 0.60, entity
  recall 1.00, entity F1 0.75, fact-type accuracy 1.00, and warning recall
  0.60.
- Retrieval ran 66 cases with 65 case passes, reported recall 1.00, MRR 0.98,
  and top-1 accuracy 0.97, while the direct tier was 0.94 against a 0.95 target.
- Claim reconciliation reported zero precision and zero recall on both
  development and test splits but still printed PASS.
- A single one-percent LongMemEval case did not reach its query within
  150 seconds, making the current end-to-end path incompatible with a
  10–20-minute release budget without architectural separation and profiling.

These observations are diagnostic snapshots, not new baselines.

## 3. Evaluation domain model

### 3.1 Eval Profile

An Eval Profile defines why a run exists and the maximum resources it may use.

| Profile | Purpose | Target budget | Contents |
|---|---|---:|---|
| `pr` | Fast regression feedback | at most 10 min | deterministic regression suites and a stable stratified external sample |
| `release` | Product quality gate | at most 20 min | full retrieval-only corpora, lifecycle release gate, and pinned-runner performance checks; deterministic sharding is allowed |
| `nightly` | Broad diagnosis | established after profiling | full end-to-end ingestion, extraction, claims, retrieval, downstream diagnostics, and performance characterization |

The time budget is a profile acceptance criterion, not permission to drop,
skip, or silently sample cases. If a profile cannot complete its declared
coverage, the run is invalid.

### 3.2 Eval Mode

Modes identify the system path being measured. Their headline metrics must
never be merged.

- `retrieval-only`: import canonical, provenance-bearing facts through a
  private evaluation adapter, then call the production retrieval path. It does
  not run extraction, claim generation, or embedding generation as part of the
  measured setup.
- `end-to-end`: use only production ingest, extraction, reconciliation, and
  `assemble_context` paths. Direct insertion of oracle facts is forbidden.
- `lifecycle`: exercise wired `LifecycleCapture` and `LifecycleRecall` entry
  points and observe persisted state, bounded recall envelopes, trust behavior,
  and resulting actions.
- `performance`: measure isolated stages and the full pipeline with a benchmark
  harness, not correctness-test timers.

The retrieval-only adapter is private to the eval harness. It is not an MCP
tool, production capability, or supported external storage interface.

### 3.3 Eval Case Outcome

Every selected case has exactly one outcome:

- `passed`: execution completed and all case quality requirements passed.
- `quality_failed`: execution completed, but one or more quality requirements
  failed.
- `invalid`: the intended measurement could not be made because setup,
  corpus validation, parsing, provider, persistence, timeout, or other
  infrastructure failed.

An invalid case stays in coverage and in the artifact. It is never reported as
skipped, passed, or removed from a denominator. An empty or incomplete suite is
invalid. All selected cases run when continuing is safe; gating occurs after
the report is assembled so failures remain diagnosable.

### 3.4 Label Trust

Ground truth carries one of three trust levels:

1. `official`: an official dataset identifier or label.
2. `reviewed`: a project-maintained, human-reviewed mapping with provenance.
3. `weak`: heuristic or inferred relevance.

Weak labels are reported as a separate diagnostic slice and cannot contribute
to the release gate.

## 4. Target architecture

The evaluation implementation becomes a private workspace crate named
`eval-harness`.

```text
eval-harness
├── domain          # profiles, modes, outcomes, metrics, gates
├── corpus          # manifests, validation, sampling, dataset adapters
├── runner          # deterministic scheduling, sharding, orchestration
├── adapters        # private canonical-fact import and production-path drivers
├── evaluators      # retrieval, extraction, claims, lifecycle, downstream QA
├── artifact        # versioned JSON schema and concise human summary
└── cli             # one entry point consumed by Make and CI
```

Repository placement follows these boundaries:

- Ordinary unit, integration, and regression correctness tests remain under
  `tests/`.
- Criterion performance targets live under `benches/`.
- Dataset-specific parsing remains behind one corpus-adapter seam.
- Evaluation policy and orchestration do not live in production modules.
- Existing `tests/eval_*.rs` files may temporarily be thin compatibility
  launchers and are removed after profile parity is demonstrated.
- The eval harness is excluded from the shipped release binary and owns its
  evaluation-only dependencies.

The Makefile becomes a thin command adapter. It contains no suite lists,
threshold policy, sampling behavior, or stdout-diff logic.

## 5. Truth contract

The following invariants apply to every profile:

1. Metrics operate on explicit result cutoffs and defined denominators.
2. Empty input, missing expected evidence, malformed timestamps, unavailable
   providers, and corpus mismatch are errors or invalid outcomes, never
   optimistic defaults.
3. Expected and observed identifiers use typed equality or explicit normalized
   matching; substring coincidence is not evidence.
4. Development splits are diagnostic. Frozen test splits gate releases.
5. A change to a threshold, frozen test corpus, label, or evaluator requires
   review and a before/after artifact.
6. A retry success is reported as flaky, preserving the initial failure and
   retry history.
7. Report order is deterministic regardless of execution order.
8. Every artifact records mode, profile, build, features, provider, model,
   device, configuration hash, corpus versions, and selected case IDs.

## 6. External corpus provenance

Evaluation never downloads or mutates a corpus. A separate explicit preparation
command:

1. Fetches a declared immutable revision.
2. Validates the SHA-256 digest.
3. Records URL, revision, digest, license, byte size, case count, and adapter
   version in a manifest.
4. Writes data to a prepared location outside the measured run.

Missing data or a manifest mismatch invalidates the affected suite.

Sampling uses a stable hash of dataset identity and case ID, followed by
declared stratification. Sharding uses the same stable identity and records
selected IDs and coverage in the artifact. Prefix sampling is not allowed.

## 7. Scheduling and the time budget

Speed improvements preserve the declared measurement:

- Seed independent contexts concurrently with a bounded worker pool.
- Run independent queries within a shared immutable seeded context with a
  separate bounded worker pool.
- Derive worker defaults from profiled resources such as database contention,
  embedding device, and memory, not from strictness or arbitrary environment
  values.
- Keep strictness as a gating policy only.
- Use stable hash sharding for complete release-corpus coverage across workers.
- Merge shards only when manifest, evaluator, configuration, and expected
  coverage fingerprints match.
- Measure stage durations before fixing concurrency limits.

The 10- and 20-minute goals are accepted only when achieved on a declared
runner class with complete profile coverage. If end-to-end work remains too
expensive, it stays nightly rather than being disguised as retrieval-only or
silently reduced.

## 8. Metrics and gates

### 8.1 Retrieval

The release gate measures the product-owned output of `assemble_context`:

- recall at an explicit cutoff;
- reciprocal rank and top-rank accuracy;
- source and citation correctness;
- temporal validity;
- scope and trust isolation;
- bounded context-envelope behavior.

Metrics preserve corpus-specific official measures and slices. A shared metric
name has one shared definition.

### 8.2 Downstream QA

Reader-generated answers are a separate downstream diagnostic. They do not
contribute to the initial release gate because a changing reader model, prompt,
or provider would confound memory quality.

Downstream QA may become a gate only after the reader model, prompt, decoding
parameters, provider version, and evaluator are pinned. LongMemEval v2 remains
a separate future profile until multimodal or agentic scenarios are product
requirements.

### 8.3 Gate policy

A release gate combines:

- hard semantic floors derived from product use cases; and
- a regression budget relative to the last approved baseline artifact.

Falling below a hard floor always fails. Exceeding a regression budget fails
even when the floor still passes. Replacing a baseline requires review,
before/after artifacts, and a reason. The frozen test split cannot be changed
in the same change that fixes behavior detected by that split.

Performance regression gates account for confidence intervals and noise.
Semantic case failures do not receive a statistical-noise exemption.

## 9. Lifecycle evidence

This design implements, rather than replaces, ADR-0017:

- Proxy evals are renamed or retained as unit tests.
- Action grounding runs through wired `LifecycleRecall`.
- The comparison includes `always_recall`, `selective_shadow`, and
  `selective_enforced`.
- Grounding is determined from an observed consequential action outcome, not
  from a recall trace.
- Capacity uses persisted rows and bytes.
- Poisoning follows capture, recall, and attempted action, verifying that
  untrusted content cannot become privileged instruction.
- The release gate asserts real event and job records, zero growth for
  ignored/duplicate capture, envelope bounds, the fixed memory-data preamble,
  leakage constraints, and trust non-elevation.

No additional lifecycle ADR is needed unless the comparative baseline deferred
by ADR-0017 becomes a product requirement.

## 10. Performance evaluation

Wall-clock assertions move from correctness tests to Criterion benchmarks.
Benchmark families cover:

- ingest;
- extraction;
- claim reconciliation;
- retrieval;
- complete end-to-end processing;
- NER on CPU;
- NER on Metal;
- explicitly contended variants.

Criterion supplies warm-up, sampling, confidence intervals, outlier reporting,
and named baselines. Artifacts include hardware, OS, Rust version, build
profile, features, provider/model, device, and configuration fingerprints.

Absolute performance gates run only on a pinned runner. Pull requests retain
gross timeouts and may gate only large, repeatable regressions. Performance
benchmarks are part of the release profile, not ordinary `cargo test`.

## 11. Artifact and orchestration contract

One manifest-driven runner owns suite selection and gating. Profiles declare:

- suites and modes;
- corpus and sample;
- expected case coverage;
- shard count;
- concurrency policy;
- build configuration;
- thresholds and regression budgets.

The runner emits:

1. Versioned JSON containing every case outcome, invalid reason, metric,
   threshold, gate decision, retry, fingerprint, duration, selected ID, and
   coverage value.
2. A short deterministic human summary derived from that JSON.

CI uploads the JSON artifact even when the run fails or is invalid. Comparisons
operate on typed schema fields and reject incompatible schema versions instead
of diffing stdout.

## 12. Rejected alternatives

### Keep all evals as ignored integration tests

Rejected because integration-test binaries do not provide a coherent profile,
artifact, corpus, or scheduling contract, and evaluation-only dependencies
continue to leak into the main crate.

### Use only end-to-end evaluation

Rejected because it makes retrieval regressions slow and hard to localize and
cannot meet the release budget today. End-to-end evidence remains mandatory in
the nightly profile.

### Use only canonical-fact retrieval

Rejected because it cannot detect extraction, claim-reconciliation, or
lifecycle failures. Retrieval-only is one explicitly labelled mode.

### Reduce runtime by dropping cases or using prefix samples

Rejected because coverage would change implicitly and rare case families could
disappear. Stable stratification and complete sharding preserve declared
coverage.

### Gate downstream LLM answers as memory quality

Rejected initially because the reader introduces a separate unpinned system.
It remains visible as a diagnostic benchmark.

## 13. Migration sequence

Implementation is divided into three reviewed plans:

1. **Evaluation foundation**: create `eval-harness`; implement domain types,
   Truth Contract, profiles, runner, artifact schema, gates, and compatibility
   launchers.
2. **Corpus pipeline**: add immutable manifests, preparation/validation,
   dataset adapters, label trust, stable sampling, full-corpus sharding, and
   retrieval-only import.
3. **Realistic and performance evals**: migrate lifecycle evidence required by
   ADR-0017, poisoning/capacity checks, Criterion benchmarks, CI profiles, and
   remove superseded orchestration.

Each plan must define test-first tasks, parity evidence, time-budget evidence,
and rollback boundaries. Independent corpus adapters and benchmark families may
be implemented in parallel only after the foundation contracts exist.

## 14. Acceptance criteria

The design is implemented only when:

- Every selected case appears exactly once in the artifact as passed,
  quality-failed, or invalid.
- An empty, incomplete, or corpus-mismatched run cannot pass.
- Retrieval-only and end-to-end results are visibly separate.
- Release evaluation covers the complete declared retrieval corpora and
  finishes within 20 minutes on the declared runner, or the unmet budget is
  reported without weakening coverage.
- The PR profile finishes within 10 minutes on its declared runner.
- Lifecycle release evidence exercises wired production entry points and
  satisfies ADR-0017.
- Performance results are reproducible on a pinned runner and do not run as
  correctness tests.
- CI always preserves a schema-valid artifact.
- The old Makefile/stdout baseline mechanism and direct-oracle hybrid seeding
  are removed after parity is proven.

## 15. Decisions and supporting records

- ADR-0017 remains authoritative for the lifecycle release evidence.
- ADR-0019 records the profile-driven evaluation architecture.
- ADR-0020 records immutable corpus provenance and label trust.
- Performance tooling, bounded concurrency, and file decomposition are
  reversible implementation choices governed by this design and do not need
  separate ADRs.

