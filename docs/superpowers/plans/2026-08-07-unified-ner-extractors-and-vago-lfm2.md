# Unified NER Extractors and Native VAGO LFM2 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the provider/model NER configuration with one typed extractor catalog, preserve download-free lightweight Anno as the zero-config default, and add shared model lifecycle support, explicit Anno NuNER ONNX, and a native Candle backend for `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.

**Architecture:** Keep `EntityExtractor` as the caller-facing capability and `backend_registry()` as the sole configured dispatch point. Put configuration validity in a discriminated `NerExtractorConfig`, artifact acquisition in a backend-neutral `NerArtifactStore`, in-memory retention in a narrow `LoadedModel<T>`, and architecture-specific inference in separate classic GLiNER, NuNER ONNX, and LFM2 modules. Model backends receive prepared local checkpoints and never perform network access.

**Tech Stack:** Rust 2024, MSRV 1.88, Tokio, Candle pinned at `21cca0b`, `hf-hub` 0.5, `tokenizers` 0.23, Anno 0.11 with its `onnx` feature, ONNX Runtime through Anno, serde/serde_json, SHA-256, tempfile, SurrealDB 3.0, and existing evaluation/CI infrastructure.

## Global Constraints

- ADR-0036 is the decision source; ADR-0029 remains the backend-registry seam; ADR-0035 is amended by the shared loaded-model lifecycle.
- `NER_EXTRACTOR` is the only public NER selector: unset/`anno`, `regex`, `anno-onnx`, `urchade/gliner_multi-v2.1`, or `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`.
- Unset `NER_EXTRACTOR` must remain local, offline, download-free lightweight Anno.
- Reject arbitrary repository IDs, unknown aliases, irrelevant settings, and removed environment variables; never silently ignore or fall back.
- Removed variables are `NER_PROVIDER`, `NER_MODEL`, `NER_MODEL_DIR`, `NER_BATCH_SIZE`, `NER_MAX_BATCH_TOKENS`, `NER_DEVICE`, and `GLINER_IDLE_UNLOAD_SECS`.
- Canonical variables are `NER_EXTRACTOR`, `NER_CACHE_DIR`, `NER_LABELS`, `NER_THRESHOLD`, `NER_MAX_CONCURRENCY`, `NER_IDLE_UNLOAD_SECS`, `GLINER_BATCH_SIZE`, `GLINER_MAX_BATCH_TOKENS`, and `GLINER_DEVICE`.
- The ordinary artifact includes every extractor; Cargo features do not select the normal extractor experience.
- The Anno ONNX/ONNX Runtime dependency change is approved by ADR-0036. Do not add any other dependency without approval.
- Do not add an MCP tool. MCP stdout remains JSON-RPC only; model progress uses stderr.
- Do not put business logic in `main.rs`; configuration belongs in `src/config/`, lifecycle and backend logic in `src/service/`.
- Do not expose Hugging Face or SurrealDB internals through the MCP surface.
- Do not use `unwrap()` in production code.
- Do not modify an existing migration. Task 11 creates one new append-only migration and requires explicit user confirmation immediately before execution, per `AGENTS.md`.
- VAGO supports/evaluates Russian, English, and mixed RU/EN. Other languages are best-effort with no detector or rejection path.
- VAGO loads upstream `pytorch_model.bin` directly with `VarBuilder::from_pth`, runs F32, and does not create derived safetensors.
- `GLINER_DEVICE=auto` may fall back from Metal to CPU with an explicit event; explicit `metal` fails.
- Structural parity is exact; confidence tolerance is absolute `1e-4`.
- Resolve upstream HEAD on each model-backed startup using two attempts under a ten-second total deadline. Downloads fail after 60 seconds without byte progress, not after a total wall-clock duration.
- Retain active plus one previous known-good revision. Never evict active. A known-incompatible commit is not retried until HEAD changes or its failure record is cleared.
- `NER_IDLE_UNLOAD_SECS=0` retains loaded state; positive values apply to all model-backed extractors.
- Preserve unrelated working-tree changes.

---

## File Map

