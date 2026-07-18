# Claim Reconciliation and Contradiction Detection Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace fixed-window string-difference warnings and singleton-triple invalidation with a local, deterministic, high-precision claim reconciliation subsystem that preserves evidence, explains every decision, upgrades old databases automatically, and requires no LLM, remote service, or runtime schema configuration.

**Architecture:** Persist immutable facts first, derive zero or more typed claims through a compiled schema registry, and atomically persist each successful claim projection with durable reconciliation jobs. Select candidates only from an indexed exact claim slot, evaluate pairs in a pure decision engine, and persist versioned relations plus any separately authorized claim-lifecycle transition in one SurrealDB transaction. Keep MCP handlers thin and preserve existing tool shapes by adding only optional metadata. Run projection, reconciliation, and backfill locally with deterministic IDs, expiring leases, stable cursors, bounded pages, and cancellation-aware workers.

**Tech Stack:** Rust 2024, Tokio 1.52, `async-trait`, Serde/Schemars, Chrono, SHA-256, SurrealDB 3.2 embedded/remote adapters, existing structured logger, `metrics` 0.24.6, optional `metrics-exporter-prometheus` 0.18.3, `tokio-util` 0.7.18, and `proptest` 1.11.0 for property tests.

**Design Source:** `docs/CONTRADICTION_DETECTION_DESIGN.md`, `docs/MEMORY_SYSTEM_SPEC.md` version 2.4, and ADRs `docs/adr/0002` through `docs/adr/0015`. If implementation pressure conflicts with an invariant there, stop and revise the design/ADR explicitly rather than weakening the invariant inside code.

## Global Constraints

- Never edit an existing file in `migrations/`. Add only `migrations/027_claim_reconciliation.surql` and append it to `versioned_migrations()`.
- A binary upgrade must start against both a fresh database and every historical migration prefix currently supported by this repository. Schema migration runs during startup; historical fact projection runs only after startup and is resumable.
- Facts remain immutable provenance-bearing evidence. A contradiction never invalidates a fact or claim. Whole-fact invalidation remains an explicit source retraction.
- Supersession may close only real-world claim validity. Correction may close only transaction validity of an erroneous derived claim. Both require explicit evidence and a lineage/authority gate.
- The default implementation is in-process, deterministic, and zero-configuration. It must not call an LLM, download a model, or require a remote service.
- Unsupported or ambiguous facts remain retrievable. They produce a bounded skip reason and no automatically comparable claim.
- Do not add a public MCP tool. Keep `ingest`, `extract`, `resolve`, `invalidate`, `assemble_context`, and `explain` names and required fields unchanged.
- All response additions use `#[serde(default, skip_serializing_if = "Option::is_none")]` and have MCP schema/e2e tests proving old payloads still deserialize.
- Automatic comparison never crosses namespace, scope, project identity including no project, access-policy fingerprint, canonical subject, compatible schema family, comparison-key hash, or qualifier hash.
- Candidate lookup is an indexed, stable, exact-slot page. It must never scan all facts/claims or use a fixed latest-N window.
- Unknown comparison keys are set-valued unless a built-in schema policy explicitly proves single-valued cardinality or mutual exclusion. Recency, confidence, fuzzy similarity, and ingestion order never select a winner.
- Prefer borrowed inputs (`&str`, slices, references) and owned outputs. Clone only at storage, task, or async ownership boundaries.
- Use validated newtypes and enums for IDs, schema references, comparison keys, hashes, job states, outcomes, stages, and reason codes. Do not pass domain states as free-form strings.
- Use `MemoryError`/`thiserror` and `Result`; no production `unwrap`, `expect`, or `panic`. Parsing constructs valid types instead of constructing an invalid value and validating it later.
- No mutex or RwLock guard may live across `.await`. Worker queues/pages are bounded, and worker shutdown uses `CancellationToken` with `tokio::select!`.
- Trace logs may contain full IDs and a full structurally redacted comparison key only at `trace`. Prometheus labels are bounded enums and never contain namespace, project, subject, comparison key, entity, fact, claim, relation, or job IDs.
- `main.rs` stays thin. Claim business rules live under `src/service/claims/`; SurrealDB queries, transactions, and leases live under `src/storage/claims.rs`.
- Keep `DbClient` backward compatible. Add a narrow `ClaimStore` adapter around the existing `Arc<dyn DbClient>` instead of adding required methods to the large trait and every test double.
- Before each commit run the focused tests named by the task. Before handoff run the complete default and optional-Prometheus quality gates with zero warnings, failures, or format drift.

---

## Resolved Design Decisions

| Decision | Implementation consequence |
|---|---|
| Claims are derived artifacts, not replacements for facts | Fact writes succeed independently; claim failures become visible durable work and never roll back evidence. |
| Four structural schemas, no property catalog | `attribute/v1`, `quantity/v1`, `relation/v1`, and `commitment/v1` accept canonical structural components rather than paths such as `company.arr`. |
| Exact arithmetic without a new decimal runtime dependency | `CanonicalDecimal` stores normalized `i128` coefficient plus scale; unit conversions use checked rational factors. |
| One feature-oriented service boundary | `ClaimService` owns projection/reconciliation orchestration; pure extraction, normalization, and decisions remain submodules. |
| Narrow storage capability | `SurrealClaimStore` wraps `Arc<dyn DbClient>` and implements `ClaimStore`; `DbClient` remains unchanged. |
| Two durable job kinds | `project_fact` survives extraction failures; successful projection atomically creates claims and per-claim `reconcile_claim` jobs. A namespace `backfill` job discovers missing/current-fingerprint projections. |
| Safe zero-config rollout | Default stage is `evidence`: projection, relation persistence, warnings, retrieval metadata, and explain evidence are active. Automatic correction/supersession effects require `MEMORY_CLAIM_ROLLOUT_STAGE=lifecycle` until production evidence justifies changing the default in a later release. |
| No default metrics listener | The `metrics` facade is always present and no-op without a recorder. `--features prometheus` plus `MEMORY_PROMETHEUS_LISTEN_ADDR` installs the scrape endpoint; absence of the variable opens no socket. |
| Full but safe trace diagnostics | Trace JSON bypasses the human logger's 200-character truncation after sensitive comparison-key leaves have been replaced with stable SHA-256 tokens. Structure, hashes, IDs, cursors, fingerprints, and reason codes remain complete. |
| Legacy removal is gated | Fixed-window warnings and singleton triple invalidation are removed only after claim evaluation and handler e2e gates pass. Triple records remain readable and may still support retrieval. |

## File Structure

### New production files

- `migrations/027_claim_reconciliation.surql` — additive claim, relation, policy, alias, job, and indexes schema.
- `src/models/claim.rs` — claim value objects, typed values, relation and public reconciliation metadata.
- `src/service/claims.rs` — feature boundary and `ClaimService` facade.
- `src/service/claims/schema.rs` — compiled registry and structural cardinality/source policies.
- `src/service/claims/extract.rs` — deterministic projection from fact/source records.
- `src/service/claims/normalize.rs` — canonical text, exact decimal/unit normalization, serialization, hashes, and slots.
- `src/service/claims/reconcile.rs` — pure decision table.
- `src/service/claims/project.rs` — post-fact projection and inline reconciliation orchestration.
- `src/service/claims/worker.rs` — lease-based, cancellation-aware local job worker.
- `src/service/claims/backfill.rs` — stable fact cursor and namespace backfill orchestration.
- `src/service/claims/retrieval.rs` — context/explain enrichment and claim-validity projection.
- `src/service/claims/telemetry.rs` — typed trace events and bounded metrics facade.
- `src/storage/claims.rs` — `ClaimStore`, SurrealDB adapter, typed query inputs, and transactions.
- `src/config/claims.rs` — validated rollout stage, page/lease/budget defaults, and optional Prometheus listen address.
- `src/observability.rs` — optional Prometheus recorder/listener installation.

### New evaluation/test files

- `tests/fixtures/evals/claim_reconciliation_cases.json` — versioned labeled corpus with origin and split metadata.
- `tests/eval_claim_reconciliation.rs` — baseline, confusion matrix, coverage, isolation, and latency report.
- `tests/claim_store_integration.rs` — embedded SurrealDB transaction, lease, pagination, idempotency, and upgrade tests.
- `tests/claim_reconciliation_e2e.rs` — service-level projection, warnings, retrieval, explain, and retraction tests.
- `tests/prometheus_claim_metrics.rs` — optional feature exposition and label-cardinality contract.
- `docs/evals/CLAIM_RECONCILIATION.md` — reproducible corpus, baseline, final metrics, and rollout evidence.

### Existing files modified

- `Cargo.toml`, `Cargo.lock`, `Makefile`
- `src/lib.rs`, `src/models.rs`, `src/models/ids.rs`, `src/models/request.rs`
- `src/service.rs`, `src/service/core.rs`, `src/service/core/builder.rs`, `src/service/episode/fact_extraction.rs`, `src/service/error.rs`
- `src/service/context.rs`, `src/tools/extract.rs`, `src/tools/response.rs`, `src/tools/assemble_context.rs`, `src/tools/explain.rs`
- `src/storage.rs`, `src/storage/migrations.rs`, `src/cli/runtime.rs`, `src/config.rs`
- `src/service/triple_extractor.rs`, `src/service/conflict_resolver.rs` (remove only after its last production use is gone)
- `tests/eval_latency.rs`, `tests/service_integration.rs`, `tests/tools_e2e.rs`
- `README.md`, `docs/EVAL_BASELINE.md`, `docs/MEMORY_SYSTEM_SPEC.md`, `.agents/skills/memory-mcp/SKILL.md`

---

### Task 1: Build the Labeled Corpus and Capture the Legacy Baseline

