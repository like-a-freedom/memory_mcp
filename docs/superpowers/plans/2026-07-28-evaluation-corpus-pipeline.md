# Evaluation Corpus Pipeline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Make every external evaluation reproducible and fast by pinning corpus bytes, validating provenance offline, using stable stratified samples and shards, and running full retrieval-only corpora without extraction shortcuts.

**Architecture:** Extend `eval-harness` with an immutable corpus manifest boundary, an explicit preparation command, dataset-specific adapters behind one trait, deterministic selection, and a private canonical-fact importer. Preparation may access the network; evaluation never does. Release shards operate on complete declared coverage and merge only when all provenance and configuration fingerprints agree.

**Tech Stack:** Rust 2024, Serde/serde_json, SHA-256, Reqwest with rustls for the preparation binary path, Tokio bounded concurrency, SurrealDB through the existing `DbClient`, existing LongMemEval/LoCoMo/PersonaMem/PrefEval fixtures.

## Global Constraints

- This plan starts only after the Evaluation Foundation completion evidence is approved.
- Evaluation never downloads or mutates corpus data.
- Preparation pins an immutable revision and verifies SHA-256 before publishing a prepared corpus.
- Every manifest records URL, revision, SHA-256, license, byte size, case count, and adapter version.
- Missing or mismatched corpus evidence creates invalid outcomes and a failed release run.
- Sampling is stable, stratified, and based on corpus identity plus case ID; prefix sampling is prohibited.
- Shard union must equal complete declared release coverage with no duplicates.
- Label trust is exactly `official`, `reviewed`, or `weak`; weak labels never enter release gates.
- Retrieval-only imports canonical provenance-bearing facts and does not run ingest, extraction, claim projection, or direct oracle insertion through `add_fact`.
- Dataset-specific official metrics and slices are preserved.
- LongMemEval v2 remains outside the current release profile.
- Every task uses TDD, targeted checks, `cargo fmt --all --check`, and a focused commit.

---

## File Map

| Path | Responsibility |
|---|---|
| `crates/eval-harness/src/corpus.rs` | Corpus trait and shared normalized types |
| `crates/eval-harness/src/corpus/manifest.rs` | Strict manifest parsing and byte validation |
| `crates/eval-harness/src/corpus/selection.rs` | Stable stratification and sharding |
| `crates/eval-harness/src/corpus/longmemeval.rs` | LongMemEval adapter |
| `crates/eval-harness/src/corpus/locomo.rs` | LoCoMo adapter |
| `crates/eval-harness/src/corpus/personamem.rs` | PersonaMem adapter |
| `crates/eval-harness/src/corpus/prefeval.rs` | PrefEval adapter |
| `crates/eval-harness/src/prepare.rs` | Fetch, verify, stage, and atomically publish corpus bytes |
| `crates/eval-harness/src/adapters.rs` | Evaluation adapter module declarations |
| `crates/eval-harness/src/adapters/canonical_fact.rs` | Private prebuilt-fact import |
| `crates/eval-harness/src/suites/external_retrieval.rs` | Full retrieval-only suite |
| `evals/corpora/*.json` | Immutable corpus manifests |
| `evals/profiles/release.json` | Full retrieval-only release declaration |
| `evals/profiles/pr.json` | Stable stratified external sample |
| `tests/eval_support/external.rs` | Compatibility re-export during migration |
| `tests/eval_support/external_full.rs` | Removed after loader parity |
| `tests/eval_external_retrieval.rs` | Thin compatibility launcher |
| `scripts/convert_external_evals.py` | Removed after Rust preparation parity |
| `tests/fixtures/evals/raw/README.md` | Preparation and license documentation |

### Task 1: Define strict corpus manifests and offline validation

**Files:**
- Create: `crates/eval-harness/src/corpus.rs`
- Create: `crates/eval-harness/src/corpus/manifest.rs`
- Create: `evals/corpora/longmemeval.json`
- Create: `evals/corpora/locomo.json`
- Create: `evals/corpora/personamem.json`
- Create: `evals/corpora/prefeval.json`

**Interfaces:**
- Produces: `CorpusId`, `CorpusManifest`, `PreparedCorpus`, `CorpusValidation`.
- Produces: `CorpusManifest::validate_at(&self, root: &Path) -> Result<PreparedCorpus, EvalError>`.
- Consumes: exact bytes from a prepared corpus directory.

- [x] **Step 1: Write failing manifest validation tests**