| File | Responsibility |
|---|---|
| `crates/memory-mcp/src/config/ner.rs` | Closed selector parsing, typed variants, normalization, applicability and migration errors. |
| `crates/memory-mcp/src/config/constants.rs` | Canonical defaults only. |
| `crates/memory-mcp/src/config.rs` | Re-export typed NER configuration. |
| `crates/memory-mcp/src/config/surreal.rs` | Environment isolation and top-level zero-config assertions. |
| `crates/memory-mcp/src/service/entity_extraction.rs` | Stable `EntityExtractor`, extractor identity, registry, and build context. |
| `crates/memory-mcp/src/service/model_runtime.rs` | Shared `LoadedModel<T>` and `InferenceGate`; no artifact/network logic. |
| `crates/memory-mcp/src/service/model_artifacts.rs` | Artifact-store public contract and orchestration. |
| `crates/memory-mcp/src/service/model_artifacts/{manifest,progress,lease,state,download}.rs` | Focused artifact lifecycle responsibilities. |
| `crates/memory-mcp/src/service/entity_extraction/anno.rs` | Explicit lightweight/default Anno path only. |
| `crates/memory-mcp/src/service/entity_extraction/anno_onnx.rs` | CPU-only explicit NuNER ONNX backend. |
| `crates/memory-mcp/src/service/entity_extraction/gliner.rs` | Classic DeBERTa GLiNER consuming a prepared checkpoint. |
| `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs` | VAGO extractor composition and lifecycle. |
| `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/{config,model,tensors,decode}.rs` | LFM2 parsing, model layers, tensor mapping, and exact span decoding. |
| `crates/memory-mcp/src/service/core/builder.rs` | Construct extractor once and inject it into `MemoryService`. |
| `crates/memory-mcp/src/service/episode/entity_extraction.rs` | Persist extractor fingerprint with new extraction projections. |
| `crates/memory-mcp/migrations/029_entity_extraction_projection.surql` | Append-only projection metadata table. |
| `crates/memory-mcp/src/storage/migrations.rs` | Register migration 029. |
| `crates/memory-mcp/tests/{zero_config_embedded,local_model_integration,ner_model_lifecycle}.rs` | Offline, model-backed, lifecycle, and parity integration gates. |
| `crates/eval-harness/benches/{ner_cpu,ner_metal}.rs` | Typed extractor benchmarks. |
| `evals/corpora/ner/{vago_release_parity,vago_runtime_regression}.json` | RU/EN/mixed expected spans and scores. |
| `Cargo.toml`, `crates/memory-mcp/Cargo.toml`, `Cargo.lock` | Enable approved Anno ONNX support. |
| `.github/workflows/ci.yml` | Cross-platform ordinary-artifact smoke and packaging gates. |
| `README.md`, `docs/agent/REPOSITORY_LAYOUT.md`, `docs/agent/MCP_TOOLS.md` | Canonical runtime documentation. |

---

### Task 1: Introduce the typed extractor configuration

**Files:**
- Modify: `crates/memory-mcp/src/config/ner.rs`
- Modify: `crates/memory-mcp/src/config/constants.rs`
- Modify: `crates/memory-mcp/src/config.rs`
- Modify: `crates/memory-mcp/src/config/surreal.rs`

**Interfaces:**
- Consumes: `MemoryError::ConfigInvalid(String)` and `config::helpers::env_lock()`.
- Produces: `NerConfig { extractor: NerExtractorConfig }`, `NerExtractorKind`, `ModelBackedNerConfig`, `NativeGlinerConfig`, and `GlinerDeviceKind`.

- [ ] **Step 1: Replace old tests with failing catalog and migration tests**

Add tests asserting exact selector variants, old alias rejection, removed-variable migration messages, irrelevant-setting rejection, normalized labels, finite `0.0..=1.0` thresholds, nonzero limits, and default Anno. Use this exact environment list in `with_ner_env` and `SURREAL_CONFIG_ENV_KEYS`:

```rust
const NER_ENV_KEYS: &[&str] = &[
    "NER_EXTRACTOR", "NER_CACHE_DIR", "NER_LABELS", "NER_THRESHOLD",
    "NER_MAX_CONCURRENCY", "NER_IDLE_UNLOAD_SECS", "GLINER_BATCH_SIZE",
    "GLINER_MAX_BATCH_TOKENS", "GLINER_DEVICE", "NER_PROVIDER", "NER_MODEL",
    "NER_MODEL_DIR", "NER_BATCH_SIZE", "NER_MAX_BATCH_TOKENS", "NER_DEVICE",
    "GLINER_IDLE_UNLOAD_SECS",
];
```

Representative assertions:

```rust
assert!(matches!(NerConfig::from_env()?.extractor, NerExtractorConfig::Anno));
assert!(matches!(parse("anno-onnx")?.extractor, NerExtractorConfig::AnnoOnnx(_)));
assert!(matches!(parse("urchade/gliner_multi-v2.1")?.extractor, NerExtractorConfig::ClassicGliner(_)));
assert!(matches!(parse("VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER")?.extractor, NerExtractorConfig::SauerkrautLfm25(_)));
```

- [ ] **Step 2: Run the focused tests and verify failure**

Run: `cargo test -p memory_mcp config::ner::tests -- --test-threads=1`

Expected: compilation fails because the typed variants do not exist.

- [ ] **Step 3: Implement the typed configuration and deterministic parser**

Use these exact core types:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NerExtractorKind { Anno, Regex, AnnoOnnx, ClassicGliner, SauerkrautLfm25 }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GlinerDeviceKind { Cpu, Metal, Auto }