**Files:**
- Create: `tests/fixtures/evals/claim_reconciliation_cases.json`
- Create: `tests/eval_claim_reconciliation.rs`
- Create: `docs/evals/CLAIM_RECONCILIATION.md`
- Modify: `Makefile`

**Interfaces:**
- Consumes: current `extract` results and `ContradictionWarning` records.
- Produces: ignored test `run_claim_reconciliation_evals`, versioned corpus `claim-reconciliation/v1`, and `make eval-claims`.

- [ ] **Step 1: Define the fixture contract and corpus integrity test**

Create the fixture types inside `tests/eval_claim_reconciliation.rs` before production claim types exist:

```rust
#[derive(Debug, serde::Deserialize)]
struct ClaimCase {
    id: String,
    corpus_version: String,
    split: CorpusSplit,
    origin: CorpusOrigin,
    language: String,
    setup: Vec<SourceSample>,
    source: SourceSample,
    expected: ExpectedCase,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CorpusSplit { Development, Test }

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum CorpusOrigin { AnonymizedReal, ExternalPublic, SyntheticAdversarial }

#[derive(Debug, serde::Deserialize)]
struct SourceSample {
    source_type: String,
    source_id: String,
    content: String,
    scope: String,
    project: Option<String>,
    policy_tags: Vec<String>,
    t_ref: String,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedCase {
    claims: Vec<ExpectedClaim>,
    relations: Vec<ExpectedRelation>,
    skip_reason_codes: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedClaim {
    schema: String,
    subject: String,
    comparison_key: std::collections::BTreeMap<String, String>,
    value: serde_json::Value,
    qualifiers: std::collections::BTreeMap<String, String>,
    cardinality: String,
    valid_from: Option<String>,
    valid_to: Option<String>,
    source_span: String,
}

#[derive(Debug, serde::Deserialize)]
struct ExpectedRelation {
    setup_source_id: String,
    source_id: String,
    outcome: String,
    reason_code: String,
    predecessor_source_id: Option<String>,
    successor_source_id: Option<String>,
}
```

Add `claim_fixture_covers_every_schema_outcome_and_isolation_boundary`. It must require:

- all four schemas and all five persisted outcomes;
- duplicate, coexistence, not-comparable, and not-same-slot negative cases;
- English and Russian plus at least one non-Latin additional language sample;
- structured records, tables/key-value text, and free sentences;
- arbitrary real-world dimensions across finance, staffing, delivery, compliance, incidents, decisions, preferences, configuration, commitments, and relations;
- aliases, exact unit conversion, unknown units, missing time, overlapping and disjoint intervals, corrections, and transitions;
- cross-scope, cross-project, cross-policy, unresolved-subject, qualifier mismatch, and set-valued cases;
- development and held-out test splits;
- at least one non-synthetic positive and negative case per schema. Synthetic positives alone must make the integrity test fail.

- [ ] **Step 2: Run the integrity test and verify failure**

```bash
rtk cargo test --test eval_claim_reconciliation claim_fixture_covers_every_schema_outcome_and_isolation_boundary -- --exact
```

Expected: the test fails because the fixture is absent or incomplete.

- [ ] **Step 3: Add the labeled corpus without a closed property list**

Populate `claim_reconciliation_cases.json`. Every expected claim stores structural components (`schema`, subject reference, dynamic comparison-key components, typed value, qualifiers, validity, cardinality), never a pre-enumerated property path. Include source spans so extraction precision can be audited. Keep held-out labels in the same file but make the runner filter by `split` for development iteration.

- [ ] **Step 4: Implement the legacy baseline runner**

Before claim production code exists, let the ignored runner execute setup/source episodes through the current handler path and compare `ContradictionWarning` to expected contradiction relations. Emit one JSON line containing:

```rust
#[derive(serde::Serialize)]
struct LegacyBaselineReport {
    corpus_version: String,
    split: String,
    total_cases: usize,
    expected_contradictions: usize,
    predicted_warnings: usize,
    true_positives: usize,
    false_positives: usize,
    false_negatives: usize,
    precision: f64,
    recall: f64,
    unsupported_schema_cases: usize,
    isolation_violations: usize,
}
```

The runner records baseline evidence; it does not assert the new target thresholds yet.

- [ ] **Step 5: Add the command and capture the baseline**

Add to `Makefile`:

```make
EVAL_CLAIMS = cargo test --test eval_claim_reconciliation run_claim_reconciliation_evals -- --ignored --exact --nocapture --test-threads=$(TEST_THREADS)

.PHONY: eval-claims
eval-claims:
	@$(EVAL_CLAIMS)
```

Run:

```bash
rtk make eval-claims
```

Expected: one machine-readable legacy baseline line. Copy the command, commit hash, corpus version/split/counts, confusion matrix, precision/recall, warning examples, and limitations into `docs/evals/CLAIM_RECONCILIATION.md`. Do not describe warning recall as contradiction-detection accuracy.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --test eval_claim_reconciliation claim_fixture_covers_every_schema_outcome_and_isolation_boundary -- --exact
rtk cargo fmt --all --check
rtk git add Makefile tests/eval_claim_reconciliation.rs tests/fixtures/evals/claim_reconciliation_cases.json docs/evals/CLAIM_RECONCILIATION.md
rtk git commit -m "test: baseline claim reconciliation corpus"
```

---

### Task 2: Add Validated Claim Value Objects and Canonicalization

**Files:**
- Create: `src/models/claim.rs`
- Create: `src/service/claims.rs`
- Create: `src/service/claims/normalize.rs`
- Modify: `src/models.rs`
- Modify: `src/models/ids.rs`
- Modify: `src/service.rs`
- Modify: `Cargo.toml`, `Cargo.lock`

**Interfaces:**
- Produces: `ClaimId`, `ClaimRelationId`, `ClaimJobId`, `ClaimSchemaRef`, `ComparisonKey`, `ClaimValue`, `ClaimSlot`, `Claim`, `ClaimRelation`, and durable `ClaimJob` value objects.
- Pure functions: `canonical_decimal`, `normalize_unit`, `canonical_payload`, `claim_id`, `relation_id`, and `policy_fingerprint`.

- [ ] **Step 1: Add ID/newtype and canonicalization tests first**

Write tests for:

- `CanonicalDecimal::parse("00120.5000") == coefficient 1205, scale 1`;
- overflow and malformed decimals return `MemoryError::Validation`;
- NFC/case/whitespace text normalization is idempotent;
- qualifier order and policy-tag order do not change hashes;
- project `None` differs from every `Some(project)` value;
- claim IDs change when schema/extractor/source/canonical payload changes;
- relation ID is symmetric for unordered pairs, while predecessor/successor remain directional fields;
- unknown units parse as typed units but compare as `not_comparable`;
- unresolved subjects cannot construct a comparable `ClaimSlot`.

Add `proptest = "1.11.0"` under `[dev-dependencies]` and property tests:

```rust
proptest::proptest! {
    #[test]
    fn qualifier_hash_is_order_invariant(entries in prop::collection::btree_map("[a-z]{1,12}", "[a-z0-9 ]{0,24}", 0..12)) {
        let forward = entries.iter().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>();
        let reverse = entries.iter().rev().map(|(k, v)| (k.clone(), v.clone())).collect::<Vec<_>>();
        prop_assert_eq!(qualifier_hash(&forward), qualifier_hash(&reverse));
    }
}
```

- [ ] **Step 2: Run focused tests and verify compilation failure**

```bash
rtk cargo test models::claim --lib
rtk cargo test service::claims::normalize --lib
```

Expected: compilation fails because the claim modules and types do not exist.

- [ ] **Step 3: Define typed IDs and domain enums**

Add a second validated-ID macro in `src/models/ids.rs` for `ClaimId`, `ClaimRelationId`, and `ClaimJobId`. Each type keeps its `String` private, accepts only its exact table prefix through `FromStr`/`TryFrom`, uses custom Serde deserialization through that parser, and exposes deterministic digest construction; do not reuse the legacy permissive `From<&str>` implementation. In `src/models/claim.rs`, keep fields private wherever invalid construction is possible:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimSchemaFamily { Attribute, Quantity, Relation, Commitment }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub(crate) struct ClaimSchemaRef {
    family: ClaimSchemaFamily,
    version: std::num::NonZeroU16,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum ClaimValue {
    Boolean(bool),
    Integer(i64),
    Decimal(CanonicalDecimal),
    Text(NormalizedText),
    DateTime(chrono::DateTime<chrono::Utc>),
    Duration(CanonicalDuration),
    Entity(EntityId),
    Quantity { value: CanonicalDecimal, unit: CanonicalUnit },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCardinality { SetValued, SingleValued }
```

Represent `ComparisonKey` as a schema reference plus a `BTreeMap<String, CanonicalAtom>`. Its constructor rejects empty component names, values, and unsupported nesting. Store hashes in `ComparisonKeyHash`, `QualifierHash`, `PolicyFingerprint`, `ExtractorFingerprint`, and `ReconciliationContextFingerprint` newtypes.

- [ ] **Step 4: Implement exact canonicalization**

Implement `CanonicalDecimal` as checked `i128` coefficient plus `u32` scale. Serialize it as its normalized decimal string and deserialize only through `CanonicalDecimal::from_str`, so storage cannot construct a non-normalized value. Normalize negative zero and trailing zeros. Define a small versioned exact unit registry for dimensional families already present in the corpus: percentage/ratio, currency identity only, duration, bytes, distance, mass, and count. Convert only when both units share a family and the rational multiplication fits; currencies never convert without an explicit same currency code.

Canonical serialization must use ordered fields and include a version byte/string. Hash the serialization with the existing `sha2` dependency. Do not use `serde_json::Value::to_string()` over unordered maps as an identity function.

