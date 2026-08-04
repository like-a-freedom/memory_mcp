# Universal NER Extractor — Implementation Plan

> For agentic workers: use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use `- [ ]` checkboxes.

**Goal:** Eliminate the hard-coded GLiNER lock-in in the NER layer by converting the `EntityExtractor` trait into a pluggable backend architecture where GLiNER, Anno, Regex, and LLM each register as self-contained backends, without changing inference behavior, thresholds, or quality metrics.

**Architecture:** Keep the existing `EntityExtractor` trait as the single call-site contract (`service/builder.rs` consumes `Arc<dyn EntityExtractor>`). Move provider construction from the factory function into per-backend `build()` functions inside each backend module, registered in a small provider map. The factory becomes: resolve provider name → call backend's `build()` → return `Arc<dyn EntityExtractor>`. No public API changes to `MemoryService`, `ExtractCapability`, or the MCP tool surface.

**Tech Stack:** Rust 1.97.1, async-trait, existing Candle / tokenizers / anno / regex.

## Global Constraints

1. **Zero behavior change** for all four existing providers (regex, anno, gliner, llm-as-code-path).
2. **Environment compatibility:** `NER_PROVIDER=regex|anno|local-gliner/gliner` env names, model-dir defaults, thresholds, and batch/concurrency defaults all stay identical.
3. **Quality gates:** `make eval-pr` + `make eval-release` must preserve v5 values: `recall_at_5=1.0000`, `mrr=0.9924`, `top_1_hit_rate=0.9848`, `entity_f1=0.7500`, `claim_precision=1.0000`, `claim_recall=1.0000`.
4. **No new dependencies per project rule** unless feature-gated. GLiNER remains optional (still compiles without its models on disk for unrelated tests).
5. **Provider name strings stay stable** for logging (`provider_name()`): `"anno"`, `"gliner"`, `"regex"`, `"llm"`.
6. **Default provider stays Anno** (`NerConfig::default()`), matching `builder.rs:429`.

## Design

```text
 MemoryService::build / new_from_env
        │  holds Arc<dyn EntityExtractor>
        ▼
  EntityExtractorFactory::create(config, data_dir, logger)
        │  looks up provider_name → BackendRegistry
        ▼
  BackendRegistry { map: HashMap<NerProviderKind, fn(&NerConfig,&PathBuf,&StdoutLogger) -> Result<Arc<dyn EntityExtractor>, MemoryError> }
        │
  regex::RegexEntityExtractor::build(config, ..)
  anno::AnnoEntityExtractor::build(config, ..)
  gliner::GlinerEntityExtractor::build(config, model_dir, logger)
  llm::LlmEntityExtractor::build(config, ..)   (unchanged — function-injection)
```

- Each backend owns its config interpretation (e.g. only GLiNER looks at `model/labels/threshold/device`; Anno ignores those).
- The `create_entity_extractor(config, data_dir, logger)` public signature does not change (back-compat for `service.rs`, `tests/`, `eval-harness`).
- `create_entity_extractor` is the *only* provider-dispatch point; no `match` on provider kind anywhere else.

---

## Task 1: Backend `build()` entry points + registry