#[derive(Debug, Clone)]
pub struct ModelBackedNerConfig {
    pub cache_dir: Option<PathBuf>,
    pub labels: Vec<String>,
    pub threshold: Option<f64>,
    pub max_concurrency: usize,
    pub idle_unload_secs: u64,
}

#[derive(Debug, Clone)]
pub struct NativeGlinerConfig {
    pub model: ModelBackedNerConfig,
    pub batch_size: usize,
    pub max_batch_tokens: usize,
    pub device: GlinerDeviceKind,
}

#[derive(Debug, Clone)]
pub enum NerExtractorConfig {
    Anno,
    Regex,
    AnnoOnnx(ModelBackedNerConfig),
    ClassicGliner(NativeGlinerConfig),
    SauerkrautLfm25(NativeGlinerConfig),
}

#[derive(Debug, Clone)]
pub struct NerConfig { pub extractor: NerExtractorConfig }
```

Implement `NerExtractorConfig::kind()`, `NerConfig::from_env()`, stable label normalization, exact case-sensitive repository selectors, and deterministic removed-variable checks before parsing any replacement value. Do not preserve old aliases.

- [ ] **Step 4: Run config and top-level zero-config tests**

Run:

```bash
cargo test -p memory_mcp config::ner::tests -- --test-threads=1
cargo test -p memory_mcp config::surreal::tests -- --test-threads=1
```

Expected: PASS; unset NER selects `Anno` and makes no model-backed fields representable.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/config/ner.rs crates/memory-mcp/src/config/constants.rs crates/memory-mcp/src/config.rs crates/memory-mcp/src/config/surreal.rs
git commit -m "refactor: add typed NER extractor configuration"
```

---

### Task 2: Adapt the backend registry and service construction

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/anno.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/regex.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`
- Modify: `crates/memory-mcp/src/service/core/builder.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`

**Interfaces:**
- Consumes: Task 1 typed configuration.
- Produces: `NerBuildContext`, five-entry registry keyed by `NerExtractorKind`, and single-construction service injection.

- [ ] **Step 1: Write failing registry exhaustiveness tests**

Assert one unique entry per `NerExtractorKind`, exact stable names `anno`, `regex`, `anno-onnx`, `gliner`, `sauerkraut-lfm2.5-gliner`, and lightweight dispatch without artifact access.

- [ ] **Step 2: Run registry tests and verify failure**

Run: `cargo test -p memory_mcp registry_`

Expected: FAIL because registry still uses `NerProviderKind` and has three entries.

- [ ] **Step 3: Implement the typed registry ABI**

```rust
pub(crate) struct NerBuildContext {
    pub(crate) data_dir: PathBuf,
    pub(crate) logger: StdoutLogger,
}

type BackendBuildFn = fn(NerExtractorConfig, NerBuildContext) -> BackendBoxFuture;

struct BackendSpec {
    kind: NerExtractorKind,
    name: &'static str,
    build: BackendBuildFn,
}
```

Keep one lookup in `create_entity_extractor`. Temporary `anno_onnx::build` and `lfm2_gliner::build` stubs must return `MemoryError::ConfigInvalid("extractor backend is not implemented in this build step".into())`; tests may inspect routing without claiming successful model construction.

Change private `MemoryService::build` to accept `Arc<dyn EntityExtractor>`. Programmatic `MemoryService::new` explicitly constructs lightweight Anno; environment startup passes the registry result and never constructs then replaces a default extractor.

- [ ] **Step 4: Run service and benchmark compilation tests**

Run:

```bash
cargo test -p memory_mcp registry_
cargo test -p memory_mcp create_entity_extractor_defaults_to_anno
cargo check -p eval-harness --benches
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/anno.rs crates/memory-mcp/src/service/entity_extraction/regex.rs crates/memory-mcp/src/service/entity_extraction/gliner.rs crates/memory-mcp/src/service/core/builder.rs crates/eval-harness/benches/ner_cpu.rs crates/eval-harness/benches/ner_metal.rs
git commit -m "refactor: dispatch typed NER extractors"
```

---

### Task 3: Promote the shared loaded-model runtime

**Files:**
- Create: `crates/memory-mcp/src/service/model_runtime.rs`
- Modify: `crates/memory-mcp/src/service.rs`
- Delete after migration: `crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs`
- Delete after migration: `crates/memory-mcp/src/service/entity_extraction/gliner/gate.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`

**Interfaces:**
- Produces: `LoadedModel<T>::new`, `get_or_load`, `install_loaded`, `arm_unload`; `InferenceGate::new`, `acquire`.

- [ ] **Step 1: Copy existing lifecycle tests and add a failing activation-handoff test**

```rust
#[tokio::test]
async fn installed_model_is_reused_without_calling_loader() {
    let model = LoadedModel::new(None);
    model.install_loaded(Arc::new("validated".to_string())).await;
    let loaded = model.get_or_load(|| panic!("loader must not run")).await.unwrap();
    assert_eq!(loaded.as_str(), "validated");
}
```

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp model_runtime::tests`

