# Evaluation Integrity and Benchmark Coverage Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every repository benchmark execute through an explicit profile with truthful coverage, provenance, failure semantics, and CI or documented platform-specific scheduling; repair the LongMemEval-V2 integration instead of treating a no-op smoke test as evidence.

**Architecture:** Keep the evaluation harness separate from production MCP behavior. Profiles are the source of execution policy, schema-v2 artifacts identify cases by `(suite_id, case_id)`, aggregate gates decide quality, and invalid configuration or missing baselines fail closed. Criterion remains the latency/microbenchmark plane; academic corpora and LongMemEval-V2 remain reproducibly pinned evaluation planes.

**Tech Stack:** Rust 2024, Rust 1.97.1, Cargo, Criterion, Python 3, GitHub Actions, JSON artifacts.

**Spec:** `docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md`. The upstream LongMemEval-V2 contract is pinned to commit `6f020ac2fc3275e46c706d3406e02c3ed79b7be2`.

**Upstream references:** `https://github.com/xiaowu0162/LongMemEval-V2/`, pinned interface `https://github.com/xiaowu0162/LongMemEval-V2/blob/6f020ac2fc3275e46c706d3406e02c3ed79b7be2/memory_modules/memory.py`, paper `https://arxiv.org/abs/2605.12493`.

## Global Constraints

- This plan is P0 and blocks completion of `2026-09-04-hobby-stabilization.md`.
- Do not change production retrieval behavior merely to improve a score.
- Do not add dependencies or modify generated/migration files without approval.
- A profile run is valid only when all selected suite-scoped cases produced outcomes and the artifact records the effective profile, build, corpus, evaluator, and provider context.
- Missing or malformed required baselines, unknown gate suites, and unsupported gate selectors are configuration errors, never silent omissions.
- `QualityFailed` is an aggregate gate result. A successfully evaluated NER case remains `Passed`; its precision/recall errors stay in evidence and metrics.
- Response-size evaluation remains diagnostic as specified, but its job must run and must fail on invalid/incomplete execution.
- NER model evaluation remains manual/platform-aware when model fixtures are unavailable; “manual” means an exact required command and artifact, not “not executed”.
- LongMemEval-V2 results must be labelled text-only while image-query support is absent. They must not be compared as full multimodal leaderboard results.
- Generated corpora, upstream checkouts, model assets, and `target/evals` artifacts are not committed.

## Required execution matrix

| Plane | Profile/command | Required execution |
|---|---|---|
| PR correctness | `make eval-pr` | every pull request, blocking |
| Release breadth | `make eval-release` | release events, blocking |
| Scheduled breadth | `make eval-nightly` | schedule/manual dispatch, blocking |
| Response size | `make eval-response-size` | pull request and nightly, blocking on execution integrity |
| NER comparison | `make eval-ner-quality` | documented manual run with fixtures; artifact required for NER changes |
| Classic corpora | one profile each for LongMemEval, LoCoMo, PersonaMem, PrefEval | after pinned preparation; required for retrieval-quality claims |
| LongMemEval-V2 | pinned upstream adapter/profile | required for LongMemEval-V2 claims; text-only label |
| Criterion | pipeline, contention, `ner_cpu`; `ner_metal` on Apple Silicon | compile on PR; measured scheduled/manual run with artifacts |

## Grounded pre-fix snapshot

The following was observed on master commit `78d267a7` on 2026-09-04. It is diagnostic input to this plan, not release evidence:

- `make eval-pr`: passed, 113 outcomes, 7 gates.
- `make eval-release`: passed, 117 outcomes, 9 gates.
- `make eval-nightly`: passed, 115 outcomes, 1 gate.
- Direct `response_size.json`: passed, 61 outcomes, no gates, combined savings `38.7762%`; the combined value does not yet prove separate assemble/explain targets.
- Direct `ner_quality.json`: exited 1 with 50 outcomes and 31 `QualityFailed` cases. Aggregate F1 was anno `0.7473`, regex `0.7447`, anno-onnx `0.2185`, GLiNER `0.9184`, and Vago `0.9302`; no aggregate gates existed.
- The NER report incorrectly declared 10 expected cases because schema-v2 writing still deduplicated bare case IDs across five suites. Tasks 2 and 4 separate coverage integrity from quality thresholds.
- The current LongMemEval-V2 `--smoke-test` has no executable entry point, and `run_pinned.sh` converts that absence into a successful script exit. Task 7 removes that false green path.

---

### Task 1: Make profile declarations closed and auditable

**Files:**
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `crates/eval-harness/src/registry.rs`
- Test: `crates/eval-harness/src/profile.rs`
- Modify: `evals/profiles/*.json`
- Modify: `docs/evals/README.md`