- [ ] **Step 5: Define persisted claim/relation structures**

Add `Claim`, `ClaimDerivation`, `ClaimRelation`, `ClaimRelationEvidence`, `ClaimRelationOutcome`, `ClaimValiditySource`, `ClaimSlot`, `ClaimJob`, `ClaimJobKind`, and `ClaimJobState`. `ClaimSchemaRef` stores `NonZeroU16`; `CanonicalDuration` stores an unsigned seconds value after checked parsing. Keep content fields immutable after creation; only `valid_to` and transaction-lifecycle fields may change through Task 11.

Use borrowed construction inputs and owned persisted outputs:

```rust
pub(crate) struct ClaimBuildInput<'a> {
    pub namespace: &'a str,
    pub source_fact_id: &'a FactId,
    pub source_episode_id: &'a EpisodeId,
    pub scope: &'a str,
    pub project: Option<&'a str>,
    pub policy_tags: &'a [String],
    pub draft: ClaimDraft,
    pub extractor_fingerprint: &'a ExtractorFingerprint,
    pub t_ingested: chrono::DateTime<chrono::Utc>,
}

pub(crate) fn build_claim(input: ClaimBuildInput<'_>) -> Result<Claim, MemoryError>;
```

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test models::claim --lib
rtk cargo test service::claims::normalize --lib
rtk cargo clippy --lib --tests
rtk cargo fmt --all --check
rtk git add Cargo.toml Cargo.lock src/models.rs src/models/ids.rs src/models/claim.rs src/service.rs src/service/claims.rs src/service/claims/normalize.rs
rtk git commit -m "feat(claims): add typed claim domain model"
```

---

### Task 3: Implement the Compiled Structural Schema Registry and Deterministic Extraction

**Files:**
- Create: `src/service/claims/schema.rs`
- Create: `src/service/claims/extract.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/claims/normalize.rs`
- Test: `tests/eval_claim_reconciliation.rs`

**Interfaces:**
- Produces: `ClaimSchemaRegistry::built_in()`, `project_fact`, `ProjectionResult`, and bounded `ClaimSkipReason`.

- [ ] **Step 1: Write schema, positive, and adversarial-negative tests**

For each schema, add unit tests for structured and sentence sources. Include negatives that differ by only one unsafe assumption: missing canonical subject, implicit time, fuzzy alias, unknown unit, ambiguous actor, unbounded free text, multiple possible measures, and a newer value without transition/correction language.

Use this registry contract:

```rust
pub(crate) trait ClaimSchema: Send + Sync {
    fn schema_ref(&self) -> ClaimSchemaRef;
    fn project(
        &self,
        input: &ClaimProjectionInput<'_>,
        output: &mut Vec<ClaimDraft>,
        skips: &mut Vec<ClaimSkip>,
    ) -> Result<(), MemoryError>;
    fn policy(&self, key: &ComparisonKey) -> ClaimPolicy;
}

pub(crate) struct ClaimSchemaRegistry {
    schemas: Vec<Box<dyn ClaimSchema>>,
    extractor_fingerprint: ExtractorFingerprint,
}
```

- [ ] **Step 2: Run focused tests and verify failure**

```bash
rtk cargo test service::claims::schema --lib
rtk cargo test service::claims::extract --lib
```

Expected: compilation fails because the registry and projectors do not exist.

- [ ] **Step 3: Implement ordered deterministic extraction**

Apply sources in this order:

1. typed connector/record fields and JSON-like objects;
2. tables, headings, key-value lines, and action-item syntax;
3. conservative schema-specific sentence patterns;
4. validated legacy triples for `relation/v1` only.

Every emitted draft includes an exact source byte/character span. Deduplicate identical drafts by canonical payload before building IDs. A projector that cannot prove required slots emits a bounded skip reason such as `unresolved_subject`, `missing_comparison_key`, `ambiguous_value`, `invalid_value`, `unknown_unit`, `missing_required_qualifier`, or `unsupported_structure`.

- [ ] **Step 4: Keep comparison keys compositional**

Build keys from captured structural components:

- `attribute/v1`: dynamic dimension phrase plus structural context;
- `quantity/v1`: dynamic measure phrase and unit family, with the value excluded;
- `relation/v1`: normalized relation phrase and object role, with the object value excluded when it is the compared value;
- `commitment/v1`: normalized action/target roles, with deadline/status represented as schema-declared value or qualifier according to the sentence form.

Do not introduce an enum or match table of business properties. `ClaimPolicy` may declare cardinality only when the structural form proves it (explicit “current status”, explicit exclusive outcome, one deadline for the same commitment lineage). Otherwise return `SetValued`.

- [ ] **Step 5: Extend the eval runner to score extraction**

Load every labeled expected claim and report per schema:

- predicted, expected, and matched canonical claims;
- precision, recall, supported-case coverage;
- skip reason distribution;
- source-span match rate;
- corpus origin and split counts.

Assert only deterministic fixture integrity during normal `cargo test`; keep release thresholds in the ignored `run_claim_reconciliation_evals` until the full storage path exists.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test service::claims --lib
rtk cargo test --test eval_claim_reconciliation --no-run
rtk cargo clippy --lib --tests
rtk cargo fmt --all --check
rtk git add src/service/claims.rs src/service/claims/schema.rs src/service/claims/extract.rs src/service/claims/normalize.rs tests/eval_claim_reconciliation.rs
rtk git commit -m "feat(claims): extract structural claims locally"
```

---

### Task 4: Add the Append-Only Schema and Narrow ClaimStore Adapter

**Files:**
- Create: `migrations/027_claim_reconciliation.surql`
- Create: `src/storage/claims.rs`
- Create: `tests/claim_store_integration.rs`
- Modify: `src/storage.rs`
- Modify: `src/storage/migrations.rs`

**Interfaces:**
- Produces: `ClaimStore`, `SurrealClaimStore`, exact-slot queries, atomic projection/relation writes, job lease methods, and backfill pages.
- Leaves: `DbClient` method set unchanged.

- [ ] **Step 1: Write failing migration compatibility tests**

Add migration-prefix tests inside the `src/storage/claims.rs` test module (where crate-private migration scripts are available) and embedded behavior tests in `tests/claim_store_integration.rs`. Prove:

- `027_claim_reconciliation.surql` is the last registered migration and is registered once;
- a fresh in-memory database contains all five new tables and expected indexes;
- for each prefix of the existing migration list, applying the current binary upgrades successfully to 027;
- an old fact with no `project`, claim, job, or relation fields remains readable and retrievable after upgrade;
- running migrations twice preserves the same migration records and data;
- changing no old migration file is required.

Keep any migration-prefix seeding helper under `#[cfg(test)]` in the storage module so no test-only API is exported.

- [ ] **Step 2: Run migration tests and verify failure**

```bash
rtk cargo test --test claim_store_integration migration -- --nocapture
rtk cargo test storage::claims::tests::migration_prefix --lib
```

Expected: tests fail because migration 027 and its tables do not exist.

- [ ] **Step 3: Add the migration without touching prior files**

The migration defines `claim`, `claim_relation`, `claim_job`, `claim_key_alias`, and `claim_policy` as `SCHEMAFULL`. Store structured values/evidence/derivation as `object FLEXIBLE`; store canonical hashes and fingerprints as strings. Add these indexes:

```surql
DEFINE FIELD invalidation_reason ON fact TYPE option<string>;

DEFINE TABLE claim SCHEMAFULL;
DEFINE FIELD claim_id ON claim TYPE string;
DEFINE FIELD namespace ON claim TYPE string;
DEFINE FIELD source_fact_id ON claim TYPE string;
DEFINE FIELD source_episode_id ON claim TYPE string;
DEFINE FIELD scope ON claim TYPE string;
DEFINE FIELD project ON claim TYPE option<string>;
DEFINE FIELD project_identity ON claim TYPE string;
DEFINE FIELD policy_tags ON claim TYPE array;
DEFINE FIELD access_policy_fingerprint ON claim TYPE string;
DEFINE FIELD schema_family ON claim TYPE string;
DEFINE FIELD schema_version ON claim TYPE int;
DEFINE FIELD subject ON claim TYPE object FLEXIBLE;
DEFINE FIELD subject_key ON claim TYPE string;
DEFINE FIELD comparison_key ON claim TYPE object FLEXIBLE;
DEFINE FIELD comparison_key_hash ON claim TYPE string;
DEFINE FIELD qualifiers ON claim TYPE object FLEXIBLE;
DEFINE FIELD qualifier_hash ON claim TYPE string;
DEFINE FIELD slot_fingerprint ON claim TYPE string;
DEFINE FIELD value ON claim TYPE object FLEXIBLE;
DEFINE FIELD cardinality ON claim TYPE string;
DEFINE FIELD observed_at ON claim TYPE datetime;
DEFINE FIELD valid_from ON claim TYPE option<datetime>;
DEFINE FIELD valid_to ON claim TYPE option<datetime>;
DEFINE FIELD validity_source ON claim TYPE string;
DEFINE FIELD source_lineage ON claim TYPE option<string>;
DEFINE FIELD derivation ON claim TYPE object FLEXIBLE;
DEFINE FIELD extractor_fingerprint ON claim TYPE string;
DEFINE FIELD t_ingested ON claim TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim TYPE option<datetime>;

DEFINE TABLE claim_relation SCHEMAFULL;
DEFINE FIELD claim_relation_id ON claim_relation TYPE string;
DEFINE FIELD left_claim_id ON claim_relation TYPE string;
DEFINE FIELD right_claim_id ON claim_relation TYPE string;
DEFINE FIELD pair_fingerprint ON claim_relation TYPE string;
DEFINE FIELD outcome ON claim_relation TYPE string;
DEFINE FIELD predecessor_claim_id ON claim_relation TYPE option<string>;
DEFINE FIELD successor_claim_id ON claim_relation TYPE option<string>;
DEFINE FIELD reason_code ON claim_relation TYPE string;
DEFINE FIELD evidence ON claim_relation TYPE object FLEXIBLE;
DEFINE FIELD evaluator_version ON claim_relation TYPE string;
DEFINE FIELD context_fingerprint ON claim_relation TYPE string;
DEFINE FIELD evaluated_at ON claim_relation TYPE datetime;
DEFINE FIELD supersedes_relation_id ON claim_relation TYPE option<string>;
DEFINE FIELD scope ON claim_relation TYPE string;
DEFINE FIELD project ON claim_relation TYPE option<string>;
DEFINE FIELD policy_tags ON claim_relation TYPE array;
DEFINE FIELD t_ingested ON claim_relation TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_relation TYPE option<datetime>;

DEFINE TABLE claim_job SCHEMAFULL;
DEFINE FIELD job_id ON claim_job TYPE string;
DEFINE FIELD kind ON claim_job TYPE string;
DEFINE FIELD namespace ON claim_job TYPE string;
DEFINE FIELD source_fact_id ON claim_job TYPE option<string>;
DEFINE FIELD claim_id ON claim_job TYPE option<string>;
DEFINE FIELD extractor_fingerprint ON claim_job TYPE string;
DEFINE FIELD evaluator_fingerprint ON claim_job TYPE option<string>;
DEFINE FIELD status ON claim_job TYPE string;
DEFINE FIELD cursor ON claim_job TYPE option<string>;
DEFINE FIELD lease_owner ON claim_job TYPE option<string>;
DEFINE FIELD lease_expires_at ON claim_job TYPE option<datetime>;
DEFINE FIELD processed ON claim_job TYPE int;
DEFINE FIELD succeeded ON claim_job TYPE int;
DEFINE FIELD skipped ON claim_job TYPE int;
DEFINE FIELD failed ON claim_job TYPE int;
DEFINE FIELD retry_count ON claim_job TYPE int;
DEFINE FIELD last_error ON claim_job TYPE option<string>;
DEFINE FIELD created_at ON claim_job TYPE datetime;
DEFINE FIELD started_at ON claim_job TYPE option<datetime>;
DEFINE FIELD updated_at ON claim_job TYPE datetime;
DEFINE FIELD completed_at ON claim_job TYPE option<datetime>;

DEFINE TABLE claim_key_alias SCHEMAFULL;
DEFINE FIELD alias_id ON claim_key_alias TYPE string;
DEFINE FIELD schema_family ON claim_key_alias TYPE string;
DEFINE FIELD canonical_key_hash ON claim_key_alias TYPE string;
DEFINE FIELD alias_key_hash ON claim_key_alias TYPE string;
DEFINE FIELD registry_version ON claim_key_alias TYPE string;
DEFINE FIELD confirmed_by ON claim_key_alias TYPE string;
DEFINE FIELD t_ingested ON claim_key_alias TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_key_alias TYPE option<datetime>;

DEFINE TABLE claim_policy SCHEMAFULL;
DEFINE FIELD policy_id ON claim_policy TYPE string;
DEFINE FIELD schema_family ON claim_policy TYPE string;
DEFINE FIELD schema_version ON claim_policy TYPE int;
DEFINE FIELD policy_fingerprint ON claim_policy TYPE string;
DEFINE FIELD definition ON claim_policy TYPE object FLEXIBLE;
DEFINE FIELD t_ingested ON claim_policy TYPE datetime;
DEFINE FIELD t_invalid_ingested ON claim_policy TYPE option<datetime>;
```

Then add:

```surql
DEFINE INDEX claim_slot_cursor_idx ON claim COLUMNS slot_fingerprint, claim_id;
DEFINE INDEX claim_source_projection_idx ON claim COLUMNS source_fact_id, extractor_fingerprint;
DEFINE INDEX claim_relation_left_active_idx ON claim_relation COLUMNS left_claim_id, t_invalid_ingested;
DEFINE INDEX claim_relation_right_active_idx ON claim_relation COLUMNS right_claim_id, t_invalid_ingested;
DEFINE INDEX claim_relation_context_idx ON claim_relation COLUMNS pair_fingerprint, context_fingerprint, t_invalid_ingested;
DEFINE INDEX claim_job_lease_idx ON claim_job COLUMNS status, lease_expires_at, job_id;
DEFINE INDEX claim_job_fact_idx ON claim_job COLUMNS source_fact_id, extractor_fingerprint, kind;
DEFINE INDEX claim_alias_lookup_idx ON claim_key_alias COLUMNS schema_family, alias_key_hash, t_invalid_ingested;
DEFINE INDEX claim_policy_lookup_idx ON claim_policy COLUMNS schema_family, policy_fingerprint, t_invalid_ingested;
DEFINE INDEX fact_claim_backfill_cursor_idx ON fact COLUMNS fact_id;
```

The `claim` table stores every exact slot component as a separate auditable field plus `slot_fingerprint`. Candidate queries use the indexed fingerprint and then re-check all components in Rust before evaluating, eliminating even a theoretical hash-collision cross-boundary comparison.

- [ ] **Step 4: Define the narrow storage capability**

Add typed request structs and this capability to `src/storage/claims.rs`:

```rust
#[async_trait::async_trait]
pub(crate) trait ClaimStore: Send + Sync {
    async fn load_projection_source(&self, namespace: &str, fact_id: &FactId) -> Result<Option<ClaimProjectionSource>, MemoryError>;
    async fn ensure_projection_job(&self, job: &ClaimJob) -> Result<(), MemoryError>;
    async fn load_job(&self, namespace: &str, job_id: &ClaimJobId) -> Result<Option<ClaimJob>, MemoryError>;
    async fn lease_next_job(&self, request: LeaseJobRequest<'_>) -> Result<Option<ClaimJob>, MemoryError>;
    async fn persist_projection(&self, request: PersistProjectionRequest<'_>) -> Result<(), MemoryError>;
    async fn select_candidates_page(&self, query: ClaimCandidateQuery<'_>) -> Result<Vec<Claim>, MemoryError>;
    async fn commit_relation(&self, request: CommitRelationRequest<'_>) -> Result<(), MemoryError>;
    async fn select_claims_for_facts(&self, query: ClaimsForFactsQuery<'_>) -> Result<Vec<Claim>, MemoryError>;
    async fn select_relations_for_facts(&self, query: RelationsForFactsQuery<'_>) -> Result<Vec<ClaimRelation>, MemoryError>;
    async fn select_source_evidence(&self, query: SourceEvidenceQuery<'_>) -> Result<Vec<SourceEvidenceRecord>, MemoryError>;
    async fn count_active_relations(&self, namespace: &str) -> Result<Vec<ActiveRelationCount>, MemoryError>;
    async fn select_facts_for_backfill(&self, query: BackfillFactQuery<'_>) -> Result<Vec<serde_json::Value>, MemoryError>;
    async fn retract_fact_and_claims(&self, request: RetractFactAndClaimsRequest<'_>) -> Result<(), MemoryError>;
    async fn upsert_compiled_policies(&self, namespace: &str, policies: &[ClaimPolicyRecord]) -> Result<(), MemoryError>;
}
```

`SurrealClaimStore` owns `Arc<dyn DbClient>` and calls only its existing `select_one`/`query` capability. Projection uses one `BEGIN TRANSACTION` block to create-or-validate claims, create per-claim reconcile jobs, and complete the projection job. Relation commit uses one transaction to version the relation and apply an optional authorized lifecycle mutation.

- [ ] **Step 5: Test exact pagination, idempotency, transactions, and leases**

Cover:

- stable `claim_id > after_claim_id` pages with exact slot filtering;
- no result across every isolation boundary;
- rerunning projection does not duplicate claims/jobs;
- canonical pair/context writes leave one active relation;
- rollback leaves neither relation nor lifecycle mutation when either statement fails;
- one lease winner under concurrent claims, expiry recovery, and owner-checked completion;
- no cursor advance after a failed item.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --test claim_store_integration -- --nocapture
rtk cargo test storage::migrations --lib
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add migrations/027_claim_reconciliation.surql src/storage.rs src/storage/migrations.rs src/storage/claims.rs tests/claim_store_integration.rs
rtk git commit -m "feat(storage): persist claim reconciliation state"
```

---

### Task 5: Wire Durable Projection After Fact Persistence

**Files:**
- Create: `src/service/claims/project.rs`
- Create: `src/config/claims.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/config.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/core/builder.rs`
- Modify: `src/service/episode/fact_extraction.rs`
- Modify: `src/service/error.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: `ClaimService::after_fact_persisted`, `ClaimService::run_projection_job`, `FactProjectionSummary`, and `MemoryService::claim_service()`.
- Preserves: public `MemoryService::add_fact(...) -> Result<String, MemoryError>`.

- [ ] **Step 1: Write failure-semantics and idempotency tests first**

Add service tests for:

- a fact is readable when projection-job creation fails;
- a projection-job failure is logged and `add_fact` still returns the fact ID; Task 12 adds the equivalent bounded metric;
- successful projection atomically stores claims and their reconcile jobs;
- unsupported input completes its projection job with `skipped:unsupported_structure` and creates no claims;
- adding the same deterministic fact twice leaves one fact, one projection job per extractor fingerprint, and one claim set;
- an existing legacy fact encountered through `add_fact` is scheduled even when the fact record already exists;
- a deterministic test budget stops before the next candidate page and leaves its cursor pending; do not assert wall-clock milliseconds with sleeps.

