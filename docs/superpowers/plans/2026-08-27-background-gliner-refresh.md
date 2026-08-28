# Background Classic GLiNER Artifact Refresh Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Start Classic GLiNER MCP transport without any remote model operation, refresh a verified local candidate in the background, and activate that candidate only after successful runtime smoke validation on the next restart.

**Architecture:** Split artifact lifecycle into durable `candidate`, `known_good`, and `incompatible` roles. Classic GLiNER startup performs typed local inspection only; a post-readiness one-shot runtime resolves/downloads a candidate with cancellation-safe staging, while next startup owns runtime construction, smoke validation, promotion, rejection, and rollback. Existing network-enabled `NerArtifactStore::prepare()` remains available and behavior-compatible for Anno ONNX and VAGO LFM2 GLiNER.

**Tech Stack:** Rust, Tokio, `tokio_util::sync::CancellationToken`, reqwest streaming, serde JSON state, rmcp stdio transport, Candle GLiNER runtime, existing integration-test fakes.

**Spec:** `docs/superpowers/specs/2026-08-27-background-gliner-refresh-design.md`

## Global Constraints

- Scope is only `NER_EXTRACTOR=urchade/gliner_multi-v2.1`; Anno, Regex, Anno ONNX, and VAGO LFM2 GLiNER startup behavior must remain unchanged.
- Do not add MCP tools, request fields, dependencies, storage partitions, or `Cargo.toml` changes.
- Keep `main.rs` limited to CLI parsing and mode dispatch; orchestration belongs in `src/cli/runtime.rs`, artifact business logic in `src/service/`.
- Do not hot-swap the active extractor. One process keeps one immutable extractor and fingerprint for its lifetime.
- Do not manufacture `RuntimeRegressionVerified`: only successful Classic GLiNER construction plus smoke inference may promote a candidate to known-good.
- Missing/recoverably corrupt local artifacts degrade only extraction; permission errors and an inaccessible cache remain startup-fatal.
- Model progress remains on stderr/the configured `ModelProgressSink`; MCP stdout remains JSON-RPC-only.
- Production code must not use `unwrap()`.
- Do not modify or stage the pre-existing user change in `Cargo.lock`.
- Add tests before implementation for every behavior change.
- Final lint gate must pass exactly:

```bash
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps --locked -- -D warnings
```

---

## File Structure

- Modify `crates/memory-mcp/src/service/model_artifacts/state.rs`: schema-v2 durable roles and schema-v1 compatibility.
- Modify `crates/memory-mcp/src/service/model_artifacts/manifest.rs`: candidate-safe validation status and typed local inspection/result contracts.
- Modify `crates/memory-mcp/src/service/model_artifacts.rs`: local inspection, candidate acquisition, promotion/rejection, retention, and cleanup guards.
- Modify `crates/memory-mcp/src/service/model_artifacts/download.rs`: cancellation-aware fetch and `.part` RAII cleanup.
- Create `crates/memory-mcp/src/service/model_artifact_refresh.rs`: one-shot Classic GLiNER refresh runtime and structured lifecycle events.
- Modify `crates/memory-mcp/src/service/mod.rs`: register/re-export the refresh runtime as crate-private service infrastructure.
- Modify `crates/memory-mcp/src/service/entity_extraction.rs`: unavailable extractor implementation and fingerprint-preserving constructor.
- Modify `crates/memory-mcp/src/service/entity_extraction/gliner.rs`: local-only startup, candidate validation/promotion, rejection/fallback, unavailable degradation.
- Modify `crates/memory-mcp/src/service/core/builder.rs`: retain the Classic GLiNER refresh configuration needed after service construction.
- Modify `crates/memory-mcp/src/service/core.rs`: expose one method that starts the post-readiness refresh runtime.
- Modify `crates/memory-mcp/src/error.rs`: typed model-not-ready domain error.
- Modify `crates/memory-mcp/src/mcp/error.rs`: stable MCP error data for unavailable Classic GLiNER.
- Modify `crates/memory-mcp/src/cli/runtime.rs`: start refresh only after `.serve()` succeeds and `main.running` is logged; shut it down deterministically.
- Modify `crates/memory-mcp/tests/ner_model_lifecycle.rs`: state, identity, candidate, rollback, cancellation, and legacy-`prepare()` coverage.
- Modify `crates/memory-mcp/tests/ner_progress_channels.rs`: process-level readiness, unavailable error, blocked refresh, and stdout-purity coverage.
- Create `crates/memory-mcp/tests/ner_gliner_real_activation.rs`: ignored real-fixture candidate activation test.
- Create `docs/adr/0035-background-classic-gliner-refresh.md`: decision record.
- Modify `README.md`: first-install, restart activation, and alternatives.

---

### Task 1: Introduce Schema-v2 Artifact Roles with Schema-v1 Compatibility

**Files:**
- Modify: `crates/memory-mcp/src/service/model_artifacts/state.rs`
- Test: `crates/memory-mcp/src/service/model_artifacts/state.rs`

