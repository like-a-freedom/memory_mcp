# Evaluation Performance and Release Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace synthetic benchmark stubs, execute pinned external corpora, verify the 10/20-minute budgets, and make CI enforce the corrected evaluation contract.

**Architecture:** Corpus preparation resolves immutable revisions into verified bytes; retrieval-only evaluation imports real canonical facts and runs complete stable samples/shards. Criterion targets measure production operations with setup outside the timed region and exact runner fingerprints. CI keeps ordinary tests separate, uploads artifacts on every outcome, and gates PR/release only after corrected baselines and timing evidence are approved.

**Tech Stack:** Rust 2024, Tokio bounded concurrency, Criterion, Reqwest/rustls for preparation only, SurrealDB `DbClient`, GitHub Actions.

## Global Constraints

- Start only after Truth Layer and Suite Realism remediation.
- External evaluation performs no network access or corpus mutation.
- Source URLs resolve the exact manifest revision; mutable `main` URLs are prohibited.
- Retrieval-only setup persists canonical provenance-bearing facts and never runs extraction.
- Release covers complete declared external populations through deterministic shards.
- Criterion measures production code, not token counting or atomic micro-stubs.
- CPU, Metal, and contention remain separate benchmark families.
- Absolute performance gates run only on an exact pinned runner.
- PR target is at most 10 minutes; release target is at most 20 minutes.
- CI never uses `continue-on-error` for a promoted required gate.
- Artifacts upload on pass, quality failure, invalid execution, and timeout recovery.

---

## File Map

| Path | Responsibility |
|---|---|
| `crates/eval-harness/src/corpus/manifest.rs` | Immutable revision and license validation |
| `crates/eval-harness/src/corpus/prepare.rs` | Revision-aware corpus acquisition |
| `crates/eval-harness/src/corpus/adapters.rs` | Dataset-specific normalized cases and trust |
| `crates/eval-harness/src/adapters.rs` | Real canonical fact persistence |
| `crates/eval-harness/src/suites/external_retrieval.rs` | Stable sample/shard retrieval execution |
| `crates/eval-harness/src/benchmark.rs` | Benchmark fingerprints and gating |
| `crates/eval-harness/benches/pipeline.rs` | Production pipeline benchmarks |
| `crates/eval-harness/benches/ner_cpu.rs` | Real CPU NER |
| `crates/eval-harness/benches/ner_metal.rs` | Real Metal NER |
| `crates/eval-harness/benches/contention.rs` | Real shared-service contention |
| `evals/corpora/*.json` | Exact immutable source manifests |
| `evals/profiles/pr.json` | Stable stratified external sample |
| `evals/profiles/release.json` | Full external retrieval, lifecycle, performance |
| `evals/profiles/nightly.json` | Full end-to-end diagnostics |
| `evals/performance/pinned-runner.json` | Actual runner identity and budgets |
| `.github/workflows/ci.yml` | Enforced PR, release shard/merge, nightly jobs |
| `Makefile` | Thin profile adapters |

### Task 1: Make corpus preparation truly revision-pinned