Use a recording `ClaimStore` test double rather than adding methods to `MockDbClient`.

- [ ] **Step 2: Run the focused tests and verify failure**

```bash
rtk cargo test --test claim_reconciliation_e2e projection -- --nocapture
rtk cargo test service::claims::project --lib
```

Expected: compilation fails because `ClaimService` and projection orchestration do not exist.

- [ ] **Step 3: Construct ClaimService behind MemoryService**

Add a cloneable feature facade:

```rust
#[derive(Clone)]
pub(crate) struct ClaimService {
    store: std::sync::Arc<dyn crate::storage::ClaimStore>,
    registry: std::sync::Arc<ClaimSchemaRegistry>,
    logger: crate::logging::StdoutLogger,
    config: ClaimConfig,
    context_cache: std::sync::Arc<tokio::sync::RwLock<
        lru::LruCache<crate::service::cache::CacheKey, Vec<crate::models::AssembledContextItem>>,
    >>,
}
```

Create the base zero-config rollout configuration now so subsequent tasks compile independently:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimRolloutStage { Disabled, Shadow, Relations, Evidence, Lifecycle }

impl Default for ClaimRolloutStage {
    fn default() -> Self { Self::Evidence }
}

#[derive(Debug, Clone)]
pub(crate) struct ClaimConfig {
    pub rollout_stage: ClaimRolloutStage,
    pub candidate_page_size: usize,
    pub inline_candidate_limit: usize,
    pub inline_budget: std::time::Duration,
}
```

`ClaimConfig::from_env` parses `MEMORY_CLAIM_ROLLOUT_STAGE` case-insensitively and rejects unknown values. Page size/inline limits use conservative compiled defaults and validated optional overrides; no variable is required.

In `MemoryService::build`, create the cache once, construct `SurrealClaimStore::new(db_client.clone())`, then construct `ClaimService` with the built-in registry and `ClaimConfig::default()`. Store it as `pub(crate) claim_service: ClaimService`. Public constructors retain their existing signatures and deterministic defaults. In `new_from_env_with_mode`, replace the default via `service.claim_service = service.claim_service.with_config(ClaimConfig::from_env()?);` before policy seeding or worker startup, so environment parsing remains explicit and test constructors stay isolated.

- [ ] **Step 4: Enqueue after the fact is committed**

Move no fact fields into the claim transaction. At the end of both the newly-created and already-existing branches of `add_fact`, call:

```rust
match self.claim_service.after_fact_persisted(&namespace, &fact_id).await {
    Ok(_) => {}
    Err(error) => self.claim_service.record_post_fact_failure(
        &namespace,
        &fact_id,
        &error,
    ),
}
```

`after_fact_persisted` performs:

1. ensure deterministic `project_fact` job for `(fact_id, extractor_fingerprint)`;
2. load the exact fact and source episode records;
3. run deterministic extraction;
4. atomically persist projection claims, per-claim reconcile jobs, and projection completion;
5. run complete candidate pages inline only while the configured budget remains;
6. leave an unexhausted reconcile job pending with its stable cursor.

Check the budget only between storage operations/pages. Never cancel an in-flight transaction or advance a cursor for an uncommitted page.

- [ ] **Step 5: Surface a persisted projection summary to extraction**

After `extract_facts`, query summaries for the returned fact IDs rather than changing `ExtractedFact` or `add_fact` signatures. Add `FactProjectionSummary` to the internal `FactExtractionOutcome`; public optional response fields are added in Task 9.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test service::claims::project --lib
rtk cargo test --test claim_reconciliation_e2e projection -- --nocapture
rtk cargo test --test service_integration test_service_ingest_and_extract_flow -- --exact
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add src/config.rs src/config/claims.rs src/service/error.rs src/service.rs src/service/claims.rs src/service/claims/project.rs src/service/core.rs src/service/core/builder.rs src/service/episode/fact_extraction.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): project claims after fact writes"
```

---

### Task 6: Implement the Pure Reconciliation Decision Table

**Files:**
- Create: `src/service/claims/reconcile.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/models/claim.rs`
- Test: `tests/eval_claim_reconciliation.rs`

**Interfaces:**
- Produces: `reconcile`, `ReconciliationInput`, `ReconciliationDecision`, `PersistedRelationDraft`, and bounded `ReconciliationReasonCode`.
- Has no database, clock, logger, network, or service dependency.

- [ ] **Step 1: Encode the full decision table as failing tests**

Create table-driven tests with two normalized claims and explicit policy/time/source evidence for every row:

```rust
pub(crate) enum ReconciliationDecision {
    Persist(PersistedRelationDraft),
    Skip(ReconciliationReasonCode),
    Coexist(ReconciliationReasonCode),
}

pub(crate) struct ReconciliationInput<'a> {
    pub left: &'a Claim,
    pub right: &'a Claim,
    pub policy: &'a ClaimPolicy,
    pub confirmed_aliases: &'a ConfirmedAliasSet,
    pub evaluator_version: &'a EvaluatorVersion,
    pub context_fingerprint: &'a ReconciliationContextFingerprint,
    pub evaluated_at: chrono::DateTime<chrono::Utc>,
}
```

Required cases:

1. isolation/slot mismatch → `Skip(NotSameSlot)`;
2. incompatible types/units → `Skip(NotComparable)`;
3. same proposition and compatible validity → `duplicate`;
4. different set-valued values without exclusivity → `Coexist(SetValued)`;
5. explicit correction plus source gate → `correction`;
6. mutually exclusive values with known overlapping validity → `contradiction`;
7. explicit transition plus single-valued/source gate → `supersession`;
8. potentially exclusive values with insufficient time → `temporal_ambiguity`;
9. known disjoint closed intervals → `Coexist(DisjointValidity)`.

Add explicit negative tests proving no correction/supersession from recency, confidence, ingestion order, observation time, or unconfirmed fuzzy alias.

- [ ] **Step 2: Add decision properties**

Use `proptest` to prove:

- duplicate, contradiction, and temporal ambiguity outcomes are symmetric;
- swapping inputs preserves canonical relation ID;
- swapping directional inputs swaps predecessor/successor but not the canonical pair;
- any changed isolation component can never yield a persisted relation;
- adding unrelated qualifier order changes no decision;
- normalization is idempotent before comparison.

- [ ] **Step 3: Run focused tests and verify failure**

```bash
rtk cargo test service::claims::reconcile --lib
```

Expected: compilation fails because the decision engine does not exist.

- [ ] **Step 4: Implement evaluation in the specified order**

Keep each gate a small pure function (`same_exact_slot`, `values_comparable`, `same_proposition`, `validity_relation`, `source_gate`, `correction_evidence`, `transition_evidence`). Construct `ClaimRelationEvidence` from the exact inputs used. Never consult wall-clock time except the caller-provided `evaluated_at`, which is audit metadata only.

The relation draft contains canonically ordered left/right IDs and only sets predecessor/successor for correction/supersession. It does not mutate claims.

- [ ] **Step 5: Extend the eval runner**

Run all fixture pairs through the pure engine and report a per-outcome confusion matrix, reason-code counts, contradiction precision/recall, supersession/correction false positives, and isolation violations. Normal tests assert exact per-case decisions; the ignored suite enforces aggregate release gates in Task 13.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test service::claims::reconcile --lib
rtk cargo test --test eval_claim_reconciliation --no-run
rtk cargo clippy --lib --tests
rtk cargo fmt --all --check
rtk git add src/models/claim.rs src/service/claims.rs src/service/claims/reconcile.rs tests/eval_claim_reconciliation.rs
rtk git commit -m "feat(claims): reconcile claims with explicit evidence"
```

---

### Task 7: Persist Relations With Leases, Stable Cursors, and Cancellation

**Files:**
- Create: `src/service/claims/worker.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/core/builder.rs`
- Modify: `src/config/claims.rs`
- Modify: `Cargo.toml`, `Cargo.lock`
- Test: `tests/claim_store_integration.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: `ClaimWorkerRuntime`, `spawn_claim_worker`, `run_one_job`, and owner-checked lease completion.
- Adds direct dependency: `tokio-util = { version = "0.7.18", features = ["rt"] }`.

- [ ] **Step 1: Write concurrency/restart tests first**

Cover:

- two workers racing for one job produce one lease owner;
- an expired lease is recoverable;
- a non-owner cannot advance/complete a job;
- crash after relation commit but before cursor update retries idempotently;
- exact-multiple pages fetch the trailing empty page before completion;
- newly concurrent claims are eventually compared because each owns a job;
- canonical pair/context leaves exactly one active relation;
- cancellation stops before the next lease/page and preserves pending work;
- page size and number of active jobs never exceed configured bounds.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test --test claim_store_integration lease -- --nocapture
rtk cargo test --test claim_reconciliation_e2e worker -- --nocapture
```

Expected: compilation fails because the worker runtime does not exist.

- [ ] **Step 3: Implement a single bounded local worker**

Use one worker per process by default; cross-process safety comes from DB leases. Do not add an in-memory unbounded channel. The loop is cancellation-aware:

```rust
loop {
    tokio::select! {
        _ = cancellation.cancelled() => break,
        outcome = claim_service.run_next_leased_job(&worker_id) => {
            match outcome? {
                JobPoll::Worked => continue,
                JobPoll::Idle => {}
            }
        }
    }

    tokio::select! {
        _ = cancellation.cancelled() => break,
        _ = tokio::time::sleep(config.idle_poll_interval) => {}
    }
}
```

Use a `ClaimWorkerGuard` held by `MemoryService`; its final `Drop` calls `cancel()`. The worker owns cloned `ClaimService` dependencies but not a `MemoryService` clone, so the guard can actually reach final drop. Do not hold cache/store locks across awaits.

- [ ] **Step 4: Process one stable candidate page transactionally**

For each candidate:

1. re-check every exact slot field in Rust;
2. call the pure decision engine;
3. persist a relation version and optional lifecycle action atomically;
4. commit the page cursor only after every candidate in that page succeeds;
5. leave cursor unchanged on the first failed candidate and increment bounded retry/error state.

For the same canonical pair and context fingerprint, create-or-validate the deterministic relation ID. When evaluator/schema/alias/policy context changes, transaction-close the prior active relation, set `supersedes_relation_id` on the new version, and keep both audit records. Complete a job only after an empty candidate page. Keep full error text in trace/job record, but expose bounded error categories in metrics.

- [ ] **Step 5: Spawn only after startup migrations**

In `new_from_env_with_mode`, seed/create-or-validate compiled policy records after migrations, construct the service, verify connectivity, then start the claim worker. Test constructors do not spawn a worker; tests call `run_one_job` explicitly for determinism.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --test claim_store_integration lease -- --nocapture
rtk cargo test --test claim_reconciliation_e2e worker -- --nocapture
rtk cargo test service::claims::worker --lib
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add Cargo.toml Cargo.lock src/config/claims.rs src/service/claims.rs src/service/claims/project.rs src/service/claims/worker.rs src/service/core/builder.rs tests/claim_store_integration.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): reconcile through durable leased jobs"
```

