# eval-harness Deepening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Remove the seven deepening opportunities found in the 2026-08-11 architecture review of `crates/eval-harness/` — dead bench scaffolding, a degraded merge path, duplicated suite case layers, a leaked reducer idiom, a growing dispatch table, copied bench setup, and a duplicated NER corpus loader — so the evaluation truth layer (ADR-0020/0025) holds for merged artifacts and future suites cost one registry row.

**Architecture:** The changes all deepen existing seams rather than adding new ones. `SuiteReducer` becomes the *only* summary-math path (direct runs and merged shards). A `suites::registry` module owns "which suite ids exist, how to build them, how to reduce them" — serving `main.rs` dispatch, `merge_shards` reduction, and profile validation. Shared case layers (`retrieval_cases`, NER corpus loader) and bench helpers (`ingest_probe`, device-parameterized fixture builder) collapse copy-paste into one module each.

**Tech Stack:** Rust, eval-harness crate only; no new dependencies; no public-surface (memory_mcp CLI / MCP 8-tool) changes.

## Global Constraints

- **Zero warnings:** `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` and `cargo fmt --all --check` must pass before shipping (memory_mcp/AGENTS.md).
- **No new ADR:** this work *implements* ADR-0025 (single formula home) and ADR-0020 (truthful artifacts). Task 2 amends ADR-0025 with one consequence line. No ADR-0016 exception needed (no MCP/CLI surface change).
- **Metric prefix is `entity_mention`** (never `ner_mention`); the NER typed diagnostic key moves to `entity_mention_typed_f1`.
- **Merged artifacts are schema `memory-mcp-eval/v2`** (same shape as a direct run) and must agree with a direct run of the same suites.
- **Deletion test:** only delete what the benches actually consume. Keep `NerBenchmarkFixture` (window texts) and `ContentionObservation`.
- **No behavior change to suite run logic** in Tasks 1, 3, 4, 7 — refactors only, verified by the existing suite tests.
- Existing test idiom: `#[tokio::test]` then `#[ignore]` (that order). No `unwrap()`/`expect()` added to non-test code.

## File Map

- `src/benchmark.rs` — slim to `NerBenchmarkFixture` + `ContentionObservation` (Task 1)
- `benches/ner_cpu.rs` — adapt to infallible fixture; use `ingest_probe` (Tasks 1, 6)
- `benches/ner_metal.rs` — use `build_extractor_for(…, Auto)` + `ingest_probe` (Task 6)
- `benches/contention.rs`, `benches/pipeline.rs` — use `ingest_probe` (Task 6)
- `src/suites/retrieval_cases.rs` — **new** shared case layer (Task 3)
- `src/suites/retrieval.rs`, `src/suites/response_size.rs` — consume shared layer (Task 3)
- 10 suite files — stored reducer fields instead of `Box::leak` (Task 4)
- `src/suites/registry.rs` — **new**: `build_suite` + `reducer_for` (Tasks 2, 5)
- `src/suites/ner_quality.rs` — expose corpus loader + `build_suite`; rename diagnostic key (Tasks 5, 7)
- `src/main.rs` — thin dispatch loop; `cmd_merge` loads the profile (Tasks 2, 5)
- `src/merge.rs` — reduce through registry; compute budget/gates/verdict (Task 2)
- `src/test_support.rs` — add `ingest_probe` (Task 6)
- `src/ner_fixtures.rs` — add `build_extractor_for(kind, device)` (Task 6)
- `tests/ner_quality_corpus.rs`, `tests/ner_quality_real_models.rs` — reuse shared loader (Task 7)
- `docs/adr/0025-single-formula-home-for-eval-metrics.md` — one consequence line (Task 2)

---

### Task 1: Remove dead bench scaffolding from `benchmark.rs`

**Files:**
- Modify: `crates/eval-harness/src/benchmark.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs:30`

**Interfaces:**
- Consumes: nothing (pure deletion).
- Produces: `NerBenchmarkFixture { single_window, multi_window }` with `pub fn load() -> Self` (infallible), accessors `single_window()` / `multi_window()` / `multi_window_token_count()`. `ContentionObservation` unchanged.