**Files:**
- Modify: `crates/eval-harness/src/corpus/manifest.rs`
- Modify: `crates/eval-harness/src/corpus/prepare.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `evals/corpora/longmemeval.json`
- Modify: `evals/corpora/locomo.json`
- Modify: `evals/corpora/personamem.json`
- Modify: `evals/corpora/prefeval.json`
- Test: `crates/eval-harness/tests/corpus_revision.rs`

**Interfaces:**
- Produces: `ResolvedSource { url, revision, expected_sha256 }`.
- Produces: `CorpusFetcher::fetch(&self, source: &ResolvedSource)`.

- [ ] **Step 1: Write a failing mutable-source test**

```rust
#[test]
fn manifest_revision_must_affect_the_resolved_url() {
    let manifest = github_manifest(
        "https://raw.githubusercontent.com/org/repo/main/data.json",
        "0123456789abcdef0123456789abcdef01234567",
    );
    let source = manifest.resolve_source().unwrap();
    assert!(source.url.contains("0123456789abcdef0123456789abcdef01234567"));
    assert!(!source.url.contains("/main/"));
}
```

- [ ] **Step 2: Model provider-specific immutable sources**

Represent GitHub commit URLs and Hugging Face dataset revisions explicitly.
Reject symbolic revisions such as `main`, `master`, `latest`,
`personamem-32k`, and `prefeval-travel`.

- [ ] **Step 3: Use revision in the fetch request**

Remove the ignored `_revision` parameter. Resolve the exact immutable URL
before fetching, stream bytes to temporary storage, validate SHA-256, byte
size, normalized case count, adapter version, and license metadata, then
publish atomically.

- [ ] **Step 4: Correct every manifest from primary upstream evidence**

Record full commit/revision IDs and URLs containing that revision. For bundled
PersonaMem/PrefEval data, include every component file and digest in the
manifest rather than pointing one URL at a generated bundle.

- [ ] **Step 5: Run and commit**

Run:

```bash
cargo test -p eval-harness corpus --test corpus_revision
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/corpus crates/eval-harness/src/main.rs crates/eval-harness/tests/corpus_revision.rs evals/corpora
git commit -m "fix(evals): resolve immutable corpus revisions"
```

### Task 2: Persist canonical retrieval facts and register external suites

**Files:**
- Modify: `crates/eval-harness/src/adapters.rs`
- Modify: `crates/eval-harness/src/suites/external_retrieval.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Modify: `crates/eval-harness/src/profile.rs`
- Test: `crates/eval-harness/tests/external_retrieval_truth.rs`

**Interfaces:**
- Produces: `CanonicalFactImporter::import_context(&self, context: &CanonicalContext) -> Result<ImportedContext, EvalError>`.
- Produces: `ExternalRetrievalSuite::expected_case_ids()`.
- Consumes: validated `PreparedCorpus`, `Selection`, and `WorkerPolicy`.

- [ ] **Step 1: Write a failing persistence test**

```rust
#[tokio::test]
async fn canonical_import_is_visible_to_production_retrieval() {
    let fixture = canonical_context_fixture();
    let imported = importer().import_context(&fixture).await.unwrap();
    let items = imported.service.assemble_context(fixture.query()).await.unwrap();
    assert!(items.iter().any(|item| item.source_episode == fixture.episode_id()));
}
```

- [ ] **Step 2: Replace the current no-op importer**

The current `import_canonical_facts` only returns IDs. Implement production
schema persistence for episodes, facts, provenance, timestamps, lexical keys,
and precomputed embedding metadata through `DbClient`. Reject missing
embeddings when the selected retrieval configuration requires semantic ANN.

- [ ] **Step 3: Remove extraction-backed external seeding**

External retrieval must call only the canonical importer plus
`assemble_context`. It cannot use `seed_fact_with_links_and_project`,
`MemoryService::ingest`, `extract`, or `add_fact`.

- [ ] **Step 4: Return exact expected case IDs**

Store selected `CaseKey`s in `ExternalRetrievalSuite`; remove the empty
`expected_case_ids`. Task join errors and semaphore acquisition failures create
invalid outcomes instead of disappearing.

- [ ] **Step 5: Use both worker limits**

Group cases by canonical context. Bound context imports with
`context_workers`; within each imported context bound queries with
`query_workers_per_context`. Add an atomic test proving observed concurrency
never exceeds either value.

- [ ] **Step 6: Register datasets from profile declarations**

`main.rs` constructs external suites from profile corpus manifest, selection,
shard, worker policy, and trust slice. Unknown suite/corpus configuration is an
invalid profile, not a warning followed by omission.

- [ ] **Step 7: Run and commit**

Run:

```bash
cargo test -p eval-harness --test external_retrieval_truth
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --suite external-retrieval --artifact target/evals/external-sample.json
```

Commit:

```bash
git add crates/eval-harness/src/adapters.rs crates/eval-harness/src/suites/external_retrieval.rs crates/eval-harness/src/main.rs crates/eval-harness/src/profile.rs crates/eval-harness/tests/external_retrieval_truth.rs
git commit -m "feat(evals): execute canonical external retrieval"
```