Expected: compilation failure because `model_runtime` does not exist.

- [ ] **Step 3: Move the proven state machines without adding artifact concerns**

Expose crate-private generic types. `install_loaded` must abort a pending unload task, update `last_used`, and replace the cached `Arc<T>`. Preserve post-use timer semantics and in-flight `Arc` safety exactly.

- [ ] **Step 4: Run lifecycle and classic GLiNER tests**

Run:

```bash
cargo test -p memory_mcp model_runtime::tests
cargo test -p memory_mcp gliner::batching::tests
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service.rs crates/memory-mcp/src/service/model_runtime.rs crates/memory-mcp/src/service/entity_extraction/gliner.rs crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs crates/memory-mcp/src/service/entity_extraction/gliner/gate.rs
git commit -m "refactor: share loaded model lifecycle"
```

---

### Task 4: Build the shared artifact manifest, state, and progress domain

**Files:**
- Create: `crates/memory-mcp/src/service/model_artifacts.rs`
- Create: `crates/memory-mcp/src/service/model_artifacts/manifest.rs`
- Create: `crates/memory-mcp/src/service/model_artifacts/state.rs`
- Create: `crates/memory-mcp/src/service/model_artifacts/progress.rs`
- Modify: `crates/memory-mcp/src/service.rs`

**Interfaces:**
- Produces the following exact public-within-crate contract:

```rust
pub(crate) struct ArtifactRequirement { pub path: &'static str, pub sha256: Option<&'static str> }
pub(crate) struct NerArtifactSpec { pub extractor_id: &'static str, pub repository: &'static str, pub files: &'static [ArtifactRequirement], pub runtime_version: &'static str }
pub(crate) enum RevisionStatus { Latest, UnverifiedLatest, LatestIncompatible }
pub(crate) enum ValidationStatus { ReleaseParityVerified, RuntimeRegressionVerified }
pub(crate) struct PreparedCheckpoint { pub root: PathBuf, pub repository: String, pub revision: String, pub artifact_identity: String, pub revision_status: RevisionStatus, pub validation_status: ValidationStatus }
pub(crate) enum ModelProgressPhase { Resolve, WaitForLease, Download, Verify, Construct, SmokeTest, Activate, Fallback }
pub(crate) struct ModelProgressEvent { pub schema_version: u8, pub extractor: String, pub phase: ModelProgressPhase, pub status: String, pub revision: Option<String>, pub downloaded_bytes: Option<u64>, pub total_bytes: Option<u64>, pub progress_percent: Option<u8>, pub message: Option<String> }
pub(crate) trait ModelProgressSink: Send + Sync { fn emit(&self, event: &ModelProgressEvent); }
```

- [ ] **Step 1: Write serialization, integrity, identity, and throttle tests**

Assert schema version `1`, one-line compact JSON, phase/completion emission, 5% boundaries, five-second heartbeat, no duplicate intermediate updates, stable SHA-256 artifact identity over sorted `path:size:sha256` entries, and rejection of missing/zero-byte files.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp model_artifacts::`

Expected: compilation failure.

- [ ] **Step 3: Implement pure manifest/state/progress logic**

`JsonLineProgressSink` writes exactly one JSON object plus newline to stderr. `CliProgressSink` renders human text to stderr. Neither type receives stdout. Persist state as JSON using write-to-sibling-temp, `sync_all`, then rename.

- [ ] **Step 4: Run focused tests**

Run: `cargo test -p memory_mcp model_artifacts::`

Expected: PASS without network access.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service.rs crates/memory-mcp/src/service/model_artifacts.rs crates/memory-mcp/src/service/model_artifacts
git commit -m "feat: define NER artifact lifecycle domain"
```

---

### Task 5: Implement acquisition, leases, activation, and recovery

**Files:**
- Create: `crates/memory-mcp/src/service/model_artifacts/download.rs`
- Create: `crates/memory-mcp/src/service/model_artifacts/lease.rs`
- Modify: `crates/memory-mcp/src/service/model_artifacts.rs`
- Replace NER usage in: `crates/memory-mcp/src/service/model_loader.rs`
- Preserve embedding behavior in: `crates/memory-mcp/src/service/model_loader.rs`
- Create: `crates/memory-mcp/tests/ner_model_lifecycle.rs`

**Interfaces:**
- Produces:

```rust
#[async_trait]
pub(crate) trait RevisionResolver { async fn latest(&self, repository: &str) -> Result<String, MemoryError>; }
#[async_trait]
pub(crate) trait ArtifactFetcher { async fn fetch(&self, repository: &str, revision: &str, requirement: &ArtifactRequirement, target: &Path, progress: &dyn ModelProgressSink) -> Result<(), MemoryError>; }
pub(crate) struct NerArtifactStore { /* root, resolver, fetcher, progress, clock */ }
pub(crate) async fn prepare(&self, spec: &NerArtifactSpec) -> Result<PreparedCheckpoint, MemoryError>;
```