**Interfaces:**
- Produces: `ArtifactRole::{Candidate, KnownGood, Incompatible}`.
- Produces: `RevisionState.role: ArtifactRole` and `PersistedArtifactState::{candidate, known_goods, incompatibility_for}`.
- Preserves: `read_state(path) -> Result<PersistedArtifactState, MemoryError>` and `persist_state(...)` for existing callers.
- Migration rule: schema-v1 records with `incompatible: None` become `KnownGood`; records with `incompatible: Some(_)` become `Incompatible`; every successful write emits schema version 2.

- [ ] **Step 1: Write schema migration and role-selection tests**

Add tests that deserialize literal schema-v1 JSON and verify in-memory schema-v2 semantics:

```rust
#[test]
fn schema_v1_non_incompatible_records_migrate_to_known_good() {
    let dir = TempDir::new().expect("temp dir");
    let path = dir.path().join("state.json");
    std::fs::write(
        &path,
        r#"{
          "schema_version": 1,
          "revisions": [{
            "revision": "old-good",
            "artifact_identity": "abc",
            "validation_status": "runtime_regression_verified",
            "revision_status": "latest",
            "activated_at": 10,
            "incompatible": null
          }]
        }"#,
    )
    .expect("write state");

    let state = read_state(&path).expect("read v1 state");
    assert_eq!(state.schema_version, STATE_SCHEMA_VERSION);
    assert_eq!(state.known_goods().map(|r| r.revision.as_str()).collect::<Vec<_>>(), vec!["old-good"]);
    assert!(state.candidate().is_none());
}

#[test]
fn schema_v2_candidate_is_never_returned_as_known_good() {
    let mut state = PersistedArtifactState::new();
    state.revisions.push(sample_revision("candidate", ArtifactRole::Candidate, 20));
    state.revisions.push(sample_revision("known-good", ArtifactRole::KnownGood, 10));

    assert_eq!(state.candidate().map(|r| r.revision.as_str()), Some("candidate"));
    assert_eq!(state.known_goods().next().map(|r| r.revision.as_str()), Some("known-good"));
}
```

Also cover v1 incompatible migration, unsupported schema version, malformed JSON, schema-v2 round-trip, and persisted writes containing `"schema_version": 2` plus explicit `role`.

- [ ] **Step 2: Run the focused tests and verify failure**

Run:

```bash
cargo test -p memory_mcp service::model_artifacts::state::tests --locked
```

Expected: compile/test failure because `ArtifactRole`, schema-v2 migration, and role selectors do not exist.

- [ ] **Step 3: Implement explicit roles and compatibility deserialization**

Use a private wire representation so unsupported versions and malformed state remain distinguishable later:

```rust
pub const STATE_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactRole {
    Candidate,
    KnownGood,
    Incompatible,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RevisionState {
    pub revision: String,
    pub artifact_identity: String,
    pub validation_status: ValidationStatus,
    pub revision_status: RevisionStatus,
    pub activated_at: i64,
    pub role: ArtifactRole,
    #[serde(default)]
    pub incompatible: Option<IncompatibilityRecord>,
}
```

Implement `read_state` by first parsing a small envelope containing `schema_version`, then parsing `PersistedArtifactStateV1` or v2. Normalize v1 to v2 in memory. Do not rewrite the file merely by reading it. Keep I/O errors as `MemoryError::Storage`; Task 2 will add typed inspection classification above this layer.

- [ ] **Step 4: Run state tests**

Run:

```bash
cargo test -p memory_mcp service::model_artifacts::state::tests --locked
```

Expected: PASS.

- [ ] **Step 5: Commit the state-model change**

```bash
git add crates/memory-mcp/src/service/model_artifacts/state.rs
git commit -m "refactor: distinguish NER artifact lifecycle roles"
```

---

### Task 2: Add Typed, Identity-Verified Local Inspection

**Files:**
- Modify: `crates/memory-mcp/src/service/model_artifacts/manifest.rs`
- Modify: `crates/memory-mcp/src/service/model_artifacts.rs`
- Test: `crates/memory-mcp/tests/ner_model_lifecycle.rs`

**Interfaces:**
- Produces exactly:

```rust
pub enum LocalCheckpointIssue {
    Incomplete { revision: String },
    IdentityMismatch { revision: String },
    MalformedState { summary: String },
    UnsupportedStateVersion { found: u8 },
}

pub struct LocalCheckpointSet {
    pub candidate: Option<PreparedCheckpoint>,
    pub known_good: Option<PreparedCheckpoint>,
    pub issue: Option<LocalCheckpointIssue>,
}

pub fn NerArtifactStore::inspect_local(
    &self,
    spec: &NerArtifactSpec,
) -> Result<LocalCheckpointSet, MemoryError>;
```