```rust
#[test]
fn digest_mismatch_invalidates_the_corpus() {
    let fixture = PreparedFixture::new(b"actual");
    let manifest = manifest_with_sha256("0".repeat(64));
    let error = manifest.validate_at(fixture.root()).unwrap_err();
    assert!(error.to_string().contains("sha-256 mismatch"));
}

#[test]
fn unknown_manifest_fields_are_rejected() {
    let raw = valid_manifest_json().replace("\"license\"", "\"unexpected\":1,\"license\"");
    assert!(CorpusManifest::parse(&raw).is_err());
}
```

- [x] **Step 2: Implement validated manifest types**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorpusManifest {
    pub schema_version: String,
    pub corpus_id: CorpusId,
    pub source_url: String,
    pub revision: String,
    pub sha256: String,
    pub license: String,
    pub byte_size: u64,
    pub case_count: usize,
    pub adapter_version: String,
    pub data_file: PathBuf,
}
```

Validate a 64-character lowercase hex digest, non-empty immutable revision,
non-empty license, positive size/count, relative `data_file` without parent
traversal, and schema `memory-mcp-corpus/v1`.

- [x] **Step 3: Validate bytes before parsing cases**

Open with `BufReader`, stream SHA-256, compare byte size and digest, then return
`PreparedCorpus`. Do not expose raw unchecked paths to adapters.

- [x] **Step 4: Populate manifests from exact upstream revisions**

For each corpus, record the official immutable commit or dataset revision,
official URL, published license, computed digest, exact size, exact normalized
case count, and adapter version `1`. Do not copy mutable `main` URLs.

- [x] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p eval-harness corpus::manifest
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/corpus.rs crates/eval-harness/src/corpus evals/corpora
git commit -m "feat(evals): validate immutable corpus manifests"
```

### Task 2: Add the explicit corpus preparation command

**Files:**
- Create: `crates/eval-harness/src/prepare.rs`
- Modify: `crates/eval-harness/src/cli.rs`
- Modify: `crates/eval-harness/src/main.rs`
- Test: `crates/eval-harness/tests/prepare_corpus.rs`

**Interfaces:**
- Produces: `memory-eval prepare-corpus --manifest <path> --output-root <path>`.
- Produces: `prepare_corpus(manifest: &CorpusManifest, output_root: &Path, fetcher: &dyn CorpusFetcher) -> Result<PreparedCorpus, EvalError>`.
- Produces: `CorpusFetcher::fetch(&self, source_url: &str, revision: &str)`.

- [x] **Step 1: Write a failing no-publication-on-mismatch test**

```rust
#[tokio::test]
async fn bad_download_is_never_published() {
    let output = tempfile::tempdir().unwrap();
    let result = prepare_corpus(
        &manifest_with_sha256(sha256(b"expected")),
        output.path(),
        &FakeFetcher::returning(b"different"),
    ).await;
    assert!(result.is_err());
    assert!(!output.path().join("corpus/data.json").exists());
}
```

- [x] **Step 2: Implement fetch-to-temporary preparation**

Fetch into a uniquely named temporary directory under `output_root`, stream the
digest, validate size/digest, parse through the declared adapter to validate
case count, write a copy of the manifest, `sync_all`, and atomically rename the
directory to `<corpus-id>/<revision>`.

- [x] **Step 3: Keep network code outside evaluation**

Only `prepare-corpus` constructs `ReqwestCorpusFetcher`. The `run` command
accepts `--corpus-root` and calls `validate_at`; it has no fetcher, URL client,
or network fallback.

- [x] **Step 4: Add repeatability and conflict tests**

An already prepared matching corpus succeeds without rewriting bytes. An
existing path with mismatched bytes returns an error and preserves the existing
directory.

- [x] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p eval-harness --test prepare_corpus
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src crates/eval-harness/tests
git commit -m "feat(evals): prepare pinned corpora explicitly"
```

### Task 3: Move dataset adapters behind one typed seam

**Files:**
- Create: `crates/eval-harness/src/corpus/longmemeval.rs`
- Create: `crates/eval-harness/src/corpus/locomo.rs`
- Create: `crates/eval-harness/src/corpus/personamem.rs`
- Create: `crates/eval-harness/src/corpus/prefeval.rs`
- Modify: `crates/eval-harness/src/corpus.rs`
- Modify: `tests/eval_support/external.rs`

**Interfaces:**
- Produces: `CorpusAdapter::load(&self, prepared: &PreparedCorpus) -> Result<Vec<ExternalCase>, EvalError>`.
- Produces: `ExternalCase { id, stratum, scope, facts, query, expectation, split, label_trust, metadata }`.
- Produces: `CanonicalFact { fact_id, episode_id, scope, project, content, quote, fact_type, t_valid, t_ingested, provenance, embedding }`.

- [x] **Step 1: Define the adapter contract and invariant tests**

```rust
pub trait CorpusAdapter: Send + Sync {
    fn corpus_id(&self) -> &CorpusId;
    fn version(&self) -> &str;
    fn load(&self, prepared: &PreparedCorpus) -> Result<Vec<ExternalCase>, EvalError>;
}