- [x] **Step 1: Delete dead items** from `benchmark.rs`:
  - `NerRunner` (and `impl`), `NerOutput`, `NerEntity`, `NerOutput::canonical`, `UnsupportedDevice`, `BenchmarkProvenance`, `assert_candidate_parity`, `NerBenchmarkFixture::metadata_only`, and the fields `model_name`, `model_digest`, `labels`, `threshold` from `NerBenchmarkFixture`.
  - Their unit tests (`parity_check_*`).
  - Drop `use crate::error::EvalError;` if no longer referenced (it is not — `ContentionObservation` uses no `EvalError`).
  - Change `load()` to return `Self` (remove `Result`/`Ok` wrapper; the two window strings cannot fail).

- [x] **Step 2: Adapt call sites**

```rust
// benches/ner_cpu.rs — was: NerBenchmarkFixture::load().unwrap()
let fixture = eval_harness::benchmark::NerBenchmarkFixture::load();
```

- [x] **Step 3: Update remaining tests** in `benchmark.rs` — `ner_fixture_loads_with_required_fields` becomes `ner_fixture_loads_window_texts` (assert both windows non-empty and token counts positive); `multi_window_exceeds_single_window` unchanged.

- [x] **Step 4: Verify** — `cargo test -p eval-harness`, then `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`.

- [x] **Step 5: Commit** — `refactor(eval): remove dead NER bench scaffolding and fabricated digest`

---

### Task 2: Make `merge_shards` reduce through the suite reducers and re-derive gates + verdict

**Files:**
- Create: `crates/eval-harness/src/suites/registry.rs` (minimal: only `reducer_for`; Task 5 expands it)
- Modify: `crates/eval-harness/src/suites.rs` (`pub mod registry;`)
- Modify: `crates/eval-harness/src/merge.rs`
- Modify: `crates/eval-harness/src/main.rs:265-307` (`cmd_merge` loads profile, passes manifest)
- Modify: `crates/eval-harness/src/suites/ner_quality.rs` — expose `pub(crate) fn build_reducer(suite_id, kind) -> Box<dyn SuiteReducer>` (fixture-dependent), used by both `NerQualitySuite::new` and the registry
- Modify: `docs/adr/0025-single-formula-home-for-eval-metrics.md` (one consequence line)

**Interfaces:**
- Consumes: `crate::reducer::{SuiteReducer, ClassificationReducer, CountReducer, RetrievalReducer, RatioReducer}`, `crate::profile::ProfileManifest`, `crate::artifact::{GateStatus, RunArtifact, SuiteSummary}`, `crate::gate::evaluate_gates`, `crate::domain::derive_run_verdict`.
- Produces:
  - `pub fn reducer_for(suite_id: &str) -> Box<dyn SuiteReducer>` in `suites::registry` — known ids return their suite reducer (ner-quality ids: `ClassificationReducer` when `ner_fixtures::fixture_present(kind)` else `CountReducer`); unknown ids return `CountReducer::new(suite_id)`.
  - `pub fn merge_shards(shards: &[RunArtifact], manifest: &ProfileManifest) -> Result<RunArtifact, EvalError>`.

- [x] **Step 1: Write the failing test** (merge must aggregate, not first-win; verdict must not be blindly Passed)