- Contract: no resolver, fetcher, lease wait, or remote retry is invoked.
- Contract: missing state returns an empty set; recoverable content/state defects return `Ok(LocalCheckpointSet { issue: Some(...) })`; permission/unreadable-directory errors return `Err(MemoryError::Storage(_))`.
- Contract: returned checkpoint keeps its persisted `validation_status`; candidate state may not contain `RuntimeRegressionVerified`.

- [ ] **Step 1: Add inspection tests with counting collaborators**

Extend the existing fake resolver/fetcher with call counters, then add tests for:

```rust
#[test]
fn inspect_local_empty_store_does_not_call_network_collaborators() {
    let temp = TempDir::new().expect("temp dir");
    let (store, resolver, fetcher) = make_counting_store(&temp);

    let inspected = store.inspect_local(&test_spec()).expect("inspect");

    assert!(inspected.candidate.is_none());
    assert!(inspected.known_good.is_none());
    assert!(inspected.issue.is_none());
    assert_eq!(resolver.calls(), 0);
    assert_eq!(fetcher.calls(), 0);
}
```

Add separate tests for removed file, zero-byte file, replaced bytes with same path, persisted/recomputed identity mismatch, malformed state, unsupported schema version, candidate exclusion from known-good, and permission failure. On Unix, gate the permission test with `#[cfg(unix)]` and restore permissions before `TempDir` cleanup.

- [ ] **Step 2: Run focused lifecycle tests and verify failure**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle inspect_local --locked
```

Expected: compile failure because inspection types/API do not exist.

- [ ] **Step 3: Implement inspection without using `active_revision()`**

Add a state-reading helper that preserves four categories: missing, malformed JSON, unsupported schema, and operational I/O error. For candidate and known-good records, check all required files are regular and non-zero, compute `artifact_identity` in `spawn_blocking` only when called from async paths, and compare it to the persisted identity before constructing `PreparedCheckpoint`.

For synchronous `inspect_local`, perform the bounded local checks synchronously and return only persisted validation status. Select at most one candidate and the newest known-good. If either selected record is recoverably invalid, return no unsafe checkpoint for that role and set `issue`; do not silently select a different unverified record.

- [ ] **Step 4: Run inspection and existing lifecycle tests**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle --locked
```

Expected: PASS, including pre-existing `prepare()` tests.

- [ ] **Step 5: Commit local inspection**

```bash
git add crates/memory-mcp/src/service/model_artifacts/manifest.rs crates/memory-mcp/src/service/model_artifacts.rs crates/memory-mcp/tests/ner_model_lifecycle.rs
git commit -m "feat: inspect local NER checkpoints without network access"
```

---

### Task 3: Add Candidate Acquisition, Promotion, Rejection, and Rollback

**Files:**
- Modify: `crates/memory-mcp/src/service/model_artifacts/manifest.rs`
- Modify: `crates/memory-mcp/src/service/model_artifacts.rs`
- Test: `crates/memory-mcp/tests/ner_model_lifecycle.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CandidateRefreshOutcome {
    UpToDate { revision: String },
    CandidateReady { revision: String },
    SuppressedIncompatible { revision: String },
}

pub async fn refresh_candidate(
    &self,
    spec: &NerArtifactSpec,
    cancellation: CancellationToken,
) -> Result<CandidateRefreshOutcome, MemoryError>;

pub fn promote_candidate(
    &self,
    spec: &NerArtifactSpec,
    revision: &str,
) -> Result<PreparedCheckpoint, MemoryError>;

pub fn reject_candidate(
    &self,
    spec: &NerArtifactSpec,
    revision: &str,
    reason: &str,
) -> Result<Option<PreparedCheckpoint>, MemoryError>;
```

- Preserves: public behavior and signature of `prepare()` for Anno ONNX and VAGO LFM2 GLiNER.
- `refresh_candidate` performs static acquisition/verification only and persists role `Candidate` with a non-runtime-verified validation status.
- `promote_candidate` is called only after the caller has completed smoke inference; it atomically changes the named candidate to `KnownGood`, marks it `RuntimeRegressionVerified`, and retains the prior known-good.
- `reject_candidate` changes the named candidate to `Incompatible`, removes its artifact directory after state persistence, and returns the newest identity-verified known-good if present.

- [ ] **Step 1: Write lifecycle state-machine tests**

Add tests proving:

```rust
#[tokio::test]
async fn refresh_persists_candidate_without_runtime_verified_status() {
    let temp = TempDir::new().expect("temp dir");
    let (store, _, _) = make_store(&temp, Arc::new(FakeResolver::ok("candidate-1")));

    let outcome = store
        .refresh_candidate(&test_spec(), CancellationToken::new())
        .await
        .expect("refresh");
    assert_eq!(outcome, CandidateRefreshOutcome::CandidateReady { revision: "candidate-1".into() });

    let inspected = store.inspect_local(&test_spec()).expect("inspect");
    let candidate = inspected.candidate.expect("candidate");
    assert_ne!(candidate.validation_status, ValidationStatus::RuntimeRegressionVerified);
    assert!(inspected.known_good.is_none());
}
```