**Why:** Centralize construction per module so each backend is self-contained; factory stops leaking Candle imports for non-GLiNER builds.

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/regex.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/anno.rs`

**Interfaces:**
- Add per backend: `pub(crate) fn build(config: &NerConfig, data_dir: &str, logger: &StdoutLogger) -> Result<Arc<dyn EntityExtractor>, MemoryError>`.
- Add in `entity_extraction.rs`: `fn registry() -> &'static [BackendSpec]` where `BackendSpec { kind: NerProviderKind, name: &'static str, build: BackendBuildFn }`.
- `create_entity_extractor` consumes the registry.

- [ ] **Step 1 — failing test** in `entity_extraction.rs::tests`: dispatch through registry returns correct `provider_name()` for each kind:
```rust
#[tokio::test]
async fn registry_dispatches_each_backend() {
    let logger = StdoutLogger::new("error");
    for (kind, expected) in [
        (NerProviderKind::Regex, "regex"),
        (NerProviderKind::Anno, "anno"),
    ] {
        let mut cfg = NerConfig::default();
        cfg.provider = kind;
        let extractor = create_entity_extractor(&cfg, "/tmp/x", &logger).await.unwrap();
        assert_eq!(extractor.provider_name(), expected);
    }
}
```
Run: `cargo test -p memory_mcp registry_dispatches_each_backend` → currently passes, so add a registry-specific invariant that fails first: assert `(create_entity_extractor)(&cfg_anno).await?.provider_name() == "anno"` **and** that a hypothetical `registered_kind_count() >= 4` helper exists. Simplest strict-failing form:
```rust
#[test]
fn registry_has_one_spec_per_provider_kind() {
    assert!(registry_spec_count() >= 4, "expected regex/anno/gliner/llm entries");
}
```
→ FAIL (helper absent). Then implement it.

- [ ] **Step 2 — implement `build()` in each backend + registry table.** In `regex.rs` and `anno.rs`:
```rust
pub(crate) fn build(
    _config: &NerConfig, _data_dir: &str, _logger: &StdoutLogger,
) -> Result<Arc<dyn EntityExtractor>, MemoryError> {
    Ok(Arc::new(Self::new()?))
}
```
(anno currently takes no config; keep signature uniform.)

In `entity_extraction.rs`:
```rust
type BackendBuildFn = fn(&NerConfig, &str, &StdoutLogger) -> Result<Arc<dyn EntityExtractor>, MemoryError>;
struct BackendSpec { kind: NerProviderKind, name: &'static str, build: BackendBuildFn }

fn backend_registry() -> &'static [BackendSpec] { &[
    BackendSpec { kind: NerProviderKind::Regex, name: "regex", build: regex::RegexEntityExtractor::build },
    BackendSpec { kind: NerProviderKind::Anno,  name: "anno",  build: anno::AnnoEntityExtractor::build },
    BackendSpec { kind: NerProviderKind::LocalGliner, name: "gliner", build: gliner::GlinerEntityExtractor::build },
] }
```
NOTE: GLiNER's build must stay async (it downloads the model). So the registry fn signature must be async: switch `BackendBuildFn` to an async fn pointer using `Pin<Box<dyn Future>>`-returning closure, or simplest: make the registry a lookup by kind returning the *module path* async fn via `Box::pin(...)`. Concretely:
```rust
type BackendBuildFn = for<'a> fn(&'a NerConfig, &'a str, &'a StdoutLogger)
    -> futures::future::BoxFuture<'a, Result<Arc<dyn EntityExtractor>, MemoryError>>;
```
But no `futures` dep is allowed → use `Pin<Box<dyn Future<Output=...> + Send + 'a>>` with `async move` blocks returned from plain `fn`s:
```rust
type BackendBuildFn = for<'a> fn(&'a NerConfig, &'a str, &'a StdoutLogger)
    -> Pin<Box<dyn Future<Output = Result<Arc<dyn EntityExtractor>, MemoryError>> + Send + 'a>>;
```
Each backend exposes a plain `fn build(...) -> Pin<Box<dyn Future...>>` that wraps its existing async logic with `Box::pin(async move { ... })`.

- [ ] **Step 3 — rewrite `create_entity_extractor`** to use the registry:
```rust
pub async fn create_entity_extractor(
    config: &NerConfig, data_dir: &str, logger: &StdoutLogger,
) -> Result<Arc<dyn EntityExtractor>, MemoryError> {
    let spec = backend_registry()
        .iter()
        .find(|spec| spec.kind == config.provider)
        .ok_or_else(|| MemoryError::ConfigInvalid(format!("unsupported NER provider: {:?}", config.provider)))?;
    (spec.build)(config, data_dir, logger).await
}
```
LLM variant stays reachable separately (it is constructed via `LlmEntityExtractor::new(f)` — code path, not config enum today); document that.