```rust
// in merge.rs tests — merge of classification outcomes aggregates confusion counts
fn classification_shard_outcome(case_id: &str, suite: &str, tp: u64, fp: u64, fn_: u64) -> EvalCaseOutcome {
    let mut outcome = EvalCaseOutcome::new(suite, case_id, EvalMode::EndToEnd,
        CorpusSplit::Test, LabelTrust::Official, CaseStatus::QualityFailed);
    let evidence = MetricEvidence::classification(tp, fp, fn_, 0);
    outcome.evidence.insert("classification".to_string(), evidence.clone());
    outcome.metrics = crate::metrics::render_case_metrics(
        &evidence, &crate::metrics::CaseMetricNames::classification("entity"));
    outcome
}

#[test]
fn merged_classification_metrics_aggregate_instead_of_first_wins() {
    let manifest = test_manifest(EvalProfile::Pr);
    // shard A: tp=2 fp=0 fn=1 -> f1 0.8 ; shard B: tp=1 fp=0 fn=2 -> f1 0.5
    let shard_a = make_shard_with_outcomes("extraction", vec![classification_shard_outcome("c1", "extraction", 2, 0, 1)]);
    let shard_b = make_shard_with_outcomes("extraction", vec![classification_shard_outcome("c2", "extraction", 1, 0, 2)]);
    let merged = merge_shards(&[shard_a, shard_b], &manifest).unwrap();
    let summary = merged.suite_summaries.iter().find(|s| s.suite_id == "extraction").unwrap();
    // aggregate tp=3 fp=0 fn=3 -> precision 1.0, recall 0.5, f1 2/3
    assert!((summary.metrics["entity_f1"] - 2.0 / 3.0).abs() < 1e-9, "got {}", summary.metrics["entity_f1"]);
}

#[test]
fn merged_verdict_reflects_quality_failures() {
    let manifest = test_manifest(EvalProfile::Pr);
    let shard = make_shard_with_outcomes("extraction", vec![classification_shard_outcome("c1", "extraction", 1, 0, 2)]);
    let merged = merge_shards(&[shard], &manifest).unwrap();
    assert_eq!(merged.verdict, RunVerdict::QualityFailed);
}
```

`test_manifest` helper: `ProfileManifest { schema_version: "memory-mcp-eval-profile/v1".into(), profile, time_budget_seconds: 600, suites: vec![], gates: vec![] }`. `make_shard_with_outcomes(suite, outcomes)` mirrors `make_shard_with_suite` but takes pre-built outcomes (expected ids derived from them).

- [x] **Step 2: Run to verify failure** — `cargo test -p eval-harness merged_` → FAIL (current merge produces `entity_f1` = 0.8 via first-wins and `verdict` = Passed).

- [x] **Step 3: Add `reducer_for` to `suites/registry.rs`** and a `ner_quality::build_reducer`

```rust
// ner_quality.rs
pub(crate) fn build_reducer(suite_id: &str, kind: NerExtractorKind) -> Box<dyn SuiteReducer> {
    if ner_fixtures::fixture_present(kind) {
        Box::new(ClassificationReducer::new(suite_id.to_string(), "entity_mention"))
    } else {
        Box::new(CountReducer::new(suite_id.to_string()))
    }
}
// NerQualitySuite::new uses build_reducer and drops the NerSuiteReducer enum for a Box<dyn SuiteReducer> field.
```

```rust
// suites/registry.rs — Task 2 minimal form
pub fn reducer_for(suite_id: &str) -> Box<dyn SuiteReducer> {
    use crate::reducer::*;
    match suite_id {
        "local-retrieval" => Box::new(RetrievalReducer::new("local-retrieval", 5)),
        "extraction" => Box::new(ClassificationReducer::new("extraction", "entity")),
        "claim-reconciliation" => Box::new(ClassificationReducer::new("claim-reconciliation", "claim")),
        "end-to-end" => Box::new(RatioReducer::new("end-to-end", E2E_SPECS)),
        "external-retrieval" => Box::new(RetrievalReducer::new("external-retrieval", 5)),
        "action-grounding" => Box::new(CountReducer::new("action-grounding")),
        "capacity" => Box::new(CountReducer::new("capacity")),
        "poisoning" => Box::new(CountReducer::new("poisoning")),
        "lifecycle" => Box::new(RatioReducer::new("lifecycle", LIFECYCLE_SPECS)),
        "downstream-qa" => Box::new(CountReducer::new("downstream-qa")),
        "response-size" => Box::new(ResponseSizeReducer::new("response-size")),
        _ => crate::suites::ner_quality::reducer_for_suite(suite_id)
            .unwrap_or_else(|| Box::new(CountReducer::new(suite_id))),
    }
}
```