Also test `UpToDate` for an already persisted candidate/known-good at HEAD, `SuppressedIncompatible`, candidate promotion preserving previous known-good, candidate rejection returning previous known-good, candidate never returned by ordinary known-good selection, and retention keeping exactly candidate/current known-good/previous known-good/incompatibility metadata required by rollback and suppression.

- [ ] **Step 2: Run candidate tests and verify failure**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle candidate --locked
```

Expected: compile failure because candidate operations do not exist.

- [ ] **Step 3: Implement candidate operations by extracting shared mechanics from `prepare()`**

Extract internal helpers for resolve, lease, download, static verification, atomic revision-directory commit, and retention. Keep `prepare()` as a wrapper preserving its existing activation semantics for non-Classic callers. `refresh_candidate()` must call the shared mechanics but persist `ArtifactRole::Candidate`; it must never call `activate()` or assign `RuntimeRegressionVerified`.

Add a static-only validation enum variant if required by the existing `ValidationStatus` type, for example:

```rust
pub enum ValidationStatus {
    ReleaseParityVerified,
    StaticArtifactVerified,
    RuntimeRegressionVerified,
}
```

Use the repository’s actual existing serde naming convention when updating fixtures. Reject promotion unless the persisted record is role `Candidate`, its revision matches, and identity verification still succeeds.

- [ ] **Step 4: Run all artifact lifecycle tests**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle --locked
```

Expected: PASS. Existing `prepare()` acquisition, offline fallback, companion file, lease, and retention tests remain green.

- [ ] **Step 5: Commit candidate lifecycle**

```bash
git add crates/memory-mcp/src/service/model_artifacts/manifest.rs crates/memory-mcp/src/service/model_artifacts.rs crates/memory-mcp/tests/ner_model_lifecycle.rs
git commit -m "feat: stage NER revisions as restart candidates"
```

---

### Task 4: Make Download and Staging Cancellation-Safe

**Files:**
- Modify: `crates/memory-mcp/src/service/model_artifacts/download.rs`
- Modify: `crates/memory-mcp/src/service/model_artifacts.rs`
- Test: `crates/memory-mcp/src/service/model_artifacts/download.rs`
- Test: `crates/memory-mcp/tests/ner_model_lifecycle.rs`

**Interfaces:**
- Changes internal trait method to accept `&CancellationToken`:

```rust
async fn ArtifactFetcher::fetch(
    &self,
    repository: &str,
    revision: &str,
    requirement: &ArtifactRequirement,
    target: &Path,
    progress: &dyn ModelProgressSink,
    cancellation: &CancellationToken,
) -> Result<(), MemoryError>;
```

- `prepare()` passes a fresh never-cancelled token so legacy behavior is unchanged.
- `refresh_candidate()` passes its runtime token.
- Produces private `PartialFileGuard` and `StagingDirGuard`; each removes its owned path on `Drop` unless `commit()` was called.
- Cancellation is represented as `MemoryError::Transient("NER artifact refresh cancelled".to_string())`; runtime logging classifies this as stopped, not failed.

- [ ] **Step 1: Write cancellation debris tests before changing implementation**

Add deterministic fakes with barriers/notifies for four phases: lease wait, response wait before first chunk, mid-stream after `.part` creation, and between files in multi-file acquisition. For each phase, cancel the token and assert:

```rust
assert_no_entries_with_suffix(store_root.join("gliner/staging"), ".part");
assert_directory_empty(store_root.join("gliner/staging"));
assert_directory_empty(store_root.join("gliner/leases"));
```

The mid-stream fetch test must exercise the real `HfArtifactFetcher` against a local TCP server that sends headers plus one chunk, then blocks; this proves the production chunk loop observes cancellation and the `.part` guard runs.

- [ ] **Step 2: Run cancellation tests and verify failure**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle cancellation --locked
cargo test -p memory_mcp service::model_artifacts::download::tests::cancel --locked
```

Expected: compile/test failure because cancellation is not threaded through fetch/lease/download and dropped futures can leave debris.

- [ ] **Step 3: Implement RAII guards and cancellation checkpoints**

Create guards before any owned path can survive cancellation. `PartialFileGuard::commit()` disarms only after atomic rename; `StagingDirGuard::commit()` disarms only after the staged directory has been atomically renamed into `revisions/`.

Use `tokio::select!` around resolver/lease sleeps/HTTP send/chunk waits, and call `cancellation.is_cancelled()` before each file, before `spawn_blocking` hash work, and before state/directory commit. Blocking hashing and filesystem commit may complete once started; do not claim forced cancellation of those operations.

- [ ] **Step 4: Run cancellation and legacy lifecycle tests**

```bash
cargo test -p memory_mcp service::model_artifacts::download::tests --locked
cargo test -p memory_mcp --test ner_model_lifecycle --locked
```

Expected: PASS with no lease, staging, or `.part` leftovers.

- [ ] **Step 5: Commit cancellation safety**

```bash
git add crates/memory-mcp/src/service/model_artifacts/download.rs crates/memory-mcp/src/service/model_artifacts.rs crates/memory-mcp/tests/ner_model_lifecycle.rs
git commit -m "fix: clean cancelled NER artifact downloads"
```

---

### Task 5: Add Stable Unavailable-Extractor and MCP Error Semantics

**Files:**
- Modify: `crates/memory-mcp/src/error.rs`
- Modify: `crates/memory-mcp/src/mcp/error.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Test: `crates/memory-mcp/src/mcp/error.rs`
- Test: `crates/memory-mcp/src/service/entity_extraction.rs`