- [ ] **Step 1: Write failing fake-resolver/fetcher lifecycle tests**

Cover: two lookup attempts within a total deadline; offline known-good fallback; failure without cache; process-unique staging; atomic activation; active+previous retention; failed candidate removal; commit-keyed incompatibility suppression; waiter behavior; heartbeat; conservative stale lease recovery; 60-second no-progress failure; active revision never deleted.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp --test ner_model_lifecycle`

Expected: compilation failure.

- [ ] **Step 3: Implement standard-library lease and injected acquisition**

Use `OpenOptions::new().write(true).create_new(true)` for lease acquisition. Lease JSON contains extractor, revision, PID, creation timestamp, heartbeat timestamp, and staging path. Never reclaim solely by age: require expired heartbeat and unsuccessful same-host process liveness check where available; otherwise wait and report. Use `tokio::time::timeout_at` for the shared ten-second resolve deadline and reset the download stall timer only when byte count increases.

- [ ] **Step 4: Run lifecycle tests**

Run: `cargo test -p memory_mcp --test ner_model_lifecycle -- --test-threads=1`

Expected: PASS without internet.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/model_artifacts.rs crates/memory-mcp/src/service/model_artifacts crates/memory-mcp/src/service/model_loader.rs crates/memory-mcp/tests/ner_model_lifecycle.rs
git commit -m "feat: add revision-safe NER artifact store"
```

---

### Task 6: Migrate classic GLiNER to prepared checkpoints

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/tests/local_model_integration.rs`

**Interfaces:**
- Consumes: `PreparedCheckpoint`, `NativeGlinerConfig`, `LoadedModel<T>`.
- Produces: classic artifact spec fixed to `urchade/gliner_multi-v2.1`, compatibility probe, and extractor fingerprint.

- [ ] **Step 1: Write failing tests for fixed identity and no backend download**

Assert required files `model.safetensors`, `gliner_config.json`, `tokenizer.json`; construction accepts a prepared root; no `NER_MODEL`; provider name remains `gliner`; idle unload comes from shared config; a probe-loaded `LoadedGliner` is installed and reused.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp gliner_`

Expected: FAIL against old flat config/downloader path.

- [ ] **Step 3: Implement prepared-checkpoint construction**

Change `GlinerLoader` to store `PreparedCheckpoint` and `NativeGlinerConfig`. Move artifact acquisition to registry startup. Keep DeBERTa inference and decoding unchanged. Run candidate construction/smoke inference only for a newly staged revision; install the returned `Arc<LoadedGliner>` into `LoadedModel` before activation.

- [ ] **Step 4: Run classic tests**

Run:

```bash
cargo test -p memory_mcp gliner_
cargo test -p memory_mcp --test local_model_integration --no-run
```

Expected: PASS/compile; no ordinary test downloads.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/gliner.rs crates/memory-mcp/tests/local_model_integration.rs
git commit -m "refactor: prepare classic GLiNER through artifact store"
```

---

### Task 7: Enable explicit Anno NuNER ONNX

**Files:**
- Modify: `Cargo.toml`
- Modify: `crates/memory-mcp/Cargo.toml`
- Modify: `Cargo.lock`
- Create: `crates/memory-mcp/src/service/entity_extraction/anno_onnx.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/anno.rs`
- Create: `crates/memory-mcp/tests/anno_onnx_integration.rs`

**Interfaces:**
- Produces: exact `numind/NuNER_Zero` artifact spec and CPU-only `AnnoOnnxEntityExtractor`.

- [ ] **Step 1: Write failing default-path and explicit-backend tests**

Assert `AnnoEntityExtractor::new()` uses the explicit dependency-light Anno constructor rather than dynamic cache-sensitive backend selection. Assert `anno-onnx` consumes only prepared local files, identifies as `anno-onnx`, maps labels deterministically, obeys `LoadedModel`, and never falls back to heuristics.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp anno_onnx --no-default-features`

Expected: compilation failure because ONNX support/module is absent.

- [ ] **Step 3: Enable the approved Anno feature and implement local-only construction**

Set workspace dependency to:

```toml
anno = { version = "0.11.0", default-features = false, features = ["onnx"] }
```

Use Anno’s explicit NuNER API for `numind/NuNER_Zero`; do not use `StackedNER::default()` for this backend and do not invoke Anno’s downloader. If Anno 0.11 lacks a public local-path constructor, add the smallest upstream-compatible adapter in this module and document the exact API limitation; do not switch to dynamic fallback.

- [ ] **Step 4: Run package and lockfile gates**

Run:

```bash
cargo update -p anno --precise 0.11.0
cargo test -p memory_mcp anno_
cargo metadata --locked --no-deps
```

Expected: PASS; zero-config Anno tests remain download-free.