`ner_quality::reducer_for_suite(suite_id)` maps `ner-quality-*` ids via `kind_for_id` and calls `build_reducer`; returns `Option<Box<dyn SuiteReducer>>`. Move `E2E_SPECS` / `LIFECYCLE_SPECS` to module-level `const`/`static` in `suites/registry.rs` (copy the literal specs from the suites, or reference them if they become `pub(crate)` — prefer copying into registry and deleting the in-method statics in Task 4).

- [x] **Step 4: Rewrite `merge_shards(shards, manifest)`**

Keep: empty-shards check; schema/profile/fingerprint-config-hash equality checks (extend to `shard.profile != manifest.profile`); outcome dedup `(suite_id, case_id)`; expected-id coverage; outcome sort.

Replace the tail (from `let mut metric_sums` on) with:

```rust
let mut by_suite: BTreeMap<String, Vec<EvalCaseOutcome>> = BTreeMap::new();
for outcome in all_outcomes.clone() {
    by_suite.entry(outcome.suite_id().to_string()).or_default().push(outcome);
}
let suite_summaries = by_suite
    .into_iter()
    .map(|(suite_id, outcomes)| crate::suites::registry::reducer_for(&suite_id).reduce(&outcomes))
    .collect::<Result<Vec<SuiteSummary>, EvalError>>()?;

let duration_ms = shards.iter().map(|s| s.duration_ms).sum::<u64>();
let budget_status = if manifest.time_budget_seconds > 0 {
    let budget_ms = manifest.time_budget_seconds as u64 * 1000;
    Some(if duration_ms > budget_ms { GateStatus::Failed } else { GateStatus::Passed })
} else { None };

let pending = RunArtifact {
    schema_version: crate::EVAL_ARTIFACT_SCHEMA_V1.to_string(),
    run_id: "pending".into(), profile: manifest.profile, started_at: chrono::Utc::now(),
    duration_ms, expected_case_ids: expected_ids.clone(), expected_cases: vec![],
    outcomes: all_outcomes.clone(), suite_summaries: suite_summaries.clone(), gates: vec![],
    fingerprint: fingerprint.clone(), budget_status: None,
    verdict: crate::domain::RunVerdict::default(), issues: vec![],
};
let gates = crate::evaluate_gates(&manifest.gates, &pending, None)?;
let budget = budget_status.unwrap_or(GateStatus::Invalid);
let verdict = derive_run_verdict(&all_outcomes, &gates, budget.clone(), &[]);

let artifact = RunArtifact {
    schema_version: crate::EVAL_ARTIFACT_SCHEMA_V2.to_string(),
    run_id: format!("merged-{}", chrono::Utc::now().timestamp()),
    profile: manifest.profile, started_at: first.started_at, duration_ms,
    expected_case_ids, expected_cases: vec![], outcomes: all_outcomes,
    suite_summaries, gates, fingerprint, budget_status: Some(budget),
    verdict, issues: vec![],
};
artifact.validate()?;
Ok(artifact)
```

Delete `compute_suite_summaries` and the `metric_sums` block.

- [x] **Step 5: Update existing merge tests** to pass `&test_manifest(EvalProfile::Pr)` (shards built with `profile: EvalProfile::Pr` match the manifest; `ner-quality-*` shards keep working — `reducer_for` is fixture-dependent but both branches produce one summary per suite).

- [x] **Step 6: Wire `cmd_merge` in `main.rs`**

```rust
let manifest = match ProfileManifest::load(&profile_path) { Ok(m) => m, Err(e) => { eprintln!("error: {e}"); return ExitCode::from(2); } };
// was: merge_shards(&shards)
eval_harness::merge_shards(&shards, &manifest)
```
Rename `_profile_path` → `profile_path`.

- [x] **Step 7: Amend ADR-0025** — add to Consequences: *"Merged shard artifacts reduce through the same suite reducers and re-evaluate gates/verdict, so a merged artifact cannot disagree with a direct run of the same suites."*

- [x] **Step 8: Verify** — `cargo test -p eval-harness`, clippy command, `cargo fmt --all --check`.

