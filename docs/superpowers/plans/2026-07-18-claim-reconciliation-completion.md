# Claim Reconciliation Completion and Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> **Required Rust discipline:** Apply `rust-skills` throughout execution, especially validated newtypes, parse-don't-validate, cancellation safety, no lock across `.await`, backward-compatible Serde defaults, structured redacted observability, property tests, and additive feature flags.

**Goal:** Finish the already-started claim reconciliation subsystem so contradiction detection is durable, local, high-precision, backward-compatible, observable, and actually used by `extract`, `assemble_context`, and `explain`.

**Architecture:** Keep immutable facts as the durable source of truth. Parse explicit local structure once into generic assertions, derive typed claims, persist each projection together with per-claim reconciliation jobs, compare only exact indexed claim slots, and append versioned relation decisions. A claim-specific local worker handles retries and historical backfill; MCP responses remain structured JSON and gain only optional reconciliation metadata.

**Tech Stack:** Rust 2024, Tokio 1.52.3, `tokio-util` 0.7.18, Serde/Schemars, Chrono, SHA-256, SurrealDB 3.2.1, `metrics` 0.24.6, optional `metrics-exporter-prometheus` 0.18.3, and `proptest` 1.11.0.

**Baseline:** This continuation plan is grounded in `master` at `d696ccbc`. It supersedes the unfinished execution portion of `docs/superpowers/plans/2026-07-18-claim-reconciliation.md`; the earlier document remains design history.

## Global Constraints

- Do not edit migrations `006` through `027`. Migration `027_claim_reconciliation.surql` is already committed and may already exist in user databases; every schema correction in this plan goes into new migration `028_claim_reconciliation_hardening.surql`.
- Applying the current binary to every supported older database prefix must automatically run missing migrations and return the server to readiness. Historical claim projection runs after readiness and never inside a migration transaction.
- The default path is local and zero-config. It must not require an LLM, remote inference, a daemon, a schema file, or a Prometheus server.
- Cosine similarity, recency, confidence, ingestion order, and observation time never decide contradiction, correction, or supersession. Embeddings may be evaluated as a future candidate-recall aid only after an exact `ComparisonKey` boundary and only with corpus evidence.
- No new MCP tool is added. Existing required response fields and flat snake_case argument contracts remain unchanged; new result fields are optional with `#[serde(default, skip_serializing_if = "Option::is_none")]`.
- Keep structured JSON as the MCP and CLI contract. Do not introduce pipe-delimited or other ad-hoc context serialization.
- `main.rs` stays thin. Claim rules live under `src/service/claims/`; SurrealQL, transactions, leases, and cursors live under `src/storage/claims.rs`.
- Keep `DbClient` backward-compatible. Extend only the narrow internal `ClaimStore` capability and its test doubles.
- Facts remain retrievable when claim work fails. Projection/reconciliation failure is durable and visible; it is never swallowed by fire-and-forget execution.
- Contradiction never invalidates a fact or claim. This plan does not enable automatic correction/supersession lifecycle effects; `MEMORY_CLAIM_ROLLOUT_STAGE=lifecycle` must return an explicit unsupported-stage configuration error until a separate safety review authorizes it.
- Use validated newtypes at boundaries, checked arithmetic for numeric normalization, `Result` rather than production panics, no lock held across `.await`, and cancellation-aware background work.
- `ClaimSchema` and `ComparisonKey` diagnostics must be available at trace level. Prometheus labels are bounded enums only; namespace, project, subject, key, fact, claim, relation, and job identifiers are never labels.

---

## Review Disposition

| Review proposal | Decision | Reason |
|---|---|---|
| Fixed cosine thresholds for contradiction/supersession/related | Reject as a decision rule | Similarity is topical proximity, not logical opposition or temporal replacement. The current held-out legacy baseline is already precision `0.0`; adding fuzzy auto-actions would amplify false positives. |
| Delta-aware `assemble_context` | Separate product experiment | It introduces caller/session state and recovery semantics unrelated to contradiction correctness. Measure repeated-context token waste first; do not couple it to claim reconciliation. |
| Pipe-delimited compact context | Reject | MCP already publishes typed structured JSON and agents rely on its schema. Token savings should come from budgets and optional fields, not a second parser contract. |
| Code-specific ingestion with tree-sitter | Separate capability | `source_type=code` already exists. AST indexing/search is valuable but does not help the claim decision boundary and would add multiple large dependencies. |
| Multi-harness setup wizard | Separate UX item | Useful, but independent of memory correctness and not required for zero-config runtime behavior. |
| Copy an access-boost decay formula or add a `0.01` floor | Reject literal copy; fix consistency only | The current heat guard already changes decay behavior, while a floor below the invalidation threshold changes nothing. This plan makes every fact retraction, including decay, retract derived claims consistently. |
| Embedding compatibility signatures | No work | `embedding_state`, target signatures, resumable `reembed`, and semantic fallback already exist. |
| Hash chains, WASM runtime, MinnsQL, ontology discovery, streaming transports | Reject | They solve different trust, hosting, or real-time products and would violate KISS/YAGNI for this local MCP server. |

## Verified Gaps at the Baseline