- [ ] **Step 5: Commit**

```bash
git add Cargo.toml Cargo.lock crates/memory-mcp/Cargo.toml crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/anno.rs crates/memory-mcp/src/service/entity_extraction/anno_onnx.rs crates/memory-mcp/tests/anno_onnx_integration.rs
git commit -m "feat: add explicit Anno NuNER ONNX extractor"
```

---

### Task 8: Implement the native VAGO LFM2 model and tensor adapter

**Files:**
- Create: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs`
- Create: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/config.rs`
- Create: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/model.rs`
- Create: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/tensors.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`

**Interfaces:**
- Produces: `Lfm2BiConfig`, `Lfm2BiModel`, `LoadedLfm2Gliner`, direct PTH loader, and effective device.

- [ ] **Step 1: Add failing config, padding, attention, convolution, and tensor-name tests**

Fixtures must encode upstream `gliner_config.json` values, prove causal masking is absent, prove odd/even symmetric center padding, prove `fuse_layers=true`, and map representative upstream keys to exact Candle module paths. Test missing/unexpected/shape-incompatible tensors as activation failures.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp lfm2_gliner::`

Expected: compilation failure.

- [ ] **Step 3: Implement the exact architecture without a generic LFM2 framework**

`Lfm2BiModel::load(vb: VarBuilder, config: &Lfm2BiConfig) -> candle_core::Result<Self>` must implement bidirectional self-attention, symmetric center-padding convolutions, layer fusion, and upstream normalization/MLP ordering. Load F32 weights only:

```rust
let vb = VarBuilder::from_pth(
    checkpoint.root.join("pytorch_model.bin"),
    DType::F32,
    &device,
)?;
```

Keep prefix/name adaptation in `tensors.rs`. Do not alter classic GLiNER based on repository name.

- [ ] **Step 4: Run focused model tests**

Run: `cargo test -p memory_mcp lfm2_gliner:: -- --test-threads=1`

Expected: PASS with synthetic tiny tensors; no full checkpoint required.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs crates/memory-mcp/src/service/entity_extraction/lfm2_gliner
git commit -m "feat: implement native LFM2 GLiNER model"
```

---

### Task 9: Implement VAGO tokenization, span decoding, device fallback, and extractor lifecycle

**Files:**
- Create: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/decode.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Create: `crates/memory-mcp/tests/vago_lfm2_integration.rs`

**Interfaces:**
- Produces `ScoredEntity { start, end, text, label, score }`, VAGO artifact spec, runtime regression probe, and `EntityExtractor` implementation.

- [ ] **Step 1: Write failing exact-span and device-policy tests**

Cover UTF-8 character spans for Cyrillic, English, and mixed text; normalized label ordering; thresholding before candidate conversion; exact accepted set; absolute score tolerance `1e-4`; `auto` Metal failure event followed by CPU; explicit Metal failure without fallback; concurrency gate; idle unload; preloaded candidate reuse.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp --test vago_lfm2_integration --no-run`

Expected: compilation failure because decoding/extractor contract is incomplete.

- [ ] **Step 3: Implement VAGO extractor composition**

Use exact artifact requirements: `pytorch_model.bin`, `gliner_config.json`, `config.json`, `tokenizer.json`, and backend-required custom config metadata verified from the upstream revision. Decode to `Vec<ScoredEntity>` first, sort by `(start, end, label, text)`, then convert to deduplicated `EntityCandidate`. Fingerprint the effective device, never the requested device alone.

- [ ] **Step 4: Run synthetic and ignored full-model compilation gates**

Run:

```bash
cargo test -p memory_mcp lfm2_gliner::
cargo test -p memory_mcp --test vago_lfm2_integration --no-run
```

Expected: PASS/compile.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/lfm2_gliner.rs crates/memory-mcp/src/service/entity_extraction/lfm2_gliner/decode.rs crates/memory-mcp/tests/vago_lfm2_integration.rs
git commit -m "feat: add VAGO LFM2 entity extractor"
```

---

### Task 10: Add release parity and unseen-revision regression fixtures

**Files:**
- Create: `evals/corpora/ner/vago_release_parity.json`
- Create: `evals/corpora/ner/vago_runtime_regression.json`
- Modify: `crates/memory-mcp/tests/vago_lfm2_integration.rs`
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Modify: `crates/eval-harness/benches/ner_metal.rs`

**Interfaces:**
- Produces an embedded regression corpus and revision classification used by activation.

- [ ] **Step 1: Generate reference fixtures with pinned Python/PyTorch tooling**

Each case must contain `id`, `language` (`ru`, `en`, `mixed`), `text`, ordered labels, and ordered entities with `start`, `end`, `text`, `label`, and `score`. Record repository commit and Python package versions at the file root. Python is fixture-generation tooling only and is never invoked by the Rust runtime.

- [ ] **Step 2: Add failing fixture-schema and parity tests**

