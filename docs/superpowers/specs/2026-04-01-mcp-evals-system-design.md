# MCP Evals System Design Specification

**Created:** 2026-04-01  
**Status:** Proposed  
**Scope:** Repository-local metric evaluation harness for `memory_mcp`

---

## 1. Overview

This specification defines an evaluation system for `memory_mcp` that measures **quality**, not just functional correctness. The repository already has strong correctness coverage in tests such as `tests/longmem_acceptance.rs`, `tests/service_acceptance.rs`, and `tests/promise_detection.rs`, but those tests answer “does it work?” rather than “how well does it work?”

The eval system introduced here adds repeatable, repository-local measurement for three areas:

1. **Retrieval quality** for `assemble_context`
2. **Extraction quality** for `extract` and the configured entity/fact extraction stack
3. **Latency** for `ingest`, `extract`, and `assemble_context`

The design must preserve the repository’s existing architectural direction:

- use the real `MemoryService` and real SurrealDB query engine,
- avoid public MCP surface growth,
- keep the runtime local-first and deterministic,
- and make evaluation runs safe to repeat without contaminating future runs.

---

## 2. Current State

### 2.1 What the repository already verifies well

The current test suite already covers important correctness scenarios:

- `tests/longmem_acceptance.rs` verifies LongMemEval-style behavior categories such as multi-session retrieval, abstention, temporal correctness, and knowledge update at the acceptance-test level.
- `tests/service_acceptance.rs` verifies `ingest` → `extract` → `assemble_context`, invalidation, access control, graph traversal, and multi-word lexical retrieval.
- `tests/promise_detection.rs` verifies the extraction of a `promise` fact from a simple sentence.
- `tests/common/mod.rs` and `tests/embedded_support.rs` already provide a real **in-memory SurrealDB** setup using `SurrealDbClient::connect_in_memory_with_namespaces(...)` and applied migrations.

### 2.2 What is missing

The current repository does **not** yet provide:

- repository-local **retrieval metrics** such as Recall@K, Precision@K, or MRR,
- repository-local **extraction metrics** such as entity precision/recall/F1 or fact-type accuracy,
- stable **latency summaries** such as p50/p95/p99 for the main memory flows,
- a fixture format and runner contract for repeatable metric evals,
- an explicit guardrail preventing benchmark state from persisting across sessions.

---

## 3. Goals and Non-Goals

### 3.1 Goals

The eval system MUST:

1. provide metric-driven evaluation for retrieval, extraction, and latency;
2. reuse the real service stack instead of mocks for primary eval runs;
3. run every DB-backed eval against **embedded in-memory SurrealDB only**;
4. avoid persisting benchmark state, cached result sets, baseline comparisons, or DB files across sessions;
5. keep evaluation output deterministic enough to catch regressions without pretending latency is perfectly deterministic;
6. remain opt-in and manual by default, so the normal developer loop stays fast.

### 3.2 Non-Goals

Phase 1 does **not** attempt to:

- reproduce the full external LongMemEval benchmark,
- add a new MCP tool for evaluation,
- store historical benchmark baselines inside the repository,
- introduce a new storage backend,
- require a remote database, external service, or hosted dashboard,
- turn latency into a hard CI gate.

---

## 4. Core Design Principles

### 4.1 Real engine, ephemeral state

All DB-backed evals and benches MUST use the existing embedded in-memory SurrealDB path:

- `SurrealDbClient::connect_in_memory(...)`, or
- `SurrealDbClient::connect_in_memory_with_namespaces(...)`

This is a hard requirement, not a preference.

Rationale:

- avoids polluting the repo or user machine with RocksDB artifacts,
- prevents state leakage between sessions,
- ensures previous evaluation runs cannot affect current scores,
- still exercises real SurrealDB query planning, migrations, and filtering behavior.

### 4.2 No persistent benchmark history

The eval system MUST NOT keep session-to-session benchmark history by default.

That means Phase 1 intentionally avoids a default Criterion-based workflow, because Criterion persists comparison history and reports under `target/criterion`, which conflicts with the requirement that runs should not keep results that might influence later evaluations.

Instead, Phase 1 uses **manual ignored integration tests** that:

- build a fresh in-memory service,
- seed deterministic fixtures,
- compute metrics in-process,
- print summaries to stdout,
- and exit without storing evaluation state.

If a future phase introduces Criterion or another benchmarking framework, it must be wrapped so that:

- the DB remains in-memory,
- benchmark output goes to a disposable temp directory,
- and no persistent baseline comparison is assumed.

### 4.3 Separate correctness from evals

Functional tests and metric evals serve different purposes.

- **Functional tests** assert that behavior is correct.
- **Metric evals** measure how strong the behavior is.

A correctness test should fail on a logic bug. An eval should fail only on a meaningful quality regression or when the harness itself is broken.

### 4.4 Deterministic fixtures, cautious thresholds

Fixture inputs and expected labels MUST be deterministic and repository-local.