#[test]
fn every_case_has_a_stable_id_and_nonempty_stratum() {
    for case in adapter_fixture_cases() {
        assert!(!case.id.as_str().is_empty());
        assert!(!case.stratum.trim().is_empty());
    }
}
```

- [x] **Step 2: Port LongMemEval and LoCoMo parsing**

Preserve official question IDs and evidence IDs. Convert all timestamps
fallibly. Derive no weak expected snippet when official evidence IDs exist.
Map question families to explicit strata without changing official metrics.

- [x] **Step 3: Port PersonaMem and PrefEval parsing**

Keep official answer labels as downstream-QA metadata. For retrieval
expectations, mark human-reviewed mappings `reviewed` and heuristic overlap
snippets `weak`. Never silently promote an inferred snippet to `official`.

- [x] **Step 4: Add golden normalized-case tests**

For one real fixture case per corpus, compare the complete normalized
`ExternalCase` JSON against a checked golden value including IDs, stratum,
trust, timestamps, provenance, and expected evidence IDs.

- [x] **Step 5: Delegate compatibility imports**

Make `tests/eval_support/external.rs` re-export or call harness adapters until
the old external runner is removed. Delete duplicated parsers only after golden
parity passes.

- [x] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p eval-harness corpus
cargo test --test eval_external_retrieval normalizes_
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/corpus tests/eval_support/external.rs
git commit -m "refactor(evals): isolate external corpus adapters"
```

### Task 4: Implement stable stratified sampling and complete sharding