Require exact text/span/label/order/set equality and `abs(native_score-reference_score) <= 1e-4`. Reject an unseen fixture whose structural output drifts even if aggregate precision remains high.

- [ ] **Step 3: Run against locally prepared checkpoint**

Run:

```bash
cargo test -p memory_mcp --test vago_lfm2_integration release_parity -- --ignored --exact
cargo test -p memory_mcp --test vago_lfm2_integration runtime_regression -- --ignored --exact
```

Expected: PASS. If unavailable locally, execution must stop here rather than marking parity complete.

- [ ] **Step 4: Embed only the compact runtime corpus and wire validation status**

Release-known commits produce `ReleaseParityVerified`; unseen HEAD commits must pass the embedded runtime corpus and produce `RuntimeRegressionVerified`. Do not claim Python parity for unseen commits.

- [ ] **Step 5: Commit**

```bash
git add evals/corpora/ner/vago_release_parity.json evals/corpora/ner/vago_runtime_regression.json crates/memory-mcp/tests/vago_lfm2_integration.rs crates/eval-harness/benches/ner_cpu.rs crates/eval-harness/benches/ner_metal.rs
git commit -m "test: lock VAGO RU EN parity corpus"
```

---

### Task 11: Persist extractor fingerprints for new extraction projections

**Execution precondition:** Ask the user for confirmation before creating the migration file, as required by `AGENTS.md`. Do not proceed without confirmation.

**Files:**
- Create: `crates/memory-mcp/migrations/029_entity_extraction_projection.surql`
- Modify: `crates/memory-mcp/src/storage/migrations.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/episode/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/service_context.rs`
- Modify: `crates/memory-mcp/src/service/core.rs`
- Test: `crates/memory-mcp/tests/service_integration.rs`

**Interfaces:**
- Produces `ExtractorFingerprint` and append-only `entity_extraction_projection` records.

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtractorFingerprint {
    pub selector: String,
    pub backend: String,
    pub repository: Option<String>,
    pub revision: Option<String>,
    pub artifact_identity: Option<String>,
    pub labels: Vec<String>,
    pub threshold: Option<f64>,
    pub revision_status: Option<RevisionStatus>,
    pub validation_status: Option<ValidationStatus>,
    pub runtime_version: String,
    pub effective_device: Option<String>,
}
```

- [ ] **Step 1: Write failing persistence test**

Extract an episode using an injected extractor identity, then select `entity_extraction_projection:<episode-key>:<projection-id>` and assert episode ID, `t_ingested`, scope, fingerprint, and entity IDs. Assert a second extraction appends a new projection and does not mutate the first.

- [ ] **Step 2: Run and verify failure**

Run: `cargo test -p memory_mcp --test service_integration extractor_fingerprint_projection -- --exact`

Expected: FAIL because table and identity contract do not exist.

- [ ] **Step 3: Add migration and append-only write**

Migration 029 defines a schemafull table with indexed `episode_id`, `scope`, `t_ingested`, `fingerprint`, and `entity_ids`. Add `fn fingerprint(&self) -> ExtractorFingerprint` to `EntityExtractor`; lightweight implementations use no repository/revision/threshold. Persist only after entity extraction succeeds. Do not re-extract historical episodes.

- [ ] **Step 4: Run migration and integration tests**

Run:

```bash
cargo test -p memory_mcp storage::migrations::tests
cargo test -p memory_mcp --test service_integration extractor_fingerprint_projection -- --exact
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/migrations/029_entity_extraction_projection.surql crates/memory-mcp/src/storage/migrations.rs crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/episode/entity_extraction.rs crates/memory-mcp/src/service/service_context.rs crates/memory-mcp/src/service/core.rs crates/memory-mcp/tests/service_integration.rs
git commit -m "feat: persist NER extractor fingerprints"
```

---

### Task 12: Add zero-config, progress-channel, documentation, and release gates

**Files:**
- Modify: `crates/memory-mcp/tests/zero_config_embedded.rs`
- Create: `crates/memory-mcp/tests/ner_progress_channels.rs`
- Modify: `.github/workflows/ci.yml`
- Modify: `README.md`
- Modify: `docs/agent/REPOSITORY_LAYOUT.md`
- Modify: `docs/agent/MCP_TOOLS.md`
- Modify: `docs/superpowers/plans/2026-08-04-zero-config-defaults.md`
- Modify: `docs/adr/0036-unified-ner-extractor-selection-and-model-lifecycle.md`

**Interfaces:**
- Consumes all previous tasks.
- Produces release-ready ordinary artifact and canonical operator documentation.

- [ ] **Step 1: Write failing end-to-end acceptance tests**

Assert empty NER environment selects lightweight Anno, creates no `<data>/models/ner`, performs ingest/extract/recall, and emits no model progress. Spawn stdio MCP with a fixture model update and assert every stdout line is valid JSON-RPC while stderr contains schema-version `1` JSON progress. Assert CLI stderr uses human progress and `memory_mcp init` performs no model lookup.

- [ ] **Step 2: Run and verify failure**

Run:

```bash
cargo test -p memory_mcp --test zero_config_embedded -- --test-threads=1
cargo test -p memory_mcp --test ner_progress_channels -- --test-threads=1
```

Expected: FAIL until startup/progress wiring is complete.

- [ ] **Step 3: Wire startup progress and cross-platform CI smoke tests**

Choose `CliProgressSink` for interactive CLI and `JsonLineProgressSink` for MCP stdio before calling `create_entity_extractor`. Add Linux, macOS x86_64/aarch64, and Windows ordinary-release smoke steps for `anno`, `regex`, and local prepared `anno-onnx`; compile and fixture-gate both native GLiNER backends. Keep one artifact per target.

- [ ] **Step 4: Replace public documentation and close ADR status**

Document the exact selector table, removed-variable migration table, canonical settings, cache location, startup network behavior, last-known-good fallback, supported RU/EN scope, and progress channels. Mark ADR-0036 implemented only after all gates pass. In the zero-config plan, replace Stage 8’s “implementation pending” wording with a link to this completed plan; do not duplicate its steps.

- [ ] **Step 5: Run focused and full validation**

Run:

```bash
cargo fmt --all
cargo test -p memory_mcp
cargo test --workspace --lib --bins --tests --locked
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
cargo build -p memory_mcp --release --locked
cargo run -p memory_mcp --release -- init --target vscode
```

On macOS additionally run:

```bash
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test -p memory_mcp --features metal lfm2_gliner::
```

Expected: all commands pass with zero warnings and no formatting diff.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/ci.yml README.md docs/agent/REPOSITORY_LAYOUT.md docs/agent/MCP_TOOLS.md docs/adr/0036-unified-ner-extractor-selection-and-model-lifecycle.md docs/superpowers/plans/2026-08-04-zero-config-defaults.md crates/memory-mcp/tests/zero_config_embedded.rs crates/memory-mcp/tests/ner_progress_channels.rs
git commit -m "docs: ship unified local NER extractors"
```