### Task 3: Replace Criterion stubs with production benchmarks

**Files:**
- Create or modify: `crates/eval-harness/src/benchmark.rs`
- Modify: `crates/eval-harness/benches/pipeline.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`
- Modify: `crates/eval-harness/benches/contention.rs`
- Modify: `crates/eval-harness/Cargo.toml`
- Modify: `docs/performance/NER_PERFORMANCE.md`

**Interfaces:**
- Produces: `BenchmarkFingerprint::capture()`.
- Produces: Criterion groups `pipeline`, `ner_cpu`, `ner_metal`, `contention`.

- [ ] **Step 1: Add benchmark smoke assertions**

```rust
#[test]
fn ner_bench_invokes_the_real_extractor() {
    let probe = RecordingEntityExtractor::new();
    run_ner_iteration(&probe, "Alice joined OpenAI").unwrap();
    assert_eq!(probe.extract_calls(), 1);
}

#[test]
fn retrieval_setup_contains_searchable_facts_before_timing() {
    let fixture = build_retrieval_bench_fixture().unwrap();
    assert!(fixture.persisted_fact_count() >= 10);
}
```

- [ ] **Step 2: Configure Criterion explicitly**

Set 5-second warm-up, 30-second measurement, at least 30 samples, 95%
confidence, throughput, and `black_box`. Write raw Criterion statistics and
fingerprint metadata together.

- [ ] **Step 3: Correct pipeline timing boundaries**

Ingest measures ingest on a prepared service; extraction measures extraction
with episode setup outside the timed closure; retrieval measures only
`assemble_context` over persisted searchable facts; end-to-end measures all
production stages. Do not create a database/model inside a single-stage timed
iteration.

- [ ] **Step 4: Invoke real CPU and Metal NER**

Construct the production `EntityExtractor` with pinned model digest,
threshold, token cap, and device. The Metal target requires `metal` and becomes
invalid if Metal cannot initialize; it never runs token counting under a Metal
name.

- [ ] **Step 5: Benchmark real service contention**

Share the declared service/database/model resource across synchronized clients,
use a barrier and bounded Tokio tasks, and measure request latency plus
throughput. Remove the atomic counter and thread-spawn microbenchmark.

- [ ] **Step 6: Verify fingerprints before absolute gates**

Compare OS image, arch, CPU model, memory, Rust version, build flags, features,
provider, model digest, device, Criterion settings, and power policy with
`pinned-runner.json`. Mismatch produces diagnostic output, not a gated pass.

- [ ] **Step 7: Run and commit**

Run:

```bash
cargo test -p eval-harness benchmark
cargo bench -p eval-harness --bench pipeline -- --noplot
cargo bench -p eval-harness --bench ner_cpu -- --noplot
cargo bench -p eval-harness --bench contention -- --noplot
cargo bench -p eval-harness --features metal --bench ner_metal -- --noplot
```

Commit:

```bash
git add crates/eval-harness/src/benchmark.rs crates/eval-harness/benches crates/eval-harness/Cargo.toml docs/performance/NER_PERFORMANCE.md
git commit -m "perf(evals): benchmark production workloads"
```

### Task 4: Declare honest PR, release, and nightly profiles