---

### Task 8: Backfill Historical Facts Outside Startup Migrations

**Files:**
- Create: `src/service/claims/backfill.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/claims/worker.rs`
- Modify: `src/storage/claims.rs`
- Modify: `src/service/core/builder.rs`
- Test: `tests/claim_store_integration.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: one deterministic namespace backfill job per extractor fingerprint and `run_backfill_page`.
- Reuses: `claim_job` lease/status/counter fields; does not reuse embedding business logic.

- [ ] **Step 1: Write restart, fingerprint, and bounded-memory tests**

Cover:

- startup returns before historical facts are projected;
- one backfill job per namespace/fingerprint is created after readiness;
- stable fact-ID cursor resumes after restart;
- a failed fact is retried and cursor does not advance past it;
- unsupported facts count as deterministic skips and allow progress;
- a changed extractor fingerprint schedules a new pass without deleting old claims;
- old claims are transaction-closed only after the new projection is committed;
- multi-namespace data never crosses stores/cursors;
- resident batch size remains at `CLAIM_BACKFILL_BATCH_SIZE` (default 100) regardless of total fact count;
- completion lag reaches zero only when every namespace cursor is exhausted.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test --test claim_reconciliation_e2e backfill -- --nocapture
```

Expected: tests fail because backfill scheduling and pages do not exist.

- [ ] **Step 3: Implement backfill with the shared job mechanism**

Use `claim_job.kind = backfill` with deterministic ID from namespace plus extractor fingerprint. Fetch facts ordered by `fact_id`, after the persisted cursor, using the index added in migration 027. For every fact, ensure/run its `project_fact` job and update namespace counters only after success/explicit skip.

Do not embed, re-ingest, rewrite, or invalidate facts. Do not run extraction inside the schema migration. Keep the proven reembed concepts—stable cursor, counters, resume, per-namespace state—but no shared embedding-specific record or code path.

- [ ] **Step 4: Seed backfill after service readiness**

After migrations, policy seeding, and connection check, call `ensure_backfill_jobs()` and return the ready service. The detached worker continues after startup. If job seeding fails, startup fails because otherwise historical coverage would silently stall; a failure while processing a fact marks the durable job failed/retryable without stopping the server.

- [ ] **Step 5: Verify and commit**

```bash
rtk cargo test --test claim_reconciliation_e2e backfill -- --nocapture
rtk cargo test --test claim_store_integration backfill -- --nocapture
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add src/service/claims.rs src/service/claims/backfill.rs src/service/claims/worker.rs src/storage/claims.rs src/service/core/builder.rs tests/claim_store_integration.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): backfill legacy facts asynchronously"
```

---

### Task 9: Preserve the Extract Contract and Return Claim-Based Warnings

**Files:**
- Modify: `src/models/request.rs`
- Modify: `src/models/claim.rs`
- Modify: `src/service/episode/fact_extraction.rs`
- Modify: `src/tools/response.rs`
- Modify: `src/tools/extract.rs`
- Modify: `src/mcp/handlers.rs`
- Test: `tests/service_integration.rs`
- Test: `tests/tools_e2e.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Preserves: all existing `ExtractResult` and `ContradictionWarning` fields.
- Adds: optional `ExtractResult.reconciliation: Option<ReconciliationSummary>` and production partial responses.

- [ ] **Step 1: Write JSON/schema compatibility tests first**

Assert:

- the pre-change JSON fixture for `ExtractResult` still deserializes;
- serializing a result with no reconciliation summary is byte-shape compatible except ordering;
- the generated schema retains required `episode_id`, `entities`, `facts`, `links`, and `warnings`;
- `reconciliation` is optional;
- `ContradictionWarning` fields and types are unchanged;
- the real MCP handler returns `status=partial` when facts exist but projection/reconciliation is pending or failed;
- a complete result remains `status=success`.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test extract_tool_response_schema --lib
rtk cargo test --test tools_e2e extract_reconciliation -- --nocapture
```

Expected: tests fail because the optional summary and production partial constructor do not exist.

- [ ] **Step 3: Add only optional public metadata**