**Interfaces:**
- Produces `MemoryError::ModelNotReady(String)`.
- Produces `UnavailableEntityExtractor::classic_gliner(config: &NerExtractorConfig) -> Arc<dyn EntityExtractor>`.
- `scheduling()` returns `NerScheduling::BlockingPool`, exactly matching the Classic GLiNER registry declaration.
- `fingerprint()` preserves selector, configured labels, threshold, and runtime version; revision, artifact identity, validation status, and effective device are `None`.
- Both extraction methods return the same `ModelNotReady` error; empty custom labels must not silently return success while unavailable.

- [ ] **Step 1: Write unavailable extractor and MCP mapping tests**

```rust
#[test]
fn model_not_ready_requires_restart_and_is_not_retryable() {
    let mapped = mcp_error(MemoryError::ModelNotReady(
        "The configured Classic GLiNER checkpoint is not available locally.".into(),
    ));
    let data = mapped.data.expect("error data");
    assert_eq!(data["kind"], "model_not_ready");
    assert_eq!(data["retryable"], false);
    assert_eq!(data["restart_required"], true);
    assert_eq!(data["activation"], "next_restart");
}
```

Add extractor tests asserting provider `gliner`, `BlockingPool`, selector, exact configured labels and threshold, absent revision/artifact/validation/device fields, and identical errors from default/custom-label extraction.

- [ ] **Step 2: Run focused tests and verify failure**

```bash
cargo test -p memory_mcp model_not_ready --locked
cargo test -p memory_mcp unavailable_classic_gliner --locked
```

Expected: compile failure because the variant and extractor do not exist.

- [ ] **Step 3: Implement error and extractor contracts**

Map `ModelNotReady` to an internal MCP error with exactly:

```rust
json!({
    "kind": "model_not_ready",
    "retryable": false,
    "restart_required": true,
    "activation": "next_restart",
    "explanation": "The configured Classic GLiNER checkpoint is not available locally.",
    "guidance": "Wait for background preparation to complete, then restart Memory MCP."
})
```

Do not reuse `Storage`/`Transient`, because both currently map to `retryable=true`.

- [ ] **Step 4: Run focused and registry tests**

```bash
cargo test -p memory_mcp mcp::error::tests --locked
cargo test -p memory_mcp service::entity_extraction::tests --locked
```

Expected: PASS, including registry scheduling enforcement.

- [ ] **Step 5: Commit unavailable behavior**

```bash
git add crates/memory-mcp/src/error.rs crates/memory-mcp/src/mcp/error.rs crates/memory-mcp/src/service/entity_extraction.rs
git commit -m "feat: report Classic GLiNER model readiness"
```

---

### Task 6: Make Classic GLiNER Startup Local-Only

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`
- Test: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`
- Test: `crates/memory-mcp/tests/ner_model_lifecycle.rs`
- Create: `crates/memory-mcp/tests/ner_gliner_real_activation.rs`

**Interfaces:**
- `gliner::build(...)` calls only `inspect_local`, `new_with_checkpoint`, `probe_and_install`, `promote_candidate`, and `reject_candidate`; it never calls `prepare`, resolver, fetcher, or lease wait.
- Candidate success: probe, promote, use the already probe-installed extractor.
- Candidate failure: reject, then construct previous known-good; if that fails or is absent, return unavailable extractor rather than silently selecting another backend.
- Known-good path: construct directly from identity-verified local checkpoint.
- Recoverable inspection issue: emit a sanitized `ner.local_checkpoint.unavailable` diagnostic, then continue with any independently verified usable role returned in `LocalCheckpointSet`; return unavailable only when neither candidate nor known-good is usable.
- Operational store error: propagate and fail startup.

- [ ] **Step 1: Add local startup decision tests using a builder seam**

Extract a private `build_from_store(native, context, store)` helper so tests inject counting resolver/fetcher collaborators. Add tests for empty store, valid known-good, candidate success, candidate probe failure with fallback, candidate failure without fallback, corrupt known-good degradation, and permission failure propagation.

The fake-byte tests may exercise state decisions but must not claim real GLiNER construction success. Inject a test-only runtime constructor/probe seam at this function boundary for unit tests; keep the production path wired to `GlinerEntityExtractor::new_with_checkpoint` and `probe_and_install`.

- [ ] **Step 2: Run focused builder tests and verify failure**

```bash
cargo test -p memory_mcp service::entity_extraction::gliner::tests::classic_startup --locked
```