**Files:**
- Create: `crates/eval-harness/src/corpus/selection.rs`
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`

**Interfaces:**
- Produces: `select_sample(cases: &[ExternalCase], request: &SampleRequest) -> Result<Selection, EvalError>`.
- Produces: `select_shard(cases: &[ExternalCase], shard: ShardSpec) -> Result<Selection, EvalError>`.
- Produces: `Selection { case_ids, strata, population_count, selected_count, fingerprint }`.

- [x] **Step 1: Write property tests for order independence**

```rust
proptest! {
    #[test]
    fn sample_is_independent_of_input_order(mut cases in external_case_vec()) {
        let first = select_sample(&cases, &sample_request()).unwrap();
        cases.reverse();
        let second = select_sample(&cases, &sample_request()).unwrap();
        prop_assert_eq!(first.case_ids, second.case_ids);
    }
}
```

- [x] **Step 2: Implement stable selection**

Compute `sha256(corpus_fingerprint || "\0" || case_id)`, sort within each
stratum by digest then case ID, and take the declared count per stratum. Reject
missing strata or a requested count larger than the stratum.

- [x] **Step 3: Implement stable shards**

Assign `u64::from_be_bytes(digest[0..8]) % shard_count == shard_index`.
Validate `shard_count > 0` and `shard_index < shard_count`. Record population,
selection, and exact IDs.

- [x] **Step 4: Prove shard completeness**

Add a property test that unions all shards for counts 1 through 16, asserts the
union equals all case IDs, and asserts pairwise intersections are empty.

- [x] **Step 5: Replace percentage/prefix configuration**

Remove `MEMORY_MCP_EVAL_SAMPLE_PCT` and `MEMORY_MCP_EVAL_MAX_CASES` from the
new runner. The PR profile declares exact per-corpus stratum counts. The
release profile declares full coverage plus optional shard parameters.

- [x] **Step 6: Run tests and commit**

Run:

```bash
cargo test -p eval-harness corpus::selection
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src/corpus/selection.rs evals/profiles
git commit -m "feat(evals): add stable sampling and sharding"
```

### Task 5: Add the private canonical-fact importer

**Files:**
- Create: `crates/eval-harness/src/adapters.rs`
- Create: `crates/eval-harness/src/adapters/canonical_fact.rs`
- Modify: `crates/eval-harness/src/lib.rs`
- Test: `crates/eval-harness/tests/canonical_fact_import.rs`

**Interfaces:**
- Produces: `CanonicalFactImporter::import_context(&self, context: &CanonicalContext) -> Result<ImportedContext, EvalError>`.
- Consumes: `Arc<dyn DbClient>` and precomputed `CanonicalFact` records.
- Produces: stable fact and episode IDs with production-compatible schema and provenance.

- [x] **Step 1: Write a failing no-extraction/no-generation integration test**

```rust
#[tokio::test]
async fn import_preserves_precomputed_embedding_without_extraction() {
    let context = canonical_context_with_embedding(vec![0.25; 384]);
    let imported = importer().import_context(&context).await.unwrap();
    let record = imported.db.select_one(&context.facts[0].fact_id, "memory").await.unwrap();
    assert_eq!(record["embedding"], serde_json::json!(vec![0.25; 384]));
    assert_eq!(imported.operations, vec![ImportOperation::Episode, ImportOperation::Fact]);
}
```

- [x] **Step 2: Implement exact schema insertion**

Use `DbClient::create` with explicit episode and fact records matching the
current storage schema. Require precomputed embedding, embedding model,
provider, dimension, and content hash. Reject missing provenance, invalid
timestamps, duplicate IDs with different content, or dimension mismatch.

- [x] **Step 3: Keep the adapter private to eval-harness**

Do not add a public `memory_mcp` capability, MCP tool, CLI command, or
production feature. The importer is `pub(crate)` except for integration-test
helpers. It cannot call `MemoryService::ingest`, `extract`, or `add_fact`.

- [x] **Step 4: Verify production retrieval sees imported facts**

Construct `MemoryService` over the same `DbClient`, issue
`assemble_context`, and assert the imported fact is returned with its original
source episode and timestamps.

- [x] **Step 5: Run tests and commit**

Run:

```bash
cargo test -p eval-harness --test canonical_fact_import
cargo clippy -p eval-harness --all-targets -- -D warnings
cargo fmt --all --check
```

Commit:

```bash
git add crates/eval-harness/src/adapters.rs crates/eval-harness/src/adapters crates/eval-harness/tests
git commit -m "feat(evals): import canonical retrieval facts privately"
```

### Task 6: Run external retrieval with bounded context and query workers

**Files:**
- Create: `crates/eval-harness/src/suites/external_retrieval.rs`
- Modify: `crates/eval-harness/src/suites.rs`
- Modify: `crates/eval-harness/src/profile.rs`
- Modify: `evals/profiles/pr.json`
- Modify: `evals/profiles/release.json`

**Interfaces:**
- Produces: `ExternalRetrievalSuite`.
- Consumes: `Selection`, `CanonicalFactImporter`, `WorkerPolicy`.
- Produces: deterministic per-case outcomes and per-corpus official/reviewed/weak slices.

- [x] **Step 1: Write a failing bounded-concurrency test**

Use a fake adapter with atomics and assert that 20 contexts complete while
`max_observed_context_workers <= 3` and
`max_observed_query_workers_per_context <= 4`.

- [x] **Step 2: Add validated worker policy**

```rust
pub struct WorkerPolicy {
    pub context_workers: NonZeroUsize,
    pub query_workers_per_context: NonZeroUsize,
}
```

Load values from the profile, not strictness or free-form environment
variables. Record effective values in the artifact.

- [x] **Step 3: Implement structured concurrency**

Use `tokio::sync::Semaphore` for both worker levels and `JoinSet` to own spawned
tasks. Convert task errors and timeouts into invalid outcomes, await all tasks,
and sort outcomes before reporting. Never hold a semaphore-owned resource or
mutex guard beyond its intended async operation.

- [x] **Step 4: Keep label-trust slices separate**

Compute release gates from official and reviewed cases only. Emit weak-label
metrics under a distinct slice and assert in a unit test that adding perfect
weak cases cannot improve the gated metric.

- [x] **Step 5: Add complete release coverage**

`release.json` declares all four corpora at full coverage. If sharded, each
artifact records shard index/count and expected IDs under a typed
`RunCompleteness::Shard` field; its case outcomes still use only the three Truth
Contract statuses. A shard cannot claim the merged release gate.

- [x] **Step 6: Run sample and one release shard**

Run:

```bash
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --artifact target/evals/pr-external.json
cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/release.json --shard 0/4 --artifact target/evals/release-0-of-4.json
```

Expected: exact selected IDs and worker limits appear in both artifacts.

- [x] **Step 7: Commit**

```bash
git add crates/eval-harness/src/suites evals/profiles
git commit -m "perf(evals): run external retrieval with bounded workers"
```

### Task 7: Merge shards only under identical provenance

**Files:**
- Create: `crates/eval-harness/src/merge.rs`
- Modify: `crates/eval-harness/src/cli.rs`
- Modify: `crates/eval-harness/src/artifact.rs`
- Test: `crates/eval-harness/tests/merge_shards.rs`

**Interfaces:**
- Produces: `memory-eval merge --profile <path> --artifact <path> <shard-artifacts...>`.
- Produces: `merge_shards(artifacts: &[RunArtifact], profile: &ProfileManifest) -> Result<RunArtifact, EvalError>`.

- [x] **Step 1: Write failing incompatible-shard tests**

Reject a merge when corpus digest, adapter version, evaluator version,
configuration hash, profile digest, build/features, model/provider/device, or
shard count differs.

- [x] **Step 2: Implement exact coverage merge**

Require each shard index exactly once. Reject duplicate case IDs, missing
expected IDs, or unexpected IDs. Sort outcomes, recompute summaries and gates
from case outcomes, and never average shard-level percentages.

- [x] **Step 3: Add a successful four-shard test**

Build four synthetic shard artifacts, merge them, and assert case union,
denominators, metrics, and gate decisions match a single unsharded run.

- [x] **Step 4: Run tests and commit**

Run:

```bash
cargo test -p eval-harness --test merge_shards
cargo fmt --all --check
cargo clippy -p eval-harness --all-targets -- -D warnings
```

Commit:

```bash
git add crates/eval-harness/src/merge.rs crates/eval-harness/src/cli.rs crates/eval-harness/src/artifact.rs crates/eval-harness/tests
git commit -m "feat(evals): merge complete compatible shards"
```

### Task 8: Remove hybrid seeding and legacy corpus orchestration

**Files:**
- Modify: `tests/eval_external_retrieval.rs`
- Modify: `tests/common/mod.rs`
- Delete: `tests/eval_support/external_full.rs`
- Delete: `scripts/convert_external_evals.py`
- Modify: `tests/eval_support/mod.rs`
- Modify: `tests/fixtures/evals/raw/README.md`
- Modify: `Makefile`

**Interfaces:**
- Consumes: `memory-eval prepare-corpus`, `run`, and `merge`.
- Removes: prefix sampling, runtime source verification, extraction-plus-`add_fact` hybrid seeding.

- [x] **Step 1: Point the compatibility runner at the harness**

Keep old test names only long enough for documented commands to delegate to a
single-suite harness run. The test must fail on invalid corpus or gate failure.

- [x] **Step 2: Remove hybrid seeding**

Delete `seed_episode_backed_fact_with_source_id` only after structural search
confirms no non-eval test uses it. If ordinary tests still require an
episode-backed helper, keep a clearly named test fixture helper but forbid it
from external retrieval.

- [x] **Step 3: Remove Python downloader and prefix loader**

Delete the Python conversion path after prepared-corpus parity is recorded.
Update the README with exact `memory-eval prepare-corpus` commands, manifest
location, license obligations, storage location, and offline-run guarantee.

- [x] **Step 4: Make the release Make target thin**

Add `eval-release` that calls `memory-eval run` for an unsharded local run or
documents the CI shard/merge workflow. Do not add suite lists or thresholds to
Make.

- [x] **Step 5: Verify complete corpus behavior**

Run:

```bash
cargo test --workspace --lib --bins --tests
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
make eval-pr
```

On the release runner, execute all declared shards and merge them. Expected:
the merged selected-ID set equals every manifest case exactly once, no network
access occurs during `run`, and full retrieval-only wall time is at most
20 minutes.

- [x] **Step 6: Commit**

```bash
git add crates/eval-harness tests Makefile tests/fixtures/evals/raw/README.md
git add -u scripts/convert_external_evals.py
git commit -m "refactor(evals): retire mutable corpus orchestration"
```

## Completion Evidence

Before starting realistic/lifecycle/performance migration, attach:

- all four validated manifests and exact preparation commands;
- PR sample IDs by corpus and stratum;
- shard-union proof for the release population;
- merged `release` JSON artifact;
- official/reviewed/weak metric slices;
- proof that `memory-eval run` performs no corpus network access;
- per-stage and per-corpus timing showing whether the 20-minute target is met
  without reduced coverage.