**Interfaces:**
- Extend `ProfileManifest::validate` (or add `validate_against_registry`) so runtime validation receives the registered suite IDs and checks suite membership, gates, selectors, and baseline policy.
- `SuiteRegistry` exposes stable registered suite IDs for a coverage test.

- [ ] Add failing tests proving that validation rejects a gate whose `suite_id` is not declared by the profile, rejects non-null `mode`, `split`, or `label_trust` selectors until sliced summaries exist, and rejects a regression budget without a required baseline.
- [ ] Add a registry/profile coverage test that loads every committed profile and asserts every registered non-platform-specific suite is either selected by at least one profile or named in an explicit `manual_suites` list with its command and prerequisite.
- [ ] Remove redundant standalone suite registrations when the canonical lifecycle suite already executes the same cases; otherwise assign them to `nightly.json`. Add `capacity` to nightly if it is not duplicated.
- [ ] Run:

```bash
cargo test -p eval-harness profile --locked
cargo test -p eval-harness registry --locked
```

- [ ] Document the resulting suite-to-profile table in `docs/evals/README.md`; the table must be generated from the same IDs asserted by the test, not from aspirational names.
- [ ] Commit: `fix(eval): validate complete profile coverage`.

---

### Task 2: Use suite-scoped expected cases in schema v2

**Files:**
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `crates/eval-harness/src/runner.rs`
- Modify: `crates/eval-harness/src/report.rs`
- Modify: `crates/eval-harness/src/merge.rs`
- Test: corresponding module tests

**Interfaces:**
- Schema-v2 `expected_cases: Vec<CaseKey>` is authoritative.
- `expected_case_ids` is read-only legacy input for schema-v1 compatibility and is empty in newly written v2 artifacts.

- [ ] Add a failing test with `suite-a/shared-id` and `suite-b/shared-id`; omit only the second outcome and assert validation reports exactly `suite-b/shared-id` missing.
- [ ] Change `Runner` to populate and deterministically sort/deduplicate `CaseKey { suite_id, case_id }`. Never deduplicate on bare `case_id`.
- [ ] Update validation, merge, and reporting to prefer `expected_cases`; accept `expected_case_ids` only while reading an older schema artifact.
- [ ] Assert the NER report counts 50 expected outcomes for five suites with ten local IDs, rather than collapsing them to 10.
- [ ] Run:

```bash
cargo test -p eval-harness artifact runner report merge --locked
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/ner_quality.json --artifact target/evals/ner-quality-contract.json
```

Expected: the artifact contains 50 distinct suite-scoped expected keys and validates complete coverage.
- [ ] Commit: `fix(eval): preserve suite-scoped case coverage`.

---

### Task 3: Fail closed on baselines and record real provenance