Expected: failure because current build calls network-enabled `prepare()`.

- [ ] **Step 3: Implement the local-only startup state machine**

Use typed inspection, not `active_revision()`, to classify state. Log `LocalCheckpointSet.issue` without paths or secrets, but do not discard the other independently identity-verified checkpoint: try a valid candidate first, otherwise use a valid known-good. Return unavailable only when no usable role remains. Promote only after `probe_and_install()` succeeds. On candidate runtime failure, persist rejection before fallback. If fallback construction fails, log a sanitized unavailable event and return `UnavailableEntityExtractor`; do not overwrite the candidate failure reason with fallback failure.

Preserve candidate checkpoint identity in the process fingerprint only after promotion succeeds. Preserve configured selector/labels/threshold for unavailable state.

- [ ] **Step 4: Add an ignored real-fixture restart activation test**

Create a test using `tests/models/ner/urchade--gliner_multi-v2.1`:

```rust
#[tokio::test]
#[ignore = "requires the local Classic GLiNER checkpoint fixture"]
async fn candidate_is_promoted_only_after_real_construction_and_smoke_probe() {
    if !gliner_fixture_present() {
        return;
    }
    // Seed fixture bytes as role Candidate with StaticArtifactVerified.
    // Build Classic GLiNER through the production constructor/probe path.
    // Assert inspect_local returns no candidate and one RuntimeRegressionVerified known-good.
}
```

Do not use fake model bytes for this assertion.

- [ ] **Step 5: Run default tests and compile the ignored test**

```bash
cargo test -p memory_mcp service::entity_extraction::gliner::tests --locked
cargo test -p memory_mcp --test ner_gliner_real_activation --locked --no-run
```

Expected: PASS. Run the ignored test only when the real fixture is present:

```bash
cargo test -p memory_mcp --test ner_gliner_real_activation --locked -- --ignored
```

- [ ] **Step 6: Commit local-only startup**

```bash
git add crates/memory-mcp/src/service/entity_extraction/gliner.rs crates/memory-mcp/tests/ner_model_lifecycle.rs crates/memory-mcp/tests/ner_gliner_real_activation.rs
git commit -m "fix: make Classic GLiNER startup local only"
```

---

### Task 7: Add the One-Shot Post-Readiness Refresh Runtime

**Files:**
- Create: `crates/memory-mcp/src/service/model_artifact_refresh.rs`
- Modify: `crates/memory-mcp/src/service/mod.rs`
- Modify: `crates/memory-mcp/src/service/core/builder.rs`
- Modify: `crates/memory-mcp/src/service/core.rs`
- Test: `crates/memory-mcp/src/service/model_artifact_refresh.rs`

**Interfaces:**
- Produces:

```rust
#[derive(Clone)]
pub(crate) struct NerArtifactRefreshConfig {
    pub(crate) store_root: PathBuf,
    pub(crate) progress: Arc<dyn ModelProgressSink>,
}

pub(crate) struct NerArtifactRefreshRuntime {
    cancellation: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
}

impl NerArtifactRefreshRuntime {
    pub(crate) fn start(config: NerArtifactRefreshConfig, logger: StdoutLogger) -> Self;
    pub(crate) async fn shutdown(mut self);
}
```

- `MemoryService` stores `Option<NerArtifactRefreshConfig>`, not a mutable extractor handle.
- `MemoryService::start_ner_artifact_refresh(&self) -> Option<NerArtifactRefreshRuntime>` starts only when configured backend is Classic GLiNER.
- Worker makes one `refresh_candidate` call, logs the outcome, and exits; it never constructs GLiNER or mutates the service extractor.

- [ ] **Step 1: Write runtime tests with an injected backend**

Define an internal trait implemented by `NerArtifactStore` and a fake in tests:

```rust
#[async_trait]
trait CandidateRefresher: Send + Sync {
    async fn refresh(
        &self,
        cancellation: CancellationToken,
    ) -> Result<CandidateRefreshOutcome, MemoryError>;
}
```

Test event mapping separately from `ModelProgressSink`:

- start → `ner.artifact_refresh.started`;
- `UpToDate` → `ner.artifact_refresh.up_to_date`;
- `CandidateReady` → `ner.artifact_refresh.candidate_ready`, `activation=next_restart`;
- error → `ner.artifact_refresh.failed`, `activation=unchanged`, process continues;
- cancellation → `ner.artifact_refresh.stopped` and joined task.

- [ ] **Step 2: Run runtime tests and verify failure**

```bash
cargo test -p memory_mcp service::model_artifact_refresh::tests --locked
```

Expected: compile failure because the module/runtime does not exist.

- [ ] **Step 3: Implement runtime and service configuration plumbing**

During service build, derive refresh config only from `NerExtractorConfig::ClassicGliner`; reuse the same cache-root rule as `gliner::build`. Store it on `MemoryService`. Do not start the task in the builder. Other lifecycle/claim/fs-watch/embedding startup order remains untouched.