---

## Final Acceptance Checklist

- [ ] Unset `NER_EXTRACTOR` uses lightweight Anno without network or model cache creation.
- [ ] All five exact selector values dispatch through one registry; unknown aliases and arbitrary repositories fail.
- [ ] Every removed variable fails with actionable replacement guidance, even if empty or accompanied by canonical settings.
- [ ] Irrelevant settings fail at configuration parsing.
- [ ] Anno ONNX is explicit, CPU-only, local-path-only, and never falls back.
- [ ] Classic GLiNER and VAGO share operational lifecycle but not architecture code.
- [ ] VAGO consumes direct PTH F32 weights and matches RU/EN/mixed structural fixtures with score tolerance `1e-4`.
- [ ] Latest lookup, progress-stall handling, lease coordination, atomic activation, incompatible revision suppression, offline fallback, and retention policies are tested without network.
- [ ] Probe-loaded candidates are reused for first extraction.
- [ ] `NER_IDLE_UNLOAD_SECS` applies to all model-backed extractors and defaults to retention.
- [ ] CLI and MCP progress use stderr; MCP stdout remains JSON-RPC only; `init` remains network-free.
- [ ] New extraction projections persist exact extractor fingerprints; historical episodes are untouched.
- [ ] One ordinary artifact passes Linux, macOS, and Windows gates.
- [ ] Full tests, strict clippy, formatting, release build, and smoke commands pass.

## Self-Review

### Spec coverage

Tasks 1–2 cover the closed selector, removed settings, typed invariants, registry, and zero-config construction. Tasks 3–6 cover shared loaded-model and artifact lifecycles plus classic migration. Task 7 covers explicit NuNER ONNX and approved dependencies. Tasks 8–10 cover the distinct direct-PTH VAGO architecture, F32 device policy, RU/EN parity, and unseen-revision validation. Task 11 covers future-only fingerprint persistence. Task 12 covers progress channels, zero-config proof, documentation, distribution, and all shipping gates.

### Placeholder scan

The plan contains no `TBD`, `TODO`, “similar to,” unspecified error handling, or unnamed test steps. The only execution gate is the project-mandated user confirmation before creating migration 029; its file, schema responsibility, tests, and commands are otherwise explicit.

### Type consistency

`NerConfig` always wraps `NerExtractorConfig`; registry dispatch uses `NerExtractorKind`; model-backed variants share `ModelBackedNerConfig`; native GLiNER variants wrap it in `NativeGlinerConfig`. Artifact preparation always returns `PreparedCheckpoint`; in-memory retention always uses `LoadedModel<T>`; persisted identity always uses `ExtractorFingerprint`. VAGO internal parity uses `ScoredEntity` before conversion to the unchanged public `EntityCandidate`.