- [x] **Step 9: Commit** — `fix(eval): merge shards through suite reducers with re-derived gates and verdict`

---

### Task 3: Extract the shared retrieval case layer

**Files:**
- Create: `crates/eval-harness/src/suites/retrieval_cases.rs`
- Modify: `crates/eval-harness/src/suites.rs` (`pub mod retrieval_cases;` — private to crate: use `mod retrieval_cases;` + re-export needed types)
- Modify: `crates/eval-harness/src/suites/retrieval.rs`, `crates/eval-harness/src/suites/response_size.rs`

**Interfaces:**
- Produces (all `pub(crate)`): `RetrievalEvalCase`, `SeedFact`, `SeedEntity`, `SeedCommunity`, `SeedEdge`, `RetrievalExpectation`, `fn load_cases() -> Result<Vec<RetrievalEvalCase>, EvalError>`, `fn case_as_of(case: &RetrievalEvalCase) -> DateTime<Utc>`, `fn fixture_path() -> PathBuf`.

- [x] **Step 1: Move the shared types + helpers** verbatim into `retrieval_cases.rs` (the union of both files' identical definitions; keep `#[allow(dead_code)]` markers from the *response_size* copy so lib-only builds don't flag fields used only by one suite).

- [x] **Step 2: Rewrite `retrieval.rs` and `response_size.rs`** to `use crate::suites::retrieval_cases::*;` (or `use super::retrieval_cases::*;`) and delete the private copies.

- [x] **Step 3: Verify** — existing suite tests (`fixture_loads_and_has_cases`, `case_ids_are_deterministic`, `single_case_produces_valid_outcome`, response_size tests) pass unchanged: `cargo test -p eval-harness`.

- [x] **Step 4: Commit** — `refactor(eval): share retrieval case layer between retrieval and response-size suites`

---

### Task 4: Replace the `Box::leak` + `static OnceLock` reducer idiom with stored fields

**Files:**
- Modify (10 files): `suites/action_grounding.rs`, `suites/capacity.rs`, `suites/claims.rs`, `suites/downstream_qa.rs`, `suites/end_to_end.rs`, `suites/external_retrieval.rs`, `suites/extraction.rs`, `suites/lifecycle.rs`, `suites/poisoning.rs`, `suites/retrieval.rs`, `suites/response_size.rs`

**Interfaces:**
- Consumes: nothing new — each suite already imports its reducer.
- Produces: each suite struct gains a concrete reducer field; `reducer()` returns `&self.reducer`. Pattern:

```rust
pub struct PoisoningSuite {
    expected_ids: Vec<EvalCaseId>,
    reducer: CountReducer,
}
impl PoisoningSuite {
    pub fn new() -> Self {
        let expected_ids = /* existing */;
        Self { expected_ids, reducer: CountReducer::new("poisoning") }
    }
}
impl EvalSuite for PoisoningSuite {
    fn reducer(&self) -> &dyn SuiteReducer { &self.reducer }
    // ... rest unchanged
}
```