Thresholds should be:

- strict enough to catch real regressions,
- loose enough to avoid flakiness,
- and documented next to the harness.

Latency thresholds should initially be **reporting-first** rather than hard-fail gates.

---

## 5. System Architecture

### 5.1 High-level shape

The eval system consists of four layers:

1. **Fixture layer** — JSON datasets for retrieval, extraction, and latency scenarios
2. **Harness layer** — shared Rust helpers to load fixtures, seed data, run a fresh in-memory service, and compute metrics
3. **Runner layer** — ignored integration tests for each eval family
4. **Reporting layer** — human-readable stdout summaries with optional temporary JSON serialization only when explicitly requested

### 5.2 Proposed file layout

```text
tests/
  eval_support/
    mod.rs
    dataset.rs
    metrics.rs
    report.rs
  eval_extraction.rs
  eval_retrieval.rs
  eval_latency.rs
  fixtures/
    evals/
      extraction_cases.json
      retrieval_cases.json
      latency_cases.json
```

### 5.3 Reused repository building blocks

The harness should reuse existing repository helpers wherever possible:

- `tests/common/mod.rs` for `make_service()`, `make_service_with_client()`, `ingest_episode()`, and seeding helpers
- `tests/embedded_support.rs` for embedded-service patterns
- the real `MemoryService`
- the real migrations applied to an in-memory SurrealDB instance

No parallel “fake memory” harness should be introduced for the primary eval path.

---

## 6. Evaluation Domains

### 6.1 Retrieval evals (`assemble_context`)

The retrieval eval suite measures whether relevant memory is surfaced in the top returned items.

#### Metrics

- **Recall@K** — fraction of required relevant facts present in top-K
- **Precision@K** — fraction of top-K results that are relevant
- **MRR** — reciprocal rank of the first relevant hit
- **Empty-when-irrelevant rate** — how often the system returns an empty result set when no answer exists
- **Noise violation count** — number of cases where explicitly forbidden distractors appear

#### Case structure

Each retrieval case includes:

- scenario ID and description,
- episodes to ingest,
- retrieval request (`query`, `scope`, `budget`, optional `as_of`),
- required matches,
- forbidden matches,
- optional exact fact IDs or quote fragments,
- expected minimum metric outcome.

#### Example fixture

```json
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
  "query": {
    "query": "alice atlas deck",
    "scope": "personal",
    "budget": 5,
    "as_of": null
  },
  "expected": {
    "must_contain": ["Atlas deck"],
    "must_not_contain": ["travel plans"],
    "min_recall_at_k": 1.0,
    "expect_empty": false
  }
}
```

### 6.2 Extraction evals (`extract`)

The extraction eval suite measures how well the current extraction stack produces entities and fact types from a known input.

#### Metrics

- **Entity precision**
- **Entity recall**
- **Entity F1**
- **Fact type accuracy**
- **Promise detection accuracy**
- **Metric detection accuracy**
- **Alias/canonicalization hit rate** for cases where multiple spellings point to the same logical entity

Phase 1 compares extracted output against deterministic expected labels in fixtures. It does not attempt to evaluate open-ended semantic equivalence.

#### Matching policy

- entity comparisons should be normalized by lowercasing and whitespace folding;
- `entity_type` must match exactly;
- fact-type comparisons should be exact-string matches against the repository’s current `fact_type: String` behavior;
- if a case expects both `metric` and `promise`, both must be found for the case to fully pass.

#### Example fixture

```json
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
```

### 6.3 Latency evals (`ingest`, `extract`, `assemble_context`)

The latency suite measures runtime cost for the main memory paths under controlled, in-memory conditions.

#### Measured operations

- `ingest`
- `extract`
- `assemble_context`

#### Metrics

- **p50**
- **p95**
- **p99**
- optional mean and max for debugging

#### Design constraints

- every latency case must build and use a fresh in-memory DB;
- each operation should run multiple iterations after warm-up;
- the runner should execute single-threaded (`--test-threads=1`) for repeatability;
- results are informational by default and should not fail CI unless the repository explicitly promotes them later.

#### Why not Criterion in Phase 1

Criterion is intentionally deferred because its default persistent output model conflicts with the repository requirement that eval runs should not store cross-session result artifacts by default.

A plain Rust latency harness inside ignored integration tests is sufficient for the first wave and better aligned with the ephemeral-state rule.

---

## 7. Data Contracts

### 7.1 Fixture contract rules

All fixture files MUST:

- be UTF-8 JSON stored under `tests/fixtures/evals/`;
- use stable IDs;
- contain repository-local, synthetic or sanitized text only;
- avoid secrets, customer data, or private correspondence;
- encode timestamps in RFC 3339 UTC format.

### 7.2 Schema families

Phase 1 uses three schema families:

- `RetrievalEvalCase`
- `ExtractionEvalCase`
- `LatencyEvalCase`