- `reconcile()` is implemented and unit-tested but has no production caller.
- `ClaimStore::lease_next_job`, candidate selection, relation commit, backfill selection, and retraction exist but are not orchestrated by a worker.
- `after_fact_persisted()` hard-codes `ep:inline`, drops policy tags, uses the first entity as the subject, creates a completed projection job, and creates no reconciliation jobs.
- `persist_projection()` performs independent writes rather than one transaction.
- `ClaimConfig::from_env()` is unused; `Shadow` currently disables projection even though shadow mode is defined as projection without exposure.
- The outcome enum serializes `consistent`/`contradicts`, while the accepted persisted vocabulary is `duplicate`/`contradiction`/`temporal_ambiguity` plus directional `correction`/`supersession`.
- Projection deduplicates by value payload alone, so distinct schemas or keys with the same value can collapse.
- `RelationV1` treats every key-value line as a relation; `AttributeV1` returns after the first line; structured fields are always empty in production.
- `extract` still calls the fixed-window `detect_contradiction_warnings()` scan, and context/explain have no claim evidence.
- Claim trace events and the six required Prometheus metric families do not exist.
- The current ignored evaluation measures only legacy warnings. Its held-out report is: 2 expected contradictions, 2 predicted warnings, 0 true positives, precision `0.0`, recall `0.0`.

## File Structure

### New production files

- `src/service/claims/structural.rs` — one deterministic parser for JSON scalar leaves, key-value records, Markdown tables, and conservative sentence assertions.
- `src/service/claims/worker.rs` — claim-specific leased projection/reconciliation worker with stable page commits and cooperative cancellation.
- `src/service/claims/backfill.rs` — post-startup discovery of facts lacking the current extractor fingerprint.
- `src/service/claims/telemetry.rs` — bounded metric labels, trace event types, and structural redaction.
- `src/observability.rs` — optional Prometheus recorder/listener installation; no listener without explicit feature and address.
- `migrations/028_claim_reconciliation_hardening.surql` — additive relation lookup/metric fields and indexes.

### New tests and evidence

- `tests/claim_reconciliation_e2e.rs` — projection, job, relation, restart, isolation, and MCP behavior through the real in-memory SurrealDB adapter.
- `tests/claim_migration_upgrade.rs` — old-prefix automatic upgrade and immutable migration checks.
- `tests/prometheus_claim_metrics.rs` — optional exposition and label-cardinality contract.
- `docs/evals/CLAIM_RECONCILIATION.md` — reproducible baseline, final corpus report, latency percentiles, and rollout evidence.

### Existing files modified

- `Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/config.rs`, `src/config/claims.rs`
- `src/models/claim.rs`, `src/models/request.rs`
- `src/service.rs`, `src/service/claims.rs`, `src/service/claims/extract.rs`, `src/service/claims/schema.rs`, `src/service/claims/project.rs`, `src/service/claims/reconcile.rs`
- `src/service/core.rs`, `src/service/core/builder.rs`, `src/service/episode/fact_extraction.rs`, `src/service/context.rs`, `src/service/lifecycle/decay.rs`, `src/service/capabilities/invalidate.rs`
- `src/cli/runtime.rs`
- `src/storage/claims.rs`, `src/storage/migrations.rs`, `src/logging.rs`, `src/tools/response.rs`, `src/tools/extract.rs`
- `tests/common/mod.rs`, `tests/eval_claim_reconciliation.rs`, `tests/tools_e2e.rs`

---

### Task 1: Turn the Existing Corpus Into a Real Current-Engine Gate

**Files:**
- Modify: `tests/eval_claim_reconciliation.rs`
- Create: `tests/claim_reconciliation_e2e.rs`
- Create: `docs/evals/CLAIM_RECONCILIATION.md`
- Test fixture (labels are read-only): `tests/fixtures/evals/claim_reconciliation_cases.json`

**Interfaces:**
- Consumes: public `MemoryService` ingest/extract flow and `Arc<SurrealDbClient>` from `tests/common/mod.rs`.
- Produces: `EvaluationReport` for both `legacy_warning` and `claim_relation`, with split/schema/outcome confusion matrices and latency percentiles.

- [ ] **Step 1: Add a failing current-engine evaluation path without changing fixture labels**

```rust
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
struct EvaluationReport {
    corpus_version: String,
    engine: &'static str,
    split: &'static str,
    total_cases: usize,
    expected_relations: usize,
    predicted_relations: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    precision: f64,
    recall: f64,
    isolation_violations: usize,
    per_schema: std::collections::BTreeMap<String, OutcomeCounts>,
    latency_ms_p50: f64,
    latency_ms_p95: f64,
}

#[derive(Debug, Default, serde::Serialize)]
struct OutcomeCounts {
    expected: usize,
    predicted: usize,
    matched: usize,
}
```

Use `common::make_service_with_client()`, run setup/source ingestion, then query `claim`, `claim_relation`, and `claim_job` in the case namespace. Match by source IDs through `source_episode_id`; never match by human-readable content.

- [ ] **Step 2: Make the ignored eval print both development and held-out reports**

Replace the erroneous development guard with `if dev_summary.total_cases > 0`. Print one JSON line per `(engine, split)`. Keep the held-out labels unread by production code and do not tune thresholds against the held-out split.

- [ ] **Step 3: Add red e2e assertions for the verified gaps**

Add these named tests with direct database assertions:

- `new_fact_eventually_has_projection_and_reconcile_jobs`: ingest one supported fact, drive due jobs until idle, then require exactly one completed projection job and one reconciliation job for every persisted claim.
- `same_value_under_distinct_keys_produces_distinct_claims`: ingest two scalar assertions with the same value and different keys, then require two claim IDs and two distinct comparison-key hashes.
- `reconciliation_never_crosses_scope_project_or_policy`: create otherwise identical slots separated by each isolation boundary in turn, then require zero relations across every boundary.
- `relation_outcomes_use_the_accepted_persisted_vocabulary`: query raw relation records and require every outcome string to be one of `duplicate`, `supersession`, `correction`, `contradiction`, or `temporal_ambiguity`.

- [ ] **Step 4: Run the baseline and record it**

Run:

```bash
rtk cargo test --test claim_reconciliation_e2e -- --nocapture
rtk proxy cargo test --test eval_claim_reconciliation run_claim_reconciliation_evals -- --ignored --exact --nocapture
```

Expected before Tasks 2–5: the e2e tests fail because no reconcile job/relations exist; the legacy held-out line remains precision `0.0`, recall `0.0`; the claim engine predicts zero persisted relations.

- [ ] **Step 5: Document the baseline and commit only test/evidence changes**

```bash
rtk git add tests/eval_claim_reconciliation.rs tests/claim_reconciliation_e2e.rs docs/evals/CLAIM_RECONCILIATION.md
rtk git commit -m "test(claims): measure current reconciliation pipeline"
```

---

### Task 2: Parse Structural Assertions Once and Correct Claim Identity

**Files:**
- Create: `src/service/claims/structural.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/claims/schema.rs`
- Modify: `src/service/claims/extract.rs`
- Modify: `src/service/claims/reconcile.rs`
- Modify: `src/models/claim.rs`
- Test: unit tests in the same modules

**Interfaces:**
- Produces: `StructuralAssertion`, `StructuralValue`, `SubjectCandidate`, `parse_assertions`, and a schema-aware `ProjectionIdentity`.
- Preserves: four generic families `attribute/v1`, `quantity/v1`, `relation/v1`, `commitment/v1`; no domain property catalog.

- [ ] **Step 1: Write parser and identity tests first**

Cover JSON scalar leaves, multiple key-value lines, Markdown tables, currency suffixes, explicit correction/transition language, promise deadlines, ambiguous multi-entity subjects, UTF-8 source spans, and the property:

```rust
proptest::proptest! {
    #[test]
    fn projection_identity_changes_when_schema_or_key_changes(
        key_a in "[a-z]{1,16}",
        key_b in "[a-z]{1,16}",
    ) {
        proptest::prop_assume!(key_a != key_b);
        let a = projection_identity(attribute_schema(), &key_a, "same value");
        let b = projection_identity(attribute_schema(), &key_b, "same value");
        proptest::prop_assert_ne!(a, b);
    }
}
```