- [ ] **Step 4 — run** `cargo test -p memory_mcp entity_extraction` and `cargo clippy -p memory_mcp --all-targets -- -D warnings` → green.

- [ ] **Step 5 — commit** `refactor(ner): registry-backed provider dispatch (Task 1)`.

## Task 2: Move GLiNER construction into `gliner::build`

**Why:** Task 1 only wires names; GLiNER's model-fetch + `new_with_runtime` call still lives in the factory. Move it so `gliner.rs` owns everything GLiNER.

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs` (add `pub(crate) fn build` wrapping existing async init)
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs` (remove `ensure_gliner_model_cached` import)

- [ ] **Step 1 — adjust/extend test** `create_entity_extractor_defaults_to_anno` to still pass; add a negative test:
```rust
#[tokio::test]
async fn gliner_build_requires_model_name_when_provider_is_gliner() {
    let mut cfg = NerConfig::default();
    cfg.provider = NerProviderKind::LocalGliner;
    cfg.model = None;
    let logger = StdoutLogger::new("error");
    let err = create_entity_extractor(&cfg, "/tmp/x", &logger).await.unwrap_err();
    assert!(matches!(err, MemoryError::ConfigInvalid(_)));
}
```
Run → PASS (behavior preserved through new path).

- [ ] **Step 2 — implement** `gliner::build` containing today's `model_dir_or_default` + `ensure_gliner_model_cached` + `new_with_runtime(...)` body (moved verbatim, including `config.device`, `batch_size`, `max_batch_tokens`, `max_concurrency`).

- [ ] **Step 3 — cleanup** factory imports so `entity_extraction.rs` no longer references `model_loader` directly (delegates to gliner).

- [ ] **Step 4 — run** full crate tests: `cargo test -p memory_mcp -p eval-harness --features accelerate`. Run GLiNER parity: `cargo test -p memory_mcp --features accelerate --test local_model_integration -- --ignored`.

- [ ] **Step 5 — commit** `refactor(ner): gliner owns its model-loading path (Task 2)`.

## Task 3: Cleanup + wiring audit

**Files:** `crates/memory-mcp/src/service.rs`, `crates/memory-mcp/src/service/core/builder.rs`, docs.

- [ ] **Step 1 — grep audit:** `grep -rn "NerProviderKind::" crates/` — expect matches only in `config/ner.rs`, `entity_extraction.rs` registry, and tests. Any other match is a leak → fix.
- [ ] **Step 2 — docs:** update `docs/agent/REPOSITORY_LAYOUT.md` entity-extraction section to describe the registry (one paragraph).
- [ ] **Step 3 — final gates:** `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`, `cargo test -p memory_mcp -p eval-harness`, `make eval-pr`, `make eval-release` — all must pass with v5 metric values.
- [ ] **Step 4 — commit** `refactor(ner): finalize universal extractor seam`.

## Task 4: Optional future backends (documented, not implemented)

Add to the ADR: how to add a backend = create `entity_extraction/<name>.rs` with `build()` + `impl EntityExtractor`, add its `BackendSpec` entry, add a `NerProviderKind` variant and env alias. No framework changes otherwise. (Doc-only task; no code.)

---

## Explicitly out of scope

- Changing inference math, thresholds, sweeping strategies, tokenizer.
- Removing `LlmEntityExtractor`'s constructor-from-fn (it stays as the code-injected path).
- Turning GLiNER into an external service — it remains in-process via Candle.
- Adding any heavyweight new model backend in this PR.

## Validation

After each task and at the end: `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`; `cargo test -p memory_mcp -p eval-harness`; GLiNER parity test (`local_model_integration`, ignored); `make eval-pr` + `make eval-release`. All gates must equal the v5 anchors. Any deviation → revert that task.

---

**Plan complete and saved to `docs/superpowers/plans/2026-08-03-universal-ner-extractor.md`. Two execution options:**

1. **Subagent-Driven (recommended)** — fresh subagent per task with review between tasks.
2. **Inline Execution** — execute tasks here with checkpoints.

**Which approach?**