**Files:**
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`
- Modify: `evals/profiles/nightly.json`
- Modify: `evals/performance/pinned-runner.json`
- Modify: `Makefile`

**Interfaces:**
- Consumes: exact fixture/corpus coverage, qualified gates, worker policies.
- Produces: complete profile manifests with no implicit suites.

- [ ] **Step 1: Add profile contract tests**

```rust
#[test]
fn release_contains_full_external_retrieval_and_lifecycle() {
    let profile = load_real_profile(EvalProfile::Release).unwrap();
    assert!(profile.has_full_corpus_suite("external-retrieval"));
    assert!(profile.has_suite("lifecycle"));
    assert_eq!(profile.time_budget_seconds, 1200);
}
```

- [ ] **Step 2: Define the PR profile**

Select deterministic local suites and a stable stratified external sample with
exact case IDs/selection fingerprint. Target 600 seconds. Use corrected hard
floors and a reviewed baseline only after the corrected run.

- [ ] **Step 3: Define the release profile**

Select complete LongMemEval, LoCoMo, PersonaMem, and PrefEval retrieval-only
populations through deterministic shards, wired lifecycle, and pinned
performance gates. Target 1,200 seconds on the declared release runner.

- [ ] **Step 4: Define the nightly profile**

Select full end-to-end corpora and configured diagnostics. Keep downstream QA
absent unless its complete reader contract is present. Establish its budget
from measured evidence rather than assuming 3,600 seconds is achieved.

- [ ] **Step 5: Keep Make thin**

Make targets pass profile, artifact, corpus root, and optional approved
baseline only. They contain no suite lists, thresholds, or concurrency values.

- [ ] **Step 6: Run and commit**

Run:

```bash
cargo test -p eval-harness profile
make eval-pr
make eval-release
make eval-nightly
```

Commit:

```bash
git add evals/profiles evals/performance/pinned-runner.json Makefile
git commit -m "build(evals): declare complete evaluation profiles"
```

### Task 5: Enforce corrected evals in CI and approve baselines

**Files:**
- Modify: `.github/workflows/ci.yml`
- Create: `evals/baselines/pr.json`
- Create: `evals/baselines/release.json`
- Create: `docs/evals/BASELINE_GOVERNANCE.md`
- Generate after execution: new corrected benchmark report under `docs/evals/`

**Interfaces:**
- Produces: required PR job, release shard/merge job, scheduled nightly job,
  pinned performance job.
- Consumes: artifact v2 and approved compatible baselines.

- [ ] **Step 1: Add artifact-on-failure CI tests**

Use a CI script test that runs a deliberately failing fixture profile, asserts
non-zero exit, and validates that artifact v2 still exists and contains the
failure.

- [ ] **Step 2: Remove `continue-on-error` after promotion criteria**

Keep evaluation advisory only while truth remediation is incomplete. Promote
PR to required after two consecutive representative runs have exact coverage,
no invalid cases, approved baseline compatibility, and duration at or below
600 seconds.

- [ ] **Step 3: Add release shard and merge jobs**

Each shard always uploads its artifact. Merge downloads every declared shard,
validates union/no overlap and fingerprints, recomputes summaries/gates, and
uploads the full release artifact. Only the merge job claims the release gate.

- [ ] **Step 4: Add scheduled nightly and pinned performance**

Nightly always uploads the full end-to-end artifact. Performance applies
absolute gates only on the exact pinned runner and uploads Criterion raw
statistics plus fingerprint metadata.

- [ ] **Step 5: Approve baselines from corrected evidence**

Review case-level failures, evaluator versions, fixture/corpus fingerprints,
environment, gates, and duration. Commit a baseline only after the corresponding
hard floors pass or an explicitly reviewed product decision changes them in a
separate commit.

- [ ] **Step 6: Generate the corrected report**

Use `memory-eval report` over PR, merged release, nightly, and performance
artifacts. The report must explicitly compare with the 2026-07-28 diagnostic
run and retract unsupported conclusions rather than overwriting history.

- [ ] **Step 7: Run the full quality gate and commit**

Run:

```bash
cargo check --workspace
cargo test --workspace --lib --bins --tests
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
make eval-pr
make eval-release
```

Commit:

```bash
git add .github/workflows/ci.yml evals/baselines docs/evals
git commit -m "ci(evals): enforce corrected benchmark gates"
```

## Completion Evidence

- every corpus URL resolves an immutable revision and validates exact bytes;
- external retrieval persists canonical facts and has exact expected coverage;
- PR completes in at most 10 minutes;
- merged full release completes in at most 20 minutes;
- Criterion invokes real production ingest/extract/retrieval/NER/contention;
- performance gates run only under an exact pinned fingerprint;
- CI preserves artifacts on all outcomes and no required gate is ignored;
- reviewed PR/release baselines contain real case outcomes and compatible fingerprints.