Each family should have a typed Rust struct with `serde` deserialization and clear validation errors.

### 7.3 Validation rules

Fixture loading should fail fast on:

- duplicate case IDs,
- empty expected labels,
- invalid timestamps,
- unsupported scopes,
- negative budgets or iteration counts,
- contradictory expectations such as `expect_empty=true` combined with non-empty `must_contain`.

---

## 8. Runner Design

### 8.1 Invocation model

All eval runners should be **manual, explicit, and ignored by default**.

Recommended commands:

```bash
cargo test --test eval_extraction -- --ignored --nocapture --test-threads=1
cargo test --test eval_retrieval -- --ignored --nocapture --test-threads=1
cargo test --test eval_latency -- --ignored --nocapture --test-threads=1
```

This keeps normal `cargo test` fast while giving developers a standard way to run metric evals.

### 8.2 Fresh service per case

Quality evals should create a fresh in-memory `MemoryService` per case to prevent cross-case contamination.

Latency evals may create one fresh service per scenario and then warm it up within the scenario, but they still must not reuse persisted state from previous sessions.

### 8.3 Output format

Each runner should print:

- suite name,
- number of cases,
- aggregate metrics,
- per-case failures,
- pass/fail summary.

Example stdout shape:

```text
suite=eval_retrieval total=24 passed=22 failed=2
recall_at_5=0.875 precision_at_5=0.733 mrr=0.791 empty_when_irrelevant=0.900
failed_cases=[ret-013, ret-021]
```

### 8.4 Optional machine-readable output

Machine-readable JSON output is allowed only when explicitly requested via an environment variable and only to a temporary location outside the repository tree.

Default behavior is **stdout only**.

If enabled, the harness should:

- create a temp directory,
- write one JSON report file,
- print its path,
- and allow the OS temp cleanup policy to remove it.

It MUST NOT write under `tests/fixtures/`, `docs/`, `data/`, or the database directory.

---

## 9. Threshold Policy

### 9.1 Quality gates

Phase 1 should use explicit but conservative thresholds.

Initial recommended gates:

- retrieval `Recall@5 >= 0.80`
- retrieval `empty_when_irrelevant_rate >= 0.90`
- extraction `fact_type_accuracy >= 0.85`
- extraction entity `F1 >= 0.75`

These are starting points, not sacred numbers. The exact thresholds should be documented next to the fixture suite and adjusted only with justification.

### 9.2 Latency gates

Latency starts as **report-only** in Phase 1.

The harness should surface p50/p95/p99 so regressions are visible, but should not fail by threshold until enough run history exists to set meaningful expectations.

---

## 10. Documentation Impact

The implementation derived from this design should update:

- `README.md` — how to run evals manually
- `docs/MEMORY_SYSTEM_SPEC.md` — mention that the repository now has a metric eval harness in addition to functional correctness tests
- optionally `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` — if needed, to cross-link this concrete eval-system design

This design document itself is the detailed source of truth for the eval-system architecture.

---

## 11. Risks and Mitigations

### 11.1 Flaky latency measurements

**Risk:** noisy machines produce unstable latency numbers.  
**Mitigation:** manual ignored runs, single-threaded execution, warm-up iterations, report-only thresholds in Phase 1.

### 11.2 Fixture drift after behavior changes

**Risk:** legitimate retrieval or extraction improvements may require fixture updates.  
**Mitigation:** keep fixtures small, explicit, reviewed, and version-controlled.

### 11.3 Hidden state contamination

**Risk:** benchmark outputs or DB artifacts persist between runs and change later measurements.  
**Mitigation:** in-memory DB only, no persistent result history by default, temp-only optional JSON output.

### 11.4 Overfitting to synthetic cases

**Risk:** the system passes the local evals but still performs poorly on broader real-world data.  
**Mitigation:** use domain-shaped cases (product, personal memory, org memory, cybersecurity/product artifacts) and expand coverage incrementally.

---

## 12. Acceptance Criteria for the Implementation Plan

The implementation plan derived from this design must produce a system where:

1. retrieval eval fixtures live in-repo and measure Recall@K, Precision@K, MRR, and empty-when-irrelevant behavior;
2. extraction eval fixtures live in-repo and measure entity precision/recall/F1 and fact-type accuracy;
3. latency runs measure `ingest`, `extract`, and `assemble_context` p50/p95/p99;
4. every DB-backed eval and bench uses embedded **in-memory** SurrealDB only;
5. eval runs do not persist DB state or benchmark-result history across sessions by default;
6. eval runners are manual and ignored by default, with documented commands;
7. normal `cargo test` remains focused on correctness, while eval runners are opt-in;
8. output is readable enough for humans and structured enough for later machine consumption.

---

## 13. Recommendation

Implement the eval system as a single repository-local subsystem with three ignored integration-test runners and shared support code. This is the smallest design that gives the repository hard metrics without violating the local-first, deterministic, and no-cross-session-artifacts constraints.