`shutdown()` cancels and awaits the task. Document the precise guarantee: waits and network reads stop promptly; a currently running bounded `spawn_blocking` hash or atomic commit may finish before join. Do not encode a universal 500 ms timeout.

- [ ] **Step 4: Run runtime and builder tests**

```bash
cargo test -p memory_mcp service::model_artifact_refresh::tests --locked
cargo test -p memory_mcp service::core::builder::tests --locked
```

Expected: PASS.

- [ ] **Step 5: Commit refresh runtime**

```bash
git add crates/memory-mcp/src/service/model_artifact_refresh.rs crates/memory-mcp/src/service/mod.rs crates/memory-mcp/src/service/core.rs crates/memory-mcp/src/service/core/builder.rs
git commit -m "feat: add one-shot Classic GLiNER refresh runtime"
```

---

### Task 8: Start Refresh Only After MCP Readiness and Add a Real Process Seam

**Files:**
- Modify: `crates/memory-mcp/src/cli/runtime.rs`
- Modify: `crates/memory-mcp/src/service/model_artifacts/download.rs`
- Modify: `crates/memory-mcp/tests/ner_progress_channels.rs`

**Interfaces:**
- Runtime order is exactly: build service and existing workers → `server.serve(...)` succeeds → log `main.running` → start `NerArtifactRefreshRuntime` → await MCP shutdown → cancel/join refresh during deterministic shutdown.
- Under existing feature `eval-support` only, `HfRevisionResolver`/`HfArtifactFetcher` may read a test artifact-source base URL from an environment variable named `MEMORY_EVAL_NER_ARTIFACT_BASE_URL`.
- Without `eval-support`, production URLs remain hard-coded Hugging Face URLs and the override variable is ignored/uncompiled.

- [ ] **Step 1: Write process tests before runtime integration**

Add a local TCP fixture that accepts the revision request but deliberately blocks its response. Spawn `CARGO_BIN_EXE_memory_mcp` compiled with `eval-support`, configure Classic GLiNER with an empty cache, send MCP `initialize`, and assert a valid response arrives while the HTTP fixture is still blocked. Then call `extract` and assert the structured model-not-ready data:

```rust
assert_eq!(data["kind"], "model_not_ready");
assert_eq!(data["retryable"], false);
assert_eq!(data["restart_required"], true);
assert_eq!(data["activation"], "next_restart");
```

Read stdout as framed JSON-RPC and fail on any non-protocol line. Read stderr separately and assert structured refresh/progress events occur there, never stdout.

- [ ] **Step 2: Run process tests with the real feature seam and verify failure**

```bash
cargo test -p memory_mcp --test ner_progress_channels --features eval-support --locked blocked_gliner_refresh_does_not_delay_initialize
```

Expected: failure because refresh still happens before `.serve()` and no eval-support endpoint seam exists.

- [ ] **Step 3: Implement gated endpoint override and post-readiness ordering**

Create one URL builder used by resolver and fetcher. Under `#[cfg(feature = "eval-support")]`, accept `MEMORY_EVAL_NER_ARTIFACT_BASE_URL`; otherwise build the existing `https://huggingface.co/...` URLs exactly. The override is test infrastructure only and must not alter normal binaries.

In `run_stdio_server`, start refresh after line-equivalent `main.running` logging. During shutdown, cancel/join refresh before returning, while preserving the existing fs-watch → claim → lifecycle shutdown ordering around the other workers.

- [ ] **Step 4: Run all progress/process tests**

```bash
cargo test -p memory_mcp --test ner_progress_channels --features eval-support --locked
```

Expected: PASS. The blocked HTTP server proves no remote refresh consumes the MCP initialize deadline; stdout contains only JSON-RPC.

- [ ] **Step 5: Commit process integration**

```bash
git add crates/memory-mcp/src/cli/runtime.rs crates/memory-mcp/src/service/model_artifacts/download.rs crates/memory-mcp/tests/ner_progress_channels.rs
git commit -m "fix: refresh Classic GLiNER after MCP readiness"
```

---

### Task 9: Record the Decision and Operator Contract

**Files:**
- Create: `docs/adr/0035-background-classic-gliner-refresh.md`
- Modify: `README.md`

**Interfaces:**
- ADR records Classic-only scope, candidate/known-good distinction, next-start runtime validation, no hot swap/silent backend fallback, cancellation limits, and unchanged Anno ONNX/VAGO behavior.
- README tells operators what happens on first install and what action follows `candidate_ready`.

- [ ] **Step 1: Write ADR with explicit alternatives and invariants**

Use sections `Status`, `Context`, `Decision`, `State machine`, `Consequences`, `Rejected alternatives`, and `Validation`. Include this state machine verbatim:

```text
absent -> candidate                  background static acquisition
candidate -> known_good             next-start construction + smoke success
candidate -> incompatible           next-start construction/smoke failure
```

Explicitly reject synchronous pre-initialize refresh, hot swap, treating download as runtime verification, and globally changing other model-backed backends.