- [x] **Step 1–11: One suite at a time** — for each file: add the field, construct it in `new()` (and `Default` where it delegates to `new()`), simplify `reducer()` to `&self.reducer`, delete the `static OnceLock` + `Box::leak` block. End-to-end/lifecycle move their `SPECS` into `new()` (keep the `static` for the spec array — it's a `&'static` slice; only the reducer box/leak goes away). Response-size stores `ResponseSizeReducer` directly.

- [x] **Step 12: Verify** — `cargo test -p eval-harness`; clippy; fmt.

- [x] **Step 13: Commit** — `refactor(eval): store suite reducers as fields instead of leaked statics`

---

### Task 5: Suite registry — build + reduce behind one seam

**Files:**
- Modify: `crates/eval-harness/src/suites/registry.rs` (expand: add `build_suite`)
- Modify: `crates/eval-harness/src/main.rs:53-152` (thin loop)
- Modify: `crates/eval-harness/src/suites/ner_quality.rs` (replace `register` with `build_suite`; keep `kind_for_id` `pub(crate)`)

**Interfaces:**
- Produces:
  - `pub fn build_suite(decl: &SuiteDecl) -> Result<Option<Box<dyn EvalSuite>>, EvalError>` — `Ok(None)` for unknown ids; `Err` on construction failure (caller warns + records an empty-suite issue); the `external-retrieval` arm (corpus manifest load + normalize) moves here from `main.rs`.
  - `ner_quality::build_suite(suite_id) -> Result<Box<dyn EvalSuite>, EvalError>` replacing `register`.

- [x] **Step 1: Write the failing test** in `registry.rs`

```rust
#[test]
fn every_declared_suite_builds() {
    let ids = ["local-retrieval", "extraction", "claim-reconciliation", "end-to-end",
        "external-retrieval", "action-grounding", "capacity", "poisoning", "lifecycle",
        "downstream-qa", "response-size", "ner-quality-anno", "ner-quality-regex",
        "ner-quality-anno-onnx", "ner-quality-gliner", "ner-quality-vago"];
    for id in ids {
        let decl = SuiteDecl { id: id.into(), corpus_root: None, expected_coverage: None };
        assert!(build_suite(&decl).unwrap().is_some(), "{id} must build");
        assert!(!registry_reducer_for(id).is_err_none_marker(), "{id} must have a reducer");
    }
    let unknown = SuiteDecl { id: "nope".into(), corpus_root: None, expected_coverage: None };
    assert!(build_suite(&unknown).unwrap().is_none());
}
```

(Simplify: assert `build_suite(&decl).unwrap().is_some()` for each id and `None` for unknown; assert `reducer_for(id).reduce(&[])` never errors for known ids. `external-retrieval` without `corpus_root` must return `Err` — assert that separately.)

- [x] **Step 2: Implement `build_suite`** by moving the `match suite_decl.id.as_str()` body from `main.rs::cmd_run` (including the `external-retrieval` corpus-loading arm) into the registry. `reducer_for` stays as-is from Task 2.

- [x] **Step 3: Slim `cmd_run`** in `main.rs` to the loop:

```rust
for suite_decl in &manifest.suites {
    if !suite_filter.is_empty() && !suite_filter.contains(&suite_decl.id) { continue; }
    match eval_harness::suites::registry::build_suite(suite_decl) {
        Ok(Some(suite)) => suites.push(suite),
        Ok(None) => { eprintln!("warning: unknown suite {}", suite_decl.id);
                      issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id)); }
        Err(e) => { eprintln!("warning: failed to load {}: {e}", suite_decl.id);
                    issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id)); }
    }
}
```

- [x] **Step 4: Delete `ner_quality::register`** (no callers remain) after adding `ner_quality::build_suite`.

- [x] **Step 5: Verify** — `cargo test -p eval-harness` (incl. `ner_quality_profile_loads_with_expected_suites` in profile.rs); clippy; fmt.

- [x] **Step 6: Commit** — `refactor(eval): centralize suite build and reduce in a registry`

---

### Task 6: Collapse copied bench setup into helpers

**Files:**
- Modify: `crates/eval-harness/src/test_support.rs` (add `ingest_probe`)
- Modify: `crates/eval-harness/src/ner_fixtures.rs` (add `build_extractor_for`)
- Modify: `benches/ner_cpu.rs`, `benches/ner_metal.rs`, `benches/contention.rs`, `benches/pipeline.rs`

**Interfaces:**
- Produces:
  - `pub async fn ingest_probe(service: &MemoryService, source_id: &str, content: &str) -> String` (returns episode id; scope `org`, `source_type` `bench`).
  - `pub async fn build_extractor_for(kind: NerExtractorKind, device: GlinerDeviceKind) -> Option<Arc<dyn EntityExtractor>>`; `build_extractor(kind)` becomes `build_extractor_for(kind, GlinerDeviceKind::Cpu)`; VAGO arm takes `device`; anno-onnx/classic-gliner ignore it.

- [x] **Step 1: Add `ingest_probe` to `test_support.rs`** (extract the `IngestRequest` literal the four benches repeat; use `expect` like the rest of `test_support`).

- [x] **Step 2: Add `build_extractor_for` to `ner_fixtures.rs`**; keep the docstring's "never download, fixture-gated" contract; `build_extractor` delegates.

- [x] **Step 3: Update `ner_cpu.rs`** — `bench_default_service_probe` uses `test_support::ingest_probe`.

- [x] **Step 4: Update `ner_metal.rs`** — `bench_ner_apple_silicon_single_window` uses `ingest_probe`; `bench_vago_apple_silicon_single_window` builds via `ner_fixtures::build_extractor_for(NerExtractorKind::SauerkrautLfm25, GlinerDeviceKind::Auto)` (skip note when `None`) and deletes its hand-rolled config + labels literal.

- [x] **Step 5: Update `contention.rs` / `pipeline.rs`** to use `ingest_probe`.

- [x] **Step 6: Verify** — `cargo check -p eval-harness --benches`; `cargo test -p eval-harness`; clippy (`--all-targets` covers benches).

- [x] **Step 7: Commit** — `refactor(eval): share bench ingest probe and device-parameterized fixture builder`

---

### Task 7: Share the NER corpus loader and fix the metric-key convention

**Files:**
- Modify: `crates/eval-harness/src/suites/ner_quality.rs`
- Modify: `crates/eval-harness/tests/ner_quality_corpus.rs`
- Modify: `crates/eval-harness/tests/ner_quality_real_models.rs`

**Interfaces:**
- Produces: `pub(crate) fn load_corpus() -> Result<CorpusFile, EvalError>`; `pub(crate) fn load_cases() -> Result<Vec<NerQualityCase>, EvalError>`; `CorpusFile` becomes `pub(crate)`.
- Renames: metric key `ner_typed_f1` → `entity_mention_typed_f1` (in `run_case` + its unit test).

- [x] **Step 1: Grep** — confirm `ner_typed_f1` appears only in `suites/ner_quality.rs`.

- [x] **Step 2: Expose the loader** — make `CorpusFile` + `corpus_path` + `load_corpus` + `load_cases` `pub(crate)`; keep the `#[allow(dead_code)]` markers for lib-only builds.