- [ ] **Step 2: Add the shared structural representation**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StructuralAssertion {
    pub subject_hint: Option<crate::models::claim::NormalizedText>,
    pub predicate: crate::models::claim::NormalizedText,
    pub value: StructuralValue,
    pub qualifiers: std::collections::BTreeMap<String, String>,
    pub cardinality_evidence: CardinalityEvidence,
    pub valid_from: Option<chrono::DateTime<chrono::Utc>>,
    pub valid_to: Option<chrono::DateTime<chrono::Utc>>,
    pub source_span: std::ops::Range<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StructuralValue {
    Text(crate::models::claim::NormalizedText),
    Number { raw: String, unit: Option<String> },
    EntityRef(crate::models::claim::NormalizedText),
    Boolean(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CardinalityEvidence {
    ExplicitScalar,
    ExplicitCollection,
    Unknown,
}
```

Parsing priority is fixed: valid JSON object scalar leaves, Markdown table rows, key-value records, then conservative schema-specific sentences. Parse once per fact; schemas consume assertions and never re-parse raw content independently.

- [ ] **Step 3: Resolve subjects conservatively**

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SubjectCandidate {
    pub entity_id: String,
    pub names: Vec<crate::models::claim::NormalizedText>,
}

pub(crate) fn resolve_subject<'a>(
    hint: Option<&crate::models::claim::NormalizedText>,
    candidates: &'a [SubjectCandidate],
) -> Result<&'a str, &'static str> {
    match hint {
        Some(hint) => {
            let mut matches = candidates
                .iter()
                .filter(|candidate| candidate.names.iter().any(|name| name == hint));
            match (matches.next(), matches.next()) {
                (Some(only), None) => Ok(only.entity_id.as_str()),
                _ => Err("unresolved_subject"),
            }
        }
        None if candidates.len() == 1 => Ok(candidates[0].entity_id.as_str()),
        None => Err("unresolved_subject"),
    }
}
```

Do not fall back to `entity:unknown` and do not select the first of multiple entities.

- [ ] **Step 4: Make schema classification mutually exclusive and generic**

- Numeric assertions go only to `quantity/v1`; canonicalize scale suffixes (`k`, `m`) and the compiled unit families exercised by the corpus. Currency codes remain distinct units; no FX conversion.
- Validated entity-reference assertions go only to `relation/v1`; arbitrary key-value attributes do not become relations.
- Explicit promise/action grammar goes only to `commitment/v1`.
- Remaining explicit scalar assignments go to `attribute/v1`; unknown sentence attributes stay set-valued unless the source syntax proves scalar cardinality.
- Correction/transition markers become qualifiers; they never directly mutate lifecycle state.

- [ ] **Step 5: Deduplicate by the full projection identity**

```rust
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ProjectionIdentity {
    schema: crate::models::claim::ClaimSchemaRef,
    subject: String,
    comparison_key_hash: crate::models::claim::ComparisonKeyHash,
    qualifier_hash: crate::models::claim::QualifierHash,
    value_hash: crate::models::claim::CanonicalPayloadHash,
}
```

Derive `PartialOrd` and `Ord` for `ClaimSchemaFamily` and `ClaimSchemaRef`, whose fields are already orderable. Retain one draft only when this complete identity repeats. Preserve two drafts with the same value under different keys or schema families.

- [ ] **Step 6: Align persisted outcome vocabulary with the accepted design**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ClaimRelationOutcome {
    Duplicate,
    Supersession,
    Correction,
    Contradiction,
    TemporalAmbiguity,
}
```

Set-valued and disjoint-validity coexistence remains a non-persisted `ReconciliationDecision::Coexist`. Add an exhaustive match test so no future enum variant silently falls into a wildcard.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test service::claims --lib
rtk cargo test models::claim --lib
rtk cargo fmt --all --check
rtk git add src/models/claim.rs src/service/claims.rs src/service/claims/structural.rs src/service/claims/schema.rs src/service/claims/extract.rs src/service/claims/reconcile.rs
rtk git commit -m "fix(claims): derive claims from validated assertions"
```

---

### Task 3: Add Migration 028 and Make Claim Writes Transactional

**Files:**
- Create: `migrations/028_claim_reconciliation_hardening.surql`
- Modify: `src/storage/migrations.rs`
- Modify: `src/storage/claims.rs`
- Modify: `src/models/claim.rs`
- Create: `tests/claim_migration_upgrade.rs`
- Test: unit tests in `src/storage/claims.rs`

**Interfaces:**
- Produces: relation lookup by source fact, schema-labeled relation counts, `persist_projection` atomicity, and `commit_reconciliation_page` atomicity.
- Preserves: existing `DbClient` trait and all migration checksums through `027`.

- [ ] **Step 1: Write failing migration-prefix and rollback tests**

For every supported cut point from migration `006` through `027`, apply the list prefix through that cut point to a fresh in-memory DB, then apply the complete current list and assert the tables, fields, and indexes from `028`. Also assert a stored checksum for `027` still equals the current `migration_checksum(include_str!("../../migrations/027_claim_reconciliation.surql"))`.

- [ ] **Step 2: Add only the additive migration**

```sql
-- Migration 028: harden persisted claim relation lookups and metrics.
DEFINE FIELD schema_family ON claim_relation TYPE option<string>;
DEFINE FIELD schema_version ON claim_relation TYPE option<int>;
DEFINE FIELD left_fact_id ON claim_relation TYPE option<string>;
DEFINE FIELD right_fact_id ON claim_relation TYPE option<string>;

DEFINE INDEX claim_relation_left_fact_active_idx
    ON claim_relation COLUMNS left_fact_id, t_invalid_ingested;
DEFINE INDEX claim_relation_right_fact_active_idx
    ON claim_relation COLUMNS right_fact_id, t_invalid_ingested;
DEFINE INDEX claim_relation_schema_outcome_active_idx
    ON claim_relation COLUMNS schema_family, outcome, t_invalid_ingested;
```

Register `028` after `027`; do not reorder, rename, or edit any earlier entry.

- [ ] **Step 3: Extend `ClaimRelation` with lookup metadata written by new binaries**

Add `schema_ref`, `left_fact_id`, and `right_fact_id`. Deserialize the migration-added fields with explicit compatibility defaults only where old `027` records may lack them; new relation construction must always populate them.

- [ ] **Step 4: Replace interpolated cursors and IDs with bound variables**

Use `type::thing($table, $id)` or bound scalar fields for record lookup. In particular, remove formatted `after_claim_id`/`after_fact_id` clauses and bind `$after`; validate positive page sizes before querying. Make `select_relations_for_facts` filter the new `left_fact_id`/`right_fact_id` fields rather than comparing claim IDs with fact IDs. Make active relation counts run inside the selected namespace and group only by `schema_family` and `outcome`; do not reference a nonexistent relation-level namespace field.

- [ ] **Step 5: Persist projection plus reconcile jobs in one transaction**

Change the request so it contains the projection job and one pending `ClaimJobKind::Reconcile` per projected claim. Execute one bound multi-statement transaction: begin, create claims and jobs, mark projection complete, then commit. On any injected statement failure, assert zero claims and zero reconcile jobs were committed and the projection job remains retryable.

- [ ] **Step 6: Commit relation versions and the job cursor atomically**

```rust
pub(crate) struct CommitReconciliationPageRequest<'a> {
    pub namespace: &'a str,
    pub job_id: &'a crate::models::ClaimJobId,
    pub expected_lease_owner: &'a str,
    pub relations: &'a [crate::models::claim::ClaimRelation],
    pub next_cursor: Option<&'a crate::models::ClaimId>,
    pub completed: bool,
    pub counters: JobCounters,
}
```

The transaction creates or validates deterministic relation versions, updates counters/cursor, and completes the job only when the exact-slot cursor is exhausted. A failed relation write must not advance the cursor.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test storage::claims --lib
rtk cargo test --test claim_migration_upgrade -- --nocapture
rtk cargo fmt --all --check
rtk git add migrations/028_claim_reconciliation_hardening.surql src/storage/migrations.rs src/storage/claims.rs src/models/claim.rs tests/claim_migration_upgrade.rs
rtk git commit -m "fix(storage): harden claim transactions and upgrades"
```

---

### Task 4: Make Projection Durable and Faithful to the Stored Fact

**Files:**
- Modify: `src/config/claims.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/core/builder.rs`
- Modify: `src/storage/claims.rs`
- Modify: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: `ClaimService::schedule_fact_projection`, `ClaimService::run_projection_job`, and `ProjectionStatus`.
- Preserves: the public `MemoryService::add_fact` result contract `Result<String, MemoryError>` and fact-first durability.

- [ ] **Step 1: Write metadata-fidelity and failure-semantics tests**

Assert the projection uses the actual `source_episode`, `scope`, `project`, `policy_tags`, `t_valid`, entity names/aliases, quote/span, and extractor fingerprint. Inject job/persistence failures and assert the fact remains readable, the job records the error, and a retry can complete idempotently.

- [ ] **Step 2: Load one authoritative projection bundle from storage**

```rust
#[derive(Debug, Clone)]
pub(crate) struct ClaimProjectionSource {
    pub fact_id: crate::models::FactId,
    pub source_episode_id: crate::models::EpisodeId,
    pub content: String,
    pub quote: String,
    pub fact_type: String,
    pub t_valid: chrono::DateTime<chrono::Utc>,
    pub scope: String,
    pub project: Option<String>,
    pub policy_tags: Vec<String>,
    pub subjects: Vec<crate::service::claims::structural::SubjectCandidate>,
}
```

`ClaimStore::load_projection_source` reads the fact, source episode, and linked entities. Delete the `ep:inline`, empty-policy, empty-structured-fields, and first-entity fallbacks.

- [ ] **Step 3: Separate rollout capabilities instead of one `is_enabled` boolean**

```rust
impl ClaimRolloutStage {
    pub(crate) const fn projects(self) -> bool { !matches!(self, Self::Disabled) }
    pub(crate) const fn evaluates_relations(self) -> bool { !matches!(self, Self::Disabled) }
    pub(crate) const fn persists_relations(self) -> bool {
        matches!(self, Self::Relations | Self::Evidence)
    }
    pub(crate) const fn exposes_evidence(self) -> bool { matches!(self, Self::Evidence) }
}
```

Shadow projects, evaluates relation decisions, and records metrics, but neither persists nor exposes relations. `relations` persists decisions without exposing them; `evidence` also exposes authorized metadata. Change the interim `Default` to `Shadow`; Task 8 may promote it to `Evidence` only with passing held-out evidence. Parsing `lifecycle` returns `MemoryError::ConfigInvalid` with guidance that automatic lifecycle effects are not shipped.

- [ ] **Step 4: Wire `ClaimConfig::from_env()` only in the production constructor**

After service construction via `MemoryService::new_with_embedding_provider` and before claim backfill scheduling:

```rust
service.claim_service = service
    .claim_service
    .clone()
    .with_config(crate::config::claims::ClaimConfig::from_env()?);
```

Unit/integration constructors retain deterministic explicit defaults and do not read process environment.

- [ ] **Step 5: Replace fire-and-forget projection with durable scheduling**

After fact commit, synchronously ensure the deterministic pending projection job. Then attempt bounded inline processing; errors update the durable job and return a non-fatal `ProjectionStatus::Failed` to extraction. Do not use a detached `tokio::spawn` as the only copy of work.

- [ ] **Step 6: Persist unsupported input as an explainable completed projection**

When no validated assertion survives, complete the projection job with `skipped > 0` and bounded reason codes such as `unsupported_structure`, `unresolved_subject`, `invalid_value`, or `unknown_unit`; create no claim or reconcile job.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test service::claims::project --lib
rtk cargo test --test claim_reconciliation_e2e projection -- --nocapture
rtk cargo test --test service_integration test_service_ingest_and_extract_flow -- --exact
rtk git add src/config/claims.rs src/service/claims/project.rs src/service/core.rs src/service/core/builder.rs src/storage/claims.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "fix(claims): schedule durable fact projection"
```

---

### Task 5: Run Reconciliation and Historical Backfill Locally

**Files:**
- Create: `src/service/claims/worker.rs`
- Create: `src/service/claims/backfill.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/core/builder.rs`
- Modify: `src/cli/runtime.rs`
- Modify: `src/models/claim.rs`
- Modify: `src/storage/claims.rs`
- Modify: `Cargo.toml`, `Cargo.lock`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: `ClaimWorkerRuntime`, `run_job_page`, `schedule_namespace_backfill`, stable leases/cursors, and graceful shutdown.
- Consumes: pure `reconcile()` and the transactional store methods from Task 3.

- [ ] **Step 1: Add direct `tokio-util` and write restart/lease tests first**

```toml
tokio-util = { version = "0.7.18", features = ["rt"] }
```

Test expired-lease recovery, two-worker contention, cancellation between pages, crash after relation commit but before the next lease, failed-fact cursor pinning, and multi-namespace backfill isolation.

- [ ] **Step 2: Implement a claim-specific runtime, not a generic job framework**

```rust
#[derive(Clone)]
pub(crate) struct ClaimWorkerRuntime {
    shutdown: tokio_util::sync::CancellationToken,
    handles: std::sync::Arc<tokio::sync::Mutex<Vec<tokio::task::JoinHandle<()>>>>,
}

impl ClaimWorkerRuntime {
    pub(crate) async fn shutdown(&self) {
        self.shutdown.cancel();
        let handles = std::mem::take(&mut *self.handles.lock().await);
        for handle in handles {
            let _ = handle.await;
        }
    }
}
```

The mutex guard is dropped before awaiting handles. Test constructors do not spawn workers.

- [ ] **Step 3: Fix lease semantics**

Lease the oldest job whose state is `pending` or whose `leased/running` lease expired. Return the updated record (`RETURN AFTER`), increment retry count only on recovery/failure, verify `lease_owner` on every page commit, and use a bounded retry ceiling with a persisted final `failed` state.

- [ ] **Step 4: Process exact-slot candidate pages**

For each reconcile job: load the owning claim, query only its complete slot fingerprint, order by `claim_id`, skip self, call pure `reconcile`, construct deterministic relation IDs from pair plus context fingerprint, and commit relations with the next cursor atomically. Check cancellation and time budget only between pages.

- [ ] **Step 5: Add one resumable backfill job per namespace and extractor fingerprint**

Add `ClaimJobKind::Backfill`. Page facts by stable `fact_id`; schedule projection only when no completed/current-fingerprint projection exists. Advance the backfill cursor only after the fact has a durable projection job. A failed fact remains the next retry target.

- [ ] **Step 6: Start workers only in long-running modes and stop them on shutdown**

In `new_from_env_with_mode`, order startup as: connect, apply migrations through `028`, embedding preflight, build service, check connection, and ensure one deterministic backfill job per namespace. Do not spawn claim workers in this constructor, so historical scanning cannot begin before the ready service is returned and one-shot CLI commands do not leak background tasks.

In `run_stdio_server` and the feature-enabled branch of `run_watch_mode`, call `memory_service.start_claim_workers()` immediately after `build_memory_service`. Keep the returned `ClaimWorkerRuntime` outside the service moved into MCP/watcher code, and call `runtime.shutdown().await` on both success and error exit paths. A cancellation-aware guard must also cancel the token on early return; tests explicitly await shutdown rather than relying on async work from `Drop`.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test service::claims::worker --lib
rtk cargo test service::claims::backfill --lib
rtk cargo test --test claim_reconciliation_e2e worker -- --nocapture
rtk cargo test --test claim_reconciliation_e2e backfill -- --nocapture
rtk git add Cargo.toml Cargo.lock src/models/claim.rs src/service/claims.rs src/service/claims/project.rs src/service/claims/worker.rs src/service/claims/backfill.rs src/service/core/builder.rs src/cli/runtime.rs src/storage/claims.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): reconcile and backfill durable jobs"
```

---

### Task 6: Replace Legacy Warnings and Expose Authorized Evidence

**Files:**
- Modify: `src/models/request.rs`
- Modify: `src/service/episode/fact_extraction.rs`
- Modify: `src/service/context.rs`
- Modify: `src/service/core.rs`
- Modify: `src/storage/claims.rs`
- Modify: `src/tools/extract.rs`
- Modify: `src/tools/response.rs`
- Modify: `tests/tools_e2e.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: optional `ReconciliationSummary` and `ClaimReconciliationMetadata` while preserving all old required fields.
- Removes as decision source: fixed-window `detect_contradiction_warnings()` after the evidence-stage gates pass.

- [ ] **Step 1: Add schema compatibility and authorization tests first**

Assert old JSON without reconciliation fields still deserializes; new empty metadata is omitted; cross-policy counterpart data is entirely absent; pending work returns partial guidance; and contradiction output never silently selects one side as resolved truth.

- [ ] **Step 2: Add optional public metadata**

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReconciliationSummary {
    pub status: ReconciliationStatus,
    pub claims_projected: usize,
    pub active_relations: usize,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus {
    Complete,
    Pending,
    Partial,
    Failed,
    #[default]
    Unsupported,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimReconciliationMetadata {
    #[serde(default)]
    pub claim_ids: Vec<String>,
    #[serde(default)]
    pub relations: Vec<ClaimRelationSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimRelationSummary {
    pub relation_id: String,
    pub outcome: ClaimRelationOutcome,
    pub counterpart_fact_id: String,
    pub counterpart_source_episode_id: String,
    pub reason_code: String,
    pub evaluator_version: String,
}
```

Add `Option<ReconciliationSummary>` to `ExtractResult` and `Option<ClaimReconciliationMetadata>` to `AssembledContextItem` and `ExplainItem` with backward-compatible serde attributes.

- [ ] **Step 3: Build `extract.warnings` from active persisted contradictions**

Query relations for the facts returned by extraction. Preserve the existing `ContradictionWarning` shape by loading both authorized source facts and mapping `outcome=contradiction` to the old fields. If projection/reconciliation is pending or failed, keep facts and return `ToolResponse::partial_with_guidance` from production code.

- [ ] **Step 4: Enrich context after cache lookup**

Keep the base retrieval cache free of relation metadata. After both cache hit and miss, query active relations by `left_fact_id/right_fact_id`, apply `as_of`, and authorize both source facts with `context::filtering::fact_record_allowed`. If the counterpart is authorized but outside the budget, include a compact counterpart fact/source handle in metadata; if unauthorized, omit the relation itself.

- [ ] **Step 5: Enrich `explain` with both authorized sources and decision evidence**

Return reason code, evaluator version, temporal/cardinality/source evidence, and snippets for both sides. Preserve the existing explanation when a legacy fact has no claim projection.

- [ ] **Step 6: Retire the fixed-window scan only after the e2e comparison passes**

Delete `detect_contradiction_warnings()` and `has_meaningful_entity_overlap()` as decision code. Keep triple records readable. Do not remove tables, old facts, response fields, or historical migration files.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test --test tools_e2e -- --nocapture
rtk cargo test --test claim_reconciliation_e2e public_contract -- --nocapture
rtk cargo test --test explain_provenance -- --nocapture
rtk git add src/models/request.rs src/service/episode/fact_extraction.rs src/service/context.rs src/service/core.rs src/storage/claims.rs src/tools/extract.rs src/tools/response.rs tests/tools_e2e.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): expose persisted reconciliation evidence"
```

---

### Task 7: Retract Claims Whenever Source Evidence Is Retracted

**Files:**
- Modify: `src/service/capabilities/invalidate.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/lifecycle/decay.rs`
- Modify: `src/storage/claims.rs`
- Test: `tests/claim_reconciliation_e2e.rs`
- Test: `tests/lifecycle_decay.rs`
- Test: `tests/embedded_invalidate.rs`

**Interfaces:**
- Produces: one transaction-valid source-retraction path for manual invalidation and confidence decay.
- Explicitly does not produce: automatic fact invalidation from contradiction, correction, or supersession.

- [ ] **Step 1: Write consistency tests first**

Assert manual invalidation and decay both close the fact plus every derived claim at the same transaction timestamp, close active relations involving those claims, preserve all records for `as_of` history, and invalidate the affected context cache. Assert contradiction alone changes none of those validity fields.

- [ ] **Step 2: Replace the two independent retraction updates with one transaction**

```rust
pub(crate) struct RetractSourceFactRequest<'a> {
    pub namespace: &'a str,
    pub fact_id: &'a crate::models::FactId,
    pub reason: &'a str,
    pub valid_at: chrono::DateTime<chrono::Utc>,
    pub ingested_at: chrono::DateTime<chrono::Utc>,
}
```

The transaction updates fact validity/reason, closes derived claims, and closes active relation transaction intervals. It never deletes records.

- [ ] **Step 3: Centralize manual and decay callers on that operation**

Keep authorization/rate-limit lookup in `InvalidateCapability`; delegate the mutation to `ClaimService::retract_source_fact`. In `run_decay_pass`, replace the direct `db_client.update` with the same service operation using reason code `confidence_decay`. Do not add the review's `0.01` floor.

- [ ] **Step 4: Verify historical reads and commit**

```bash
rtk cargo test --test embedded_invalidate -- --nocapture
rtk cargo test --test lifecycle_decay -- --nocapture
rtk cargo test --test claim_reconciliation_e2e retraction -- --nocapture
rtk git add src/service/capabilities/invalidate.rs src/service/claims/project.rs src/service/lifecycle/decay.rs src/storage/claims.rs tests/claim_reconciliation_e2e.rs tests/lifecycle_decay.rs tests/embedded_invalidate.rs
rtk git commit -m "fix(claims): retract derived claims with source facts"
```

---

### Task 8: Add Trace/Prometheus Diagnostics and Pass Rollout Gates

**Files:**
- Create: `src/service/claims/telemetry.rs`
- Create: `src/observability.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/logging.rs`
- Modify: `src/lib.rs`
- Modify: `src/service/core/builder.rs`
- Modify: `Cargo.toml`, `Cargo.lock`
- Create: `tests/prometheus_claim_metrics.rs`
- Modify: `tests/eval_claim_reconciliation.rs`
- Modify: `docs/evals/CLAIM_RECONCILIATION.md`

**Interfaces:**
- Produces the six design metric families and untruncated, structurally redacted claim trace events.
- Preserves zero-config behavior: no recorder/listener is required and no socket opens by default.

- [ ] **Step 1: Add dependencies and an additive exporter feature**

```toml
[dependencies]
metrics = "0.24.6"
metrics-exporter-prometheus = { version = "0.18.3", optional = true, default-features = false, features = ["http-listener"] }

[features]
prometheus = ["dep:metrics-exporter-prometheus"]
```

- [ ] **Step 2: Define bounded labels and exact metric names**

Emit:

- `memory_claim_pipeline_total{stage,schema,outcome,reason_code}`
- `memory_claim_pipeline_duration_seconds{stage,schema,outcome}`
- `memory_claim_candidate_count{schema,match_mode}`
- `memory_claim_relations_active{schema,outcome}`
- `memory_claim_backfill_facts_total{outcome,reason_code}`
- `memory_claim_backfill_lag`

Use counters for `pipeline_total` and `backfill_facts_total`, histograms for duration and candidate count, and gauges for active relations and backfill lag. Define backfill lag as the age in seconds of the oldest known fact lacking the current extractor fingerprint, or zero when none remains. Map schema to `attribute`, `quantity`, `relation`, `commitment`, or `other`; map every outcome/reason through exhaustive enums. Add a test that rejects any forbidden label key such as `namespace`, `project`, `subject`, `comparison_key`, `fact_id`, `claim_id`, `relation_id`, or `job_id`.

- [ ] **Step 3: Add a safe full trace event path**

```rust
#[derive(Debug, serde::Serialize)]
pub(crate) struct ClaimSchemaTrace<'a> {
    pub family: &'static str,
    pub version: u16,
    pub cardinality: &'static str,
    pub extractor_fingerprint: &'a str,
}

#[derive(Debug, serde::Serialize)]
pub(crate) struct ClaimTraceEvent<'a> {
    pub stage: &'static str,
    pub schema: ClaimSchemaTrace<'a>,
    pub outcome: &'static str,
    pub reason_code: &'static str,
    pub request_id: Option<&'a str>,
    pub job_id: Option<&'a str>,
    pub fact_id: Option<&'a str>,
    pub claim_id: Option<&'a str>,
    pub relation_id: Option<&'a str>,
    pub comparison_key: serde_json::Value,
    pub qualifier_hash: Option<&'a str>,
    pub cursor: Option<&'a str>,
    pub candidate_count: usize,
    pub duration_micros: u64,
}
```

Build `ClaimSchemaTrace` from the selected schema reference, effective cardinality policy, and extractor fingerprint. Before serialization, replace every comparison-key leaf value with a stable SHA-256 token while preserving object shape. Add `StdoutLogger::log_json_trace` that emits this already-redacted JSON without the generic 200-character formatter and only when level `trace` is enabled. Never log raw claim values, episode content, quotes, policy tags, aliases, email addresses, or names.

- [ ] **Step 4: Install Prometheus only when explicitly requested**

Without the `prometheus` feature, metrics remain no-op. With the feature, require a valid `MEMORY_PROMETHEUS_LISTEN_ADDR`; absence means no listener, invalid values fail startup clearly, and `127.0.0.1:0` is supported in tests.

- [ ] **Step 5: Run release gates on the held-out corpus**

Required results:

- zero cross-namespace/scope/project/policy relations;
- claim extraction precision at least `0.98` on supported cases;
- contradiction precision at least `0.95`;
- zero false-positive automatic lifecycle effects (the feature remains disabled);
- exact-slot candidate recall `1.00`;
- backfill restart/idempotency/bounded-memory tests pass;
- no regression beyond existing ingest and `assemble_context` latency gates.

If a gate fails, keep default rollout at `shadow`, record the failing confusion matrix, and do not weaken slot, temporal, source, or authorization gates. Promote the default to `evidence` only in the same commit that records passing held-out evidence.

- [ ] **Step 6: Run the complete quality gate**

```bash
rtk cargo test
rtk cargo test --features prometheus --test prometheus_claim_metrics -- --nocapture
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk proxy cargo test --test eval_claim_reconciliation run_claim_reconciliation_evals -- --ignored --exact --nocapture
```

Expected: zero failures, zero clippy warnings, zero format drift, and documented corpus/latency results matching the release gates.

- [ ] **Step 7: Commit observability and final rollout evidence**

```bash
rtk git add Cargo.toml Cargo.lock src/lib.rs src/observability.rs src/logging.rs src/service/claims.rs src/service/claims/telemetry.rs src/service/core/builder.rs tests/prometheus_claim_metrics.rs tests/eval_claim_reconciliation.rs docs/evals/CLAIM_RECONCILIATION.md
rtk git commit -m "feat(claims): expose reconciliation telemetry"
```

---

## Acceptance Summary

- The current 42-case corpus evaluates the actual persisted claim engine, not only legacy warnings.
- Structural input is parsed once and classified without a tiny hard-coded business-property catalog.
- Claim identity includes schema, subject, comparison key, qualifiers, and value; distinct claims cannot collapse merely because values match.
- Claims use the stored fact/episode/project/policy/entity metadata; no `ep:inline`, `entity:unknown`, or first-entity fallback remains.
- Migration `027` is unchanged; `028` auto-applies to old databases; historical facts backfill after readiness and resume safely.
- Projection creates claims plus reconciliation jobs atomically; reconciliation commits relation versions plus cursor progress atomically.
- Workers use bounded pages, expiring leases, deterministic IDs, cooperative cancellation, and no lock across `.await`.
- `extract`, `assemble_context`, and `explain` preserve old required fields and expose only authorized optional reconciliation evidence.
- Fixed-window warnings are retired only after held-out gates pass; legacy facts/triples/migrations remain readable.
- Manual invalidation and decay retract derived claims consistently; contradiction never invalidates evidence; automatic correction/supersession remains disabled.
- Trace mode exposes complete structurally redacted `ClaimSchema`/`ComparisonKey` diagnostics; Prometheus exposes exactly six bounded-label metric families and opens no default listener.
- Cosine thresholds, delta-aware sessions, compact non-JSON output, tree-sitter ingestion, setup wizard, and unrelated database/runtime features are not coupled to this implementation.