**Files:**
- Modify: `crates/eval-harness/src/cli.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Modify: `crates/eval-harness/src/runner.rs`
- Test: CLI, artifact, and runner module tests
- Modify: `evals/baselines/*.json`

**Interfaces:**
- Explicit `--baseline PATH` returns exit 2 for absent, unreadable, malformed, or incompatible input.
- Profiles with regression budgets require a baseline.
- `RunFingerprint` contains no sentinel value such as `uncomputed` for required fields.

- [ ] Add CLI tests for missing and malformed baseline paths and runner tests for a regression budget without baseline; assert configuration failure before suite execution.
- [ ] Build the fingerprint from the exact profile bytes/digest, selected feature list, runtime `git rev-parse HEAD` with the compile-time commit as fallback, suite/corpus digests, evaluator versions, and configured provider/model/device values. Represent genuinely inapplicable values as `None`; do not invent them.
- [ ] Make validation reject missing required profile/config/corpus fingerprints for PR, release, and nightly artifacts.
- [ ] Refresh committed small baselines only after Tasks 1-2 pass; record the command and source commit beside each baseline.
- [ ] Run malformed-input tests and then `make eval-pr`; inspect the emitted JSON and confirm its commit equals `git rev-parse HEAD` and its profile digest changes when profile content changes.
- [ ] Commit: `fix(eval): enforce baselines and run provenance`.

---

### Task 4: Correct NER and response-size profile semantics

**Files:**
- Modify: `crates/eval-harness/src/suites/ner_quality.rs`
- Modify: `crates/eval-harness/src/suites/response_size.rs`
- Modify: `evals/profiles/ner_quality.json`
- Modify: `evals/profiles/response_size.json`
- Add/modify: `evals/baselines/ner-quality.json`
- Test: both suite modules

**Interfaces:**
- Successful extraction/evaluation emits `OutcomeStatus::Passed`; entity misses are evidence and aggregate metrics.
- Profile gates/regression budgets, not exact per-case equality, decide NER quality failure.
- Response-size reports separate compact ratios for `assemble_context` and `explain` if both response classes are claimed.

- [ ] Add a failing NER test where an extractor returns an imperfect but valid result; assert the case passes execution while precision/recall/F1 reflect the miss. Preserve true `Invalid`, `Skipped`, and runtime failure states.
- [ ] Add aggregate NER gates or baseline regression budgets for each selected backend. Seed them from a reviewed clean run; do not encode the current `anno-onnx` score as an unexplained quality target.
- [ ] Split response-size aggregation by response class and assert each expected class has non-zero cases. Preserve the design-spec diagnostic thresholds and report them explicitly; do not substitute the current combined `overall_savings_pct` for two different claims.
- [ ] Run:

```bash
make eval-response-size
make eval-ner-quality
```

Expected: both commands execute all expected suite-scoped cases; neither fails merely because a comparative extractor is not exact; any failure names an aggregate gate, regression, invalid case, or missing prerequisite.
- [ ] Commit: `fix(eval): separate execution from quality gates`.

---

### Task 5: Put every built-in profile into Make and CI

**Files:**
- Modify: `Makefile`
- Modify: `.github/workflows/ci.yml`
- Modify: `docs/evals/README.md`

**Interfaces:**
- Make targets own exact profile and baseline arguments.
- CI calls Make targets, uploads JSON/report artifacts with `if: always()`, and never uses `continue-on-error` for required evaluation jobs.

- [ ] Add `eval-response-size` and `eval-ner-quality` targets. Ensure `eval-pr`, `eval-release`, and every profile with regression budgets supplies its baseline.
- [ ] Change PR, release, nightly, and response-size CI steps to call those targets. Remove `continue-on-error: true` from required evaluation jobs.
- [ ] Make release binary jobs depend on the release evaluation job. Keep NER manual when fixtures/platform support are absent, but compile its suite in CI and print its exact manual prerequisite instead of claiming it ran.
- [ ] Upload each produced artifact/report with profile name, commit SHA, and run ID in the artifact name.
- [ ] Run locally:

```bash
make eval-pr
make eval-release
make eval-nightly
make eval-response-size
```

- [ ] Inspect CI YAML and assert there is no required profile invoked without its baseline and no required evaluation step masked by `continue-on-error`.
- [ ] Commit: `ci(eval): run required benchmark profiles`.

---

### Task 6: Give each classic external corpus its own reproducible profile

**Files:**
- Modify: `evals/corpora/longmemeval.json`
- Modify: `evals/corpora/locomo.json`
- Modify: `evals/corpora/personamem.json`
- Modify: `evals/corpora/prefeval.json`
- Add: `evals/profiles/external_longmemeval.json`
- Add: `evals/profiles/external_locomo.json`
- Add: `evals/profiles/external_personamem.json`
- Add: `evals/profiles/external_prefeval.json`
- Modify: `crates/eval-harness/src/corpus/prepare.rs`
- Modify: `crates/eval-harness/src/corpus/manifest.rs`
- Test: the corresponding Rust module tests
- Modify: `docs/evals/README.md`

**Interfaces:**
- Each manifest pins source URL/revision, license, checksum, preparation version, split, and expected case count.
- Each profile selects one prepared root and cannot accidentally read another corpus under the same suite ID.

- [ ] Add fixture tests for checksum mismatch, wrong revision, unexpected row count, and deterministic normalized output. Preparation must fail before overwriting a previously valid corpus.
- [ ] Add one profile per corpus with exact manifest path, split, expected cases, metric gates or a documented diagnostic-only rationale, and required baseline policy.
- [ ] Add `make prepare-eval-corpora` using the existing CLI for each flat manifest, for example:

```bash
cargo run -p eval-harness --bin memory-eval -- prepare-corpus --manifest evals/corpora/longmemeval.json --output-root target/eval-corpora
```

Add one run target per profile. Network/download work remains an explicit preparation step; ordinary profile runs are offline and point `corpus_root` at `target/eval-corpora/<corpus_id>/<revision>`.
- [ ] Run all four profiles against small committed fixtures in tests. When real corpora are available, run all four full profiles and retain artifacts outside Git.
- [ ] Do not call these LongMemEval-V2 results; classic LongMemEval and LongMemEval-V2 are distinct evaluation tracks.
- [ ] Commit: `feat(eval): profile every pinned external corpus`.

---

### Task 7: Replace the LongMemEval-V2 no-op with a real pinned adapter

**Files:**
- Modify: `evals/longmemeval_v2/memory_mcp_backend.py`
- Modify: `evals/longmemeval_v2/run_pinned.sh`
- Add: `evals/longmemeval_v2/test_memory_mcp_backend.py`
- Add: `evals/longmemeval_v2/profile.json`
- Modify: `evals/longmemeval_v2/README.md`

**Interfaces:**
- `MemoryMcpBackend` subclasses upstream `Memory`, uses `@register_memory`, accepts `memory_params`, implements `insert(trajectory)` and returns `list[{"type":"text","value":...}]` from `query`.
- A real `--smoke-test` exercises argument parsing and adapter contracts; success cannot come from an empty module body.

- [ ] Add Python unit tests with an injected command runner. Assert ingest uses a supported source type (`other`), never passes obsolete `--scope`, parses the actual JSON episode ID, and query returns upstream text context items. Assert image queries fail with an explicit unsupported/text-only result.
- [ ] Configure a persistent RocksDB URL for subprocess calls so separate CLI processes share state. Do not guess `episode:{source_id}`.
- [ ] Implement `--smoke-test` using the fake runner and `--integration-smoke --binary PATH --db-path PATH` using the built CLI; both must perform insert then query and validate the returned schema.
- [ ] Make `run_pinned.sh` verify the upstream checkout is exactly `6f020ac2fc3275e46c706d3406e02c3ed79b7be2`, import the adapter so registration occurs, pass the pinned profile to the official launcher, and propagate every non-zero exit.
- [ ] Record domains/tier, memory-context token limit, reader configuration, selected IDs, upstream commit, dataset revision, and output path. Label reports `text-only`; exclude or explicitly mark image-query cases instead of silently scoring them as supported.
- [ ] Run:

```bash
python3 -m unittest evals.longmemeval_v2.test_memory_mcp_backend
python3 evals/longmemeval_v2/memory_mcp_backend.py --smoke-test
cargo build -p memory_mcp --features eval-support --locked
evals/longmemeval_v2/run_pinned.sh --integration-smoke target/debug/memory_mcp
```

- [ ] Only after both smokes pass, run the pinned upstream evaluation. Never publish a full LongMemEval-V2 comparison from the text-only adapter.
- [ ] Commit: `fix(eval): implement pinned LongMemEval-V2 adapter`.

---

### Task 8: Execute every Criterion benchmark on the right platform

**Files:**
- Modify: `Makefile`
- Modify: `.github/workflows/ci.yml`
- Add: `docs/evals/CRITERION_MATRIX.md`
- Modify: `crates/eval-harness/benches/pipeline.rs`
- Modify: `crates/eval-harness/benches/contention.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`

**Interfaces:**
- `bench-check` compiles every benchmark available on the current platform.
- `bench-cpu` runs pipeline, contention, and CPU NER; `bench-metal` runs Metal NER only on macOS arm64 with the required feature/assets.

- [ ] Add minimal deterministic smoke/sample-size configuration for CI without changing the documented measurement configuration used for publishable runs.
- [ ] Compile all benches on PR. Run CPU benches on schedule/manual dispatch and upload `target/criterion`; run Metal only on an Apple Silicon runner or via the exact documented manual command.
- [ ] Make unavailable hardware a declared prerequisite, not a green benchmark result. Never merge Criterion latency into retrieval-quality artifacts.
- [ ] Run on supported local hardware:

```bash
make bench-check
make bench-cpu
make bench-metal
```

If Metal is unavailable, `bench-metal` must exit with a clear prerequisite error and the coverage report must remain incomplete.
- [ ] Commit: `ci(eval): schedule Criterion benchmark matrix`.

---

### Task 9: Reconcile claims against fresh artifacts

**Files:**
- Modify: `docs/evals/README.md`
- Modify: affected reports under `docs/evals/`
- Modify: benchmark claims in `README.md`

**Interfaces:**
- Every numeric claim links to profile, artifact schema version, source commit, corpus revision, mode/device, and command.

- [ ] Run every row in the required execution matrix that the current machine supports. Record unsupported rows as incomplete with the exact missing prerequisite; do not mark them passed.
- [ ] Compare fresh results with existing documents. Remove or qualify stale, combined, unsupported, or differently scoped claims.
- [ ] Run final gates:

```bash
cargo fmt --all --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,control-plane --locked -- -D warnings
make eval-pr
make eval-release
make eval-nightly
make eval-response-size
make eval-ner-quality
git diff --check
git status --short
```

- [ ] Validate every new JSON artifact with the harness before using it as evidence. Confirm expected and observed `CaseKey` sets are equal.
- [ ] Commit: `docs(eval): reconcile claims with complete benchmark runs`.

## Completion Gate

This plan is complete only when every registered suite and every Criterion bench has an execution row; all built-in runnable profiles pass execution-integrity checks; manual/platform-specific rows have been genuinely run for affected claims or remain explicitly incomplete; and LongMemEval-V2 can no longer return success without exercising its adapter.

## Self-Review

- Profile integrity: Tasks 1-5.
- Classic academic corpora: Task 6.
- LongMemEval-V2 contract and pinning: Task 7.
- Criterion coverage: Task 8.
- Claim grounding: Task 9.
- No benchmark is deferred to an unowned backlog. Platform/data prerequisites may block a run, but they are explicit non-green states.