- [x] **Step 3: Rewrite `tests/ner_quality_corpus.rs`** to deserialize via `ner_quality::load_corpus()` (keep every assertion; the structural checks now exercise the shared shape).

- [x] **Step 4: Rewrite `tests/ner_quality_real_models.rs`** to use `ner_quality::load_cases()` (drop the private `Corpus` struct).

- [x] **Step 5: Rename the diagnostic key** in `run_case` and `typed_diagnostic_punishes_label_mismatch`.

- [x] **Step 6: Verify** — `cargo test -p eval-harness` (incl. corpus tests); clippy; fmt.

- [x] **Step 7: Commit** — `refactor(eval): share NER corpus loader and align metric key convention`

---

## Final validation gate

- [x] `cargo test -p eval-harness` — all pass
- [x] `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` — zero warnings
- [x] `cargo fmt --all --check` — zero diff
- [x] `cargo bench -p eval-harness --bench ner_cpu -- --noplot` still compiles and runs (fixture-gated skip on machines without checkpoints)

## Self-Review

**1. Spec coverage** — All seven review candidates map to tasks: #1→Task 1, #2→Task 2, #3→Task 3, #5→Task 4, #4→Task 5, #6→Task 6, #7→Task 7. The coupled pair (#2 needs `reducer_for`) is resolved by Task 2 creating the minimal registry and Task 5 expanding it.

**2. Placeholder scan** — No TBD/TODO. Every task has concrete code or a precise mechanical rule.

**3. Type consistency** — `merge_shards(shards, &manifest)` appears in Task 2 and all its tests use the same signature. `reducer_for(suite_id) -> Box<dyn SuiteReducer>` is created in Task 2 and consumed unchanged in Task 5. `build_extractor_for(kind, GlinerDeviceKind)` in Task 6 is consumed by `ner_metal.rs` with `GlinerDeviceKind::Auto`. Metric key is `entity_mention_typed_f1` everywhere (Task 7).