Define:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ReconciliationSummary {
    pub status: ReconciliationStatus,
    pub projected_claims: usize,
    pub active_relations: usize,
    pub pending_jobs: usize,
    #[serde(default)]
    pub reason_codes: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationStatus { Complete, Pending, Partial, Failed, #[default] Unsupported }
```

Add this field to `ExtractResult`:

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub reconciliation: Option<ReconciliationSummary>,
```

Do not add claim IDs or comparison keys to the warning shape.

- [ ] **Step 4: Build warnings from persisted active relations**

At rollout stages `evidence` and `lifecycle`, replace the in-memory 500-fact warning source with active `contradiction` relations for the newly extracted fact IDs. Load both authorized fact records and map them into the unchanged warning fields. Deduplicate by relation ID and produce stable ordering by `(new_fact_id, conflicting_fact_id)`.

During `shadow`/`relations`, compute both legacy and claim results only for parity telemetry; return the legacy result until `evidence` is active. Never return a warning when both source fact records are not accessible.

- [ ] **Step 5: Make partial responses available in production**

Remove `#[cfg(test)]` from `ToolResponse::partial_with_guidance`. In both stored and inline branches of `src/tools/extract.rs`, select partial when the summary status is `Pending`, `Partial`, or `Failed`; state that facts were stored and reconciliation will resume locally. Keep `success` for `Complete`, `Unsupported`, and legacy/no-summary results.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --lib mcp::handlers::tests
rtk cargo test --test service_integration contradiction_warning -- --nocapture
rtk cargo test --test tools_e2e
rtk cargo test --test claim_reconciliation_e2e extract -- --nocapture
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add src/models/claim.rs src/models/request.rs src/service/episode/fact_extraction.rs src/tools/response.rs src/tools/extract.rs src/mcp/handlers.rs tests/service_integration.rs tests/tools_e2e.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): expose compatible reconciliation warnings"
```

---

### Task 10: Enrich Context and Explain Without Hiding Contradictions

**Files:**
- Create: `src/service/claims/retrieval.rs`
- Modify: `src/models/claim.rs`
- Modify: `src/models/request.rs`
- Modify: `src/service/context.rs`
- Modify: `src/service/core.rs`
- Modify: `src/tools/assemble_context.rs`
- Modify: `src/tools/explain.rs`
- Modify: `src/mcp/handlers.rs`
- Test: `tests/claim_reconciliation_e2e.rs`
- Test: `tests/tools_e2e.rs`

**Interfaces:**
- Adds optional `reconciliation` metadata to `AssembledContextItem` and `ExplainItem`.
- Preserves legacy no-claim facts and existing required fields.

- [ ] **Step 1: Write retrieval/explain behavior tests first**

Cover:

- a legacy fact with no claims remains retrievable;
- a multi-claim fact is not excluded because one derived claim is superseded;
- current view excludes only claims closed by correction/supersession, not the evidence fact;
- historical `as_of` sees the correct transaction/world-valid relation versions;
- unresolved contradiction includes both accessible facts when budget permits;
- with budget 1, one fact plus compact counterpart evidence handle is returned rather than a silent winner;
- inaccessible counterpart causes the relation metadata to be omitted, not leaked;
- explain returns both source snippets, reason code, temporal/source-policy evidence, and evaluator version;
- old context/explain JSON without metadata still deserializes and schema-required fields remain unchanged.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test --test claim_reconciliation_e2e context -- --nocapture
rtk cargo test --test claim_reconciliation_e2e explain -- --nocapture
```

Expected: tests fail because claim-aware enrichment does not exist.

- [ ] **Step 3: Add compact public evidence handles**

Define optional metadata with no raw comparison keys:

```rust
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimRelationSummary {
    pub relation_id: String,
    pub claim_id: String,
    pub counterpart_fact_id: String,
    pub outcome: String,
    pub reason_code: String,
    pub evaluator_version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ClaimRelationEvidenceSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimRelationEvidenceSummary {
    pub validity_relation: String,
    pub cardinality: String,
    pub source_gate: String,
    pub correction_evidence: bool,
    pub transition_evidence: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimCounterpartEvidence {
    pub fact_id: String,
    pub claim_id: String,
    pub source_episode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quote: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ClaimReconciliationMetadata {
    #[serde(default)]
    pub claim_ids: Vec<String>,
    #[serde(default)]
    pub relations: Vec<ClaimRelationSummary>,
    #[serde(default)]
    pub counterpart_sources: Vec<ClaimCounterpartEvidence>,
}
```

Add `Option<ClaimReconciliationMetadata>` to `AssembledContextItem` and `ExplainItem` with serde defaults/skipping.

- [ ] **Step 4: Enrich after base retrieval and authorization**

Keep the existing ranking pipeline intact. After it selects authorized fact items:

1. batch-load claims and relation summaries for selected fact IDs at `as_of`;
2. re-check scope/project/policy access for both source facts;
3. annotate selected items;
4. append an accessible contradictory counterpart if absent and budget allows;
5. if the budget is full, evict only the lowest-ranked item not participating in the contradiction;
6. if no safe eviction exists, keep the selected fact and attach the compact counterpart handle.

Perform enrichment after a cache hit as well as a cache miss so relation changes cannot leave stale truth selection in cached base results. Cache only base retrieval items or strip reconciliation metadata before inserting into the cache. Invalidate the affected scope cache after a relation/lifecycle commit as a second safety measure, acquiring the write lock only after the DB await completes.

- [ ] **Step 5: Return full explain evidence without leakage**

For each authorized relation in an input item, load both claims/facts/episodes through `ClaimStore`, apply the same access filter to both, and attach exact source snippets plus typed relation evidence. If either side is unauthorized, return the original explanation with no relation existence indicator.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --test claim_reconciliation_e2e context -- --nocapture
rtk cargo test --test claim_reconciliation_e2e explain -- --nocapture
rtk cargo test --test tools_e2e
rtk cargo test --lib mcp::handlers::tests
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add src/models/claim.rs src/models/request.rs src/service/claims/retrieval.rs src/service/context.rs src/service/core.rs src/tools/assemble_context.rs src/tools/explain.rs src/mcp/handlers.rs tests/claim_reconciliation_e2e.rs tests/tools_e2e.rs
rtk git commit -m "feat(claims): explain unresolved claim relations"
```

---

### Task 11: Separate Claim Lifecycle From Fact Retraction

**Files:**
- Modify: `src/service/claims/reconcile.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/claims/retrieval.rs`
- Modify: `src/storage/claims.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service/capabilities/invalidate.rs`
- Modify: `src/config/claims.rs`
- Test: `tests/claim_store_integration.rs`
- Test: `tests/claim_reconciliation_e2e.rs`

**Interfaces:**
- Produces: `ClaimLifecycleAction::{None, CloseWorldValidity, CloseTransactionValidity, RetractDerivedClaims}`.
- Preserves: `invalidate` public arguments and whole-fact behavior.

- [ ] **Step 1: Write lifecycle invariant tests first**

Assert:

- contradiction/duplicate/ambiguity never mutates claim or fact lifecycle;
- supersession closes predecessor `valid_to` only at an explicit successor `valid_from` and never uses observation/ingestion time;
- correction sets predecessor `t_invalid_ingested` and does not rewrite its real-world interval;
- no correction/supersession action is produced without every policy/source/time gate;
- fact invalidation transaction-closes all active derived claims but preserves them, relations, and audit history;
- re-projection with a new extractor fingerprint transaction-closes old projection claims only after the new projection commits;
- relation and lifecycle mutation roll back together;
- default `evidence` stage persists the decision but applies no correction/supersession effect;
- explicit `lifecycle` stage applies only authorized effects.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test --test claim_reconciliation_e2e lifecycle -- --nocapture
rtk cargo test --test claim_store_integration relation_transaction -- --nocapture
```

Expected: tests fail because lifecycle actions are not implemented.

- [ ] **Step 3: Enforce the validated rollout-stage matrix**

Use the `ClaimRolloutStage` parsed since Task 5. Add one table-driven test for every stage: `Disabled` performs no claim work; `Shadow` projects and compares without persisting relations; `Relations` persists relations without public enrichment or lifecycle effects; `Evidence` also exposes warnings/context/explain; `Lifecycle` additionally permits only gated correction/supersession actions. Projection/reconciliation remain zero-config at the `Evidence` default. `Lifecycle` is an explicit operational promotion, not required to detect contradictions.

- [ ] **Step 4: Commit relation and lifecycle together**

Map only `correction` and `supersession` relation drafts to lifecycle actions, only when stage is `Lifecycle`. Execute relation-version close/create plus claim lifecycle update in the existing `ClaimStore::commit_relation` transaction. Retry transaction conflicts through the storage client's existing bounded retry policy; deterministic IDs make replay create-or-validate.

- [ ] **Step 5: Make explicit fact retraction atomic**

Refactor `InvalidateCapability::invalidate` to keep its rate-limit, lookup, scope, and authorization work, then call `ClaimStore::retract_fact_and_claims`. In one transaction, update the fact's `t_invalid`, `t_invalid_ingested`, and optional `invalidation_reason`, and transaction-close all active claims derived from that fact. Add `invalidation_reason` as an optional fact field in migration 027; older records/binaries remain valid. Invalidate the cache only after the transaction succeeds. Do not map ordinary supersession to `invalidate(fact)` and do not leave a fact retracted while its derived claims remain active.

- [ ] **Step 6: Verify and commit**

```bash
rtk cargo test --test claim_reconciliation_e2e lifecycle -- --nocapture
rtk cargo test --test claim_store_integration relation_transaction -- --nocapture
rtk cargo test --test service_integration fact_invalidation -- --nocapture
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk git add src/config/claims.rs src/service/claims/reconcile.rs src/service/claims/project.rs src/service/claims/retrieval.rs src/storage/claims.rs src/service/core.rs src/service/capabilities/invalidate.rs tests/claim_store_integration.rs tests/claim_reconciliation_e2e.rs
rtk git commit -m "feat(claims): separate claim lifecycle from retraction"
```

---

### Task 12: Add Full Trace Diagnostics and Bounded Prometheus Metrics

**Files:**
- Create: `src/service/claims/telemetry.rs`
- Create: `src/observability.rs`
- Create: `tests/prometheus_claim_metrics.rs`
- Modify: `src/logging.rs`
- Modify: `src/config/claims.rs`
- Modify: `src/config.rs`
- Modify: `src/lib.rs`
- Modify: `src/service/claims.rs`
- Modify: `src/service/claims/project.rs`
- Modify: `src/service/claims/worker.rs`
- Modify: `src/service/claims/backfill.rs`
- Modify: `src/cli/runtime.rs`
- Modify: `Cargo.toml`, `Cargo.lock`
- Modify: `README.md`

**Interfaces:**
- Produces exact metric families from the design and a structured trace path not truncated at 200 characters.
- Optional feature: `prometheus = ["dep:metrics-exporter-prometheus"]`.

- [ ] **Step 1: Write trace and metric-contract tests first**

Test that:

- a canonical comparison key longer than 200 characters is fully present in structured trace JSON after leaf redaction;
- no raw source content/value is present;
- trace is emitted only when `LogLevel::Trace` is enabled;
- all six metric families use exact names;
- built-in schemas map to four bounded labels and any extension maps to `other`;
- no forbidden identifier can appear as a label key/value;
- the active relation gauge is rebuilt from persisted state after restart;
- compiling without `prometheus` opens no listener;
- with the feature enabled, a recorder renders all six families in Prometheus text format.

- [ ] **Step 2: Run tests and verify failure**

```bash
rtk cargo test service::claims::telemetry --lib
rtk cargo test --features prometheus --test prometheus_claim_metrics -- --nocapture
```

Expected: compilation fails because telemetry and the feature do not exist.

- [ ] **Step 3: Add dependencies and feature boundaries**

Use:

```toml
metrics = "0.24.6"
metrics-exporter-prometheus = { version = "0.18.3", default-features = false, features = ["http-listener"], optional = true }

[features]
prometheus = ["dep:metrics-exporter-prometheus"]
```

Keep existing `default = []`, `cli-watch`, `mcp-apps`, and `metal` entries unchanged.
The API contract is the official [`metrics` facade documentation](https://docs.rs/metrics/0.24.6/metrics/) and [`PrometheusBuilder`](https://docs.rs/metrics-exporter-prometheus/0.18.3/metrics_exporter_prometheus/struct.PrometheusBuilder.html); `with_http_listener` is available only through the exporter's `http-listener` feature.

- [ ] **Step 4: Implement a typed internal metrics facade**

Define bounded enums for `ClaimMetricStage`, `ClaimMetricSchema`, `ClaimMetricOutcome`, `ClaimMetricReason`, and `ClaimMatchMode`. `ClaimMetricSchema::from_schema` maps unknown versions/families to `other`. The facade emits exactly:

```text
memory_claim_pipeline_total{stage,schema,outcome,reason_code}
memory_claim_pipeline_duration_seconds{stage,schema,outcome}
memory_claim_candidate_count{schema,match_mode}
memory_claim_relations_active{schema,outcome}
memory_claim_backfill_facts_total{outcome,reason_code}
memory_claim_backfill_lag
```

Wrap the facade and existing logger in cloneable `ClaimTelemetry`, then replace `ClaimService.logger` with `ClaimService.telemetry`. Tests inject a recording sink; production uses the `metrics` facade. Do not dynamically turn arbitrary errors into labels. Map them to bounded `validation`, `storage`, `lease`, `retry_exhausted`, or `internal` categories; full messages stay in trace/job state.

- [ ] **Step 5: Add structured, redacted trace output**

Add `StdoutLogger::log_trace_json(&ClaimTraceEvent)` or a generic crate-private equivalent that writes one JSON object to stderr without `value_to_string` truncation. It must first pass the event through a typed redactor:

- full IDs, fingerprints, hashes, cursor, counts, stage, outcome, and timings remain;
- comparison-key shape/component names remain;
- sensitive text/entity/value leaves become deterministic `sha256:<hex>` tokens;
- source content and quotes are never accepted as trace fields.

Emit request/correlation ID, job/claim/fact/relation IDs, extractor/evaluator/context fingerprints, redacted full key, qualifier hash, cursor/count, match mode, outcome/reason, lifecycle action, retry/resume state, stage duration, and total duration.

- [ ] **Step 6: Install Prometheus only when explicitly configured**

Under `#[cfg(feature = "prometheus")]`, parse `MEMORY_PROMETHEUS_LISTEN_ADDR` as `SocketAddr`. When absent, install no recorder/listener and continue with the no-op facade. When present, call:

```rust
metrics_exporter_prometheus::PrometheusBuilder::new()
    .with_http_listener(address)
    .with_recommended_naming(false)
    .install()
    .map_err(|error| MemoryError::ConfigInvalid(
        format!("failed to install Prometheus exporter: {error}")
    ))?;
```

Call this once in `src/cli/runtime.rs` before constructing `MemoryService`. A duplicate recorder or invalid address is a startup error. Document the feature/env pair and the fact that the default binary opens no metrics socket.

- [ ] **Step 7: Verify and commit**

```bash
rtk cargo test service::claims::telemetry --lib
rtk cargo test --features prometheus --test prometheus_claim_metrics -- --nocapture
rtk cargo check
rtk cargo check --features prometheus
rtk cargo clippy --all-targets --features prometheus
rtk cargo fmt --all --check
rtk git add Cargo.toml Cargo.lock README.md src/config.rs src/config/claims.rs src/lib.rs src/logging.rs src/observability.rs src/service/claims.rs src/service/claims/telemetry.rs src/service/claims/project.rs src/service/claims/worker.rs src/service/claims/backfill.rs src/cli/runtime.rs tests/prometheus_claim_metrics.rs
rtk git commit -m "feat(observability): trace and export claim metrics"
```

---

### Task 13: Retire Legacy Decisions and Run the Rollout Gates

**Files:**
- Modify: `src/service/episode/fact_extraction.rs`
- Modify: `src/service/core.rs`
- Modify: `src/service.rs`
- Modify: `src/service/triple_extractor.rs`
- Remove: `src/service/conflict_resolver.rs` after confirming zero references
- Modify: `tests/eval_claim_reconciliation.rs`
- Modify: `tests/eval_latency.rs`
- Modify: `Makefile`
- Modify: `README.md`
- Modify: `docs/EVAL_BASELINE.md`
- Modify: `docs/evals/CLAIM_RECONCILIATION.md`
- Modify: `docs/MEMORY_SYSTEM_SPEC.md`
- Modify: `.agents/skills/memory-mcp/SKILL.md`

**Interfaces:**
- Default source of contradiction warnings becomes active persisted `ClaimRelation` records.
- Triple extraction may remain for retrieval; triple singleton invalidation is removed.

- [ ] **Step 1: Add final legacy-removal and handler tests**

Prove:

- extraction never calls `select_active_facts(namespace, 500)` for contradiction detection;
- a different fact type with the same claim slot can reconcile correctly;
- same fact type/entity overlap with unrelated keys produces no warning;
- triple extraction no longer invalidates a triple because its predicate appears in a hard-coded singleton set;
- old triple records remain readable and triple-assisted retrieval still works;
- the real stdio MCP sequence `ingest → extract → assemble_context → explain` returns optional claim evidence and both sides of a contradiction;
- old database fixture upgrades automatically before the same stdio sequence;
- no new public tool appears in `tools/list`.

- [ ] **Step 2: Run the final eval and enforce release thresholds**

Extend `run_claim_reconciliation_evals` to assert on the held-out split:

- zero cross-scope, cross-project, or access-policy violations;
- zero automatic supersession/correction false positives;
- claim extraction precision at least `0.98` on supported cases;
- contradiction precision at least `0.95`;
- candidate recall `1.00` for comparable claims in the same exact slot;
- every quality report includes corpus version, origin/split counts, per-schema metrics, confusion matrix, reason distribution, recall/coverage, and latency percentiles.

Run:

```bash
rtk make eval-claims
```

Expected: all release assertions pass. If a precision/isolation gate fails, keep the legacy-removal code uncommitted, narrow extraction/policy rules, and rerun; never weaken the threshold or add a latest-write-wins fallback.

- [ ] **Step 3: Remove the fixed-window and singleton decision paths**

Delete `detect_contradiction_warnings` and `has_meaningful_entity_overlap` once no tests depend on them. In `spawn_triple_extraction`, retain validated triple persistence if still used by retrieval but remove the call into `conflict_resolver`. Remove the module/file only after structural reference search reports no uses.

Do not drop the `triple` table, rewrite old triples, edit migration 024, or remove triple retrieval in this change.

- [ ] **Step 4: Run latency and bounded-work gates**

Extend `tests/eval_latency.rs` with claim-stage percentiles and compare against the recorded pre-change baseline. Run:

```bash
TEST_THREADS=1 rtk cargo test --test eval_latency run_latency_evals -- --ignored --exact --nocapture
TEST_THREADS=1 rtk cargo test --test eval_claim_reconciliation run_claim_reconciliation_evals -- --ignored --exact --nocapture
```

Expected: existing ingest and `assemble_context` p95 gates still pass; claim candidate pages stay bounded; backfill multi-namespace restart/idempotency tests pass. Record measurements rather than projecting improvement.

- [ ] **Step 5: Update contracts and operator documentation**

Document:

- deterministic partial coverage and no LLM/service dependency;
- fact/claim/relation lifecycle distinctions;
- four structural schemas and extension requirements;
- rollout stage meanings and `Evidence` default;
- asynchronous backfill/resume behavior;
- optional response fields and `partial` extraction status;
- trace fields/redaction and all Prometheus metric names;
- optional feature/listener setup;
- rollback: set stage `disabled` or `shadow`, stop workers/enrichment, retain additive tables/audit data, and never rewrite old migrations.

Update `.agents/skills/memory-mcp/SKILL.md` without adding a tool: describe optional `extract.reconciliation`, context/explain evidence, and fact invalidation semantics.

- [ ] **Step 6: Run the complete quality gate**

```bash
rtk cargo check
rtk cargo clippy --all-targets
rtk cargo fmt --all --check
rtk cargo test
rtk cargo check --features prometheus
rtk cargo clippy --all-targets --features prometheus
rtk cargo test --features prometheus
rtk cargo check --features mcp-apps
rtk cargo test --features mcp-apps
rtk make eval-claims
TEST_THREADS=1 rtk cargo test --test eval_latency run_latency_evals -- --ignored --exact --nocapture
```

Expected: zero warnings/errors/format drift/failures; every evaluation threshold passes; both feature builds pass.

- [ ] **Step 7: Commit the rollout record**

```bash
rtk git add Makefile README.md docs/EVAL_BASELINE.md docs/evals/CLAIM_RECONCILIATION.md docs/MEMORY_SYSTEM_SPEC.md .agents/skills/memory-mcp/SKILL.md src/service.rs src/service/core.rs src/service/episode/fact_extraction.rs src/service/triple_extractor.rs tests/eval_claim_reconciliation.rs tests/eval_latency.rs tests/tools_e2e.rs
rtk git add -u src/service/conflict_resolver.rs
rtk git commit -m "feat(claims): replace legacy contradiction decisions"
```

---

## Acceptance Summary

- Facts are stored first and remain retrievable when claim projection/reconciliation fails.
- Claim extraction is local, deterministic, partial by design, and built from four structural schemas rather than a small world-property catalog.
- All identity-bearing domain values are validated newtypes; canonical decimal/unit comparison is exact and checked.
- Claim and relation IDs are deterministic; retries, restarts, and concurrent workers are idempotent.
- Successful claim creation and its reconcile jobs are atomic; relation versions and authorized lifecycle effects are atomic.
- Candidate lookup uses exact indexed slots, stable cursors, bounded pages, and zero cross-boundary comparisons.
- Contradiction, supersession, correction, duplicate, ambiguity, and coexistence have distinct tested semantics.
- No automatic correction/supersession occurs from recency, confidence, fuzzy similarity, observation time, or ingestion order.
- Migration 027 is additive; no prior migration changes; old databases auto-upgrade; backfill happens after startup and resumes safely.
- Existing MCP tools and required response fields remain compatible; no new tool is exposed.
- `extract` can return visible partial success after storing facts; `assemble_context` and `explain` expose authorized evidence without silently choosing a contradiction winner.
- Fact invalidation retracts source evidence; ordinary claim supersession/correction does not invalidate a fact.
- Trace mode carries complete IDs, fingerprints, cursors, reason codes, and structurally redacted full keys without the human logger's truncation.
- Prometheus exports the six specified metric families with bounded labels and no tenant/object IDs; the default build opens no listener.
- Workers are cancellation-aware, lease-protected, bounded, and hold no locks across awaits.
- The held-out release corpus meets precision/isolation gates, candidate recall is complete within exact slots, and existing latency gates do not regress.
- The fixed-window warning scan and singleton triple invalidation are gone; legacy facts/triples and database histories remain readable.

## Execution Order and Review Boundaries

Tasks 1–3 establish scientific evidence and pure domain behavior. Tasks 4–8 add persistence and durable local execution without changing public behavior. Tasks 9–10 expose backward-compatible evidence. Task 11 keeps risky lifecycle effects separately gated. Task 12 makes the system diagnosable before rollout. Task 13 removes legacy decisions only after every correctness, isolation, compatibility, and latency gate passes.

Review and merge in that order. Do not combine Tasks 4–8 with public contract changes in one commit, and do not promote `Lifecycle` as the default stage in this implementation plan.