- [ ] **Step 2: Update README operator guidance**

Document:

- First start with no Classic GLiNER cache brings MCP up with extraction unavailable.
- Background progress and `ner.artifact_refresh.candidate_ready` appear on stderr/logs.
- `candidate_ready` means restart Memory MCP/Zed to activate after local smoke validation.
- Retrying `extract` in the same process cannot activate the model.
- Anno and Regex remain download-free alternatives.
- Permission/unreadable-cache failures still fail startup explicitly.

- [ ] **Step 3: Verify documentation references and formatting**

```bash
git diff --check -- docs/adr/0035-background-classic-gliner-refresh.md README.md
```

Expected: exit 0.

- [ ] **Step 4: Commit documentation**

```bash
git add docs/adr/0035-background-classic-gliner-refresh.md README.md
git commit -m "docs: explain background Classic GLiNER refresh"
```

---

### Task 10: Run Complete Regression and Release Gates

**Files:**
- Verify all files changed by Tasks 1–9.
- Do not modify or stage `Cargo.lock` unless the user separately authorizes it.

**Interfaces:**
- Verifies every acceptance criterion in the design spec.

- [ ] **Step 1: Run formatting and inspect any diff**

```bash
cargo fmt --all --check
```

Expected: exit 0. If it fails, run `cargo fmt --all`, inspect only formatting changes in task-owned files, then rerun the check.

- [ ] **Step 2: Run focused artifact and process suites**

```bash
cargo test -p memory_mcp --test ner_model_lifecycle --locked
cargo test -p memory_mcp --test ner_progress_channels --features eval-support --locked
cargo test -p memory_mcp --test ner_gliner_real_activation --locked --no-run
```

Expected: all default tests pass and the ignored real-fixture test compiles.

- [ ] **Step 3: Run the production crate suite**

```bash
cargo test -p memory_mcp --locked
```

Expected: exit 0; ignored real-fixture tests remain ignored unless explicitly requested.

- [ ] **Step 4: Run compile checks across feature combinations touched by the change**

```bash
cargo check --workspace --locked
cargo check -p memory_mcp --features eval-support --locked
cargo check -p memory_mcp --features fs-watch,mcp-apps --locked
```

Expected: exit 0. This catches accidental dependence of production code on the eval-only endpoint seam.

- [ ] **Step 5: Run the mandatory zero-warning Clippy gate**

```bash
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps --locked -- -D warnings
```

Expected: exit 0 with zero warnings.

- [ ] **Step 6: Verify scope, stdout purity, and working tree safety**

```bash
git diff --check
git --no-pager diff --stat
git --no-optional-locks status --short
```

Expected:

- no whitespace errors;
- no `Cargo.toml` or new dependency changes;
- no new MCP tool/request surface;
- `Cargo.lock` remains the user’s pre-existing unstaged modification and is absent from every task commit;
- Classic GLiNER process tests prove initialize succeeds while refresh is blocked and stdout remains JSON-RPC-only.

- [ ] **Step 7: Run the real fixture activation test when the fixture is available**

```bash
cargo test -p memory_mcp --test ner_gliner_real_activation --locked -- --ignored
```

Expected: PASS when the local 1.15 GB fixture exists. If absent, report that the test compiled but runtime activation was not locally exercised; do not claim it passed.

---

## Plan Self-Review Checklist

- **Spec coverage:** Every acceptance criterion maps to Tasks 1–10: no pre-readiness network (Tasks 6/8), candidate safety (Tasks 1/3/6), immutable runtime (Tasks 5/7), rollback (Tasks 3/6), typed degradation versus fatal I/O (Tasks 2/6), cancellation cleanup (Task 4), observability/stdout separation (Tasks 7/8), unchanged other backends (Tasks 3/10), and no public/dependency changes (Global Constraints/Task 10).
- **State consistency:** `candidate` is statically verified only; only next-start smoke success invokes `promote_candidate`; `known_good` selection excludes candidates; rejection preserves prior known-good.
- **Type consistency:** `CandidateRefreshOutcome`, `LocalCheckpointSet`, `LocalCheckpointIssue`, `ArtifactRole`, and `MemoryError::ModelNotReady` have one spelling and one owner throughout the plan.
- **Cancellation consistency:** token reaches lease waits, HTTP send/chunks, inter-file boundaries, and pre-commit checks; RAII owns `.part` and staging cleanup; no false universal shutdown bound is promised.
- **Test realism:** fake bytes test artifact state only; real GLiNER construction is proved solely by an ignored real-fixture test; spawned-binary network control uses `eval-support`, not `cfg(test)`.
- **Compatibility:** shared `prepare()` remains intact for Anno ONNX/VAGO; Classic GLiNER alone switches to local inspection plus post-readiness refresh.
- **Error semantics:** unavailable extraction is `retryable=false`, `restart_required=true`, `activation=next_restart`; scheduling remains `BlockingPool`; fingerprint retains selector, labels, threshold, and runtime version.
