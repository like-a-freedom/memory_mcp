# NER Performance Optimization Plan (v3 — verified)

> For agentic workers: use superpowers:subagent-driven-development or superpowers:executing-plans. Steps use `- [ ]` checkboxes.

**Goal:** Reduce GLiNER NER extraction latency and pipeline latency on the reference host (Apple Silicon) with **zero quality degradation** — bitwise-stable ordered candidates and unchanged eval-gate metrics.

**Architecture:** Fix the measurement axis first; enable the Apple Accelerate BLAS backend for Candle (ADR-0028); then remove redundant I/O in the extraction pipeline. GLiNER hot-path micro-optimizations are deferred unless a real GLiNER profiler proves they matter — the dominant cost is the DeBERTa forward pass.

**Tech Stack:** Rust 1.97.1, Tokio 1.53, Candle 0.11.0 (git pin `21cca0b`, `default-features=false`), tokenizers 0.23.1, SurrealDB (kv-mem), Criterion 0.5, macOS arm64 (Accelerate, Metal).

## Global Constraints (verbatim)

1. **Quality invariant:** identical ordered `(canonical_name, entity_type)` candidate lists for default and per-call zero-shot labels; hidden-state diagnostic within `atol=5e-5, rtol=1e-4` (existing test in `gliner/batching.rs`).
2. **No parameter changes:** never change `NER_THRESHOLD`, `max_span_width`, `max_seq_len`, label sets, or the tokenizer.
3. **v5 metrics anchor (must not degrade):** `recall_at_5=1.0000`, `mrr=0.9924`, `top_1_hit_rate=0.9848`, `entity_f1=0.7500`, `claim_precision=1.0000`, `claim_recall=1.0000`, `action_grounding_pass_rate=1.0000`, `poisoning_pass_rate=1.0000`, `end-to-end/context_match_rate=1.0000`.
4. **No new heavy deps:** no gigatoken (no WordPiece, no Rust crate); no inference framework swaps.

## Corrected facts this plan now encodes

- The `ner_cpu` bench exercises **Anno**, not GLiNER (`MemoryService::new` defaults to Anno at `crates/memory-mcp/src/service/core/builder.rs:429`). A true GLiNER bench must build the extractor via `create_entity_extractor` with a local model dir, not via `make_service`.
- Candle's default CPU matmul at pin `21cca0b` is the pure-Rust `gemm` crate with rayon parallelism (`candle-core/src/cpu_backend/mod.rs:1372-1453`); `accelerate` swaps in Apple `cblas_sgemm/dgemm` (same file, `1455+`). Expected delta is tens of percent on typical GLiNER GEMM shapes, **not** 10×. The earlier statement "isolation reduces variance > 50%" was invented; dropped.
- `iter_batched` with `BatchSize::LargeInput` still measures the per-iteration setup if the setup closure is timed. For async, use `criterion::BatchSize::SmallInput` and hoist non-timed setup outside, or use `iter_custom` for async wall-time.
- Paths in this repo are `crates/memory-mcp/...` and `crates/eval-harness/...` (v2 of this plan used a wrong `memory_mcp/src/` prefix).
- Adding a `[[bench]]` needs no `benches/mod.rs` (v2 incorrectly suggested it).
- Hard "assert exactly 0.7500 F1 inside a bench" contradicts gate design: Criterion tracks deltas; eval gates enforce floors. Watchdog belongs in `eval-harness` gate evaluation, not benches.

---

## Task 1: Real GLiNER benchmark + isolation fix

**Why:** Without a GLiNER-backed bench, every optimization claim is unverifiable; current `ner_cpu` measures Anno + service startup.

**Files:**
- Modify: `crates/eval-harness/benches/ner_cpu.rs`
- Create: `crates/eval-harness/benches/ner_gliner.rs`
- Register bench target: `crates/eval-harness/Cargo.toml`

**Interfaces:**
- Consumes: `memory_mcp::service::entity_extraction::create_entity_extractor(config, data_dir, &logger)`, `NerConfig`, `NerProviderKind::LocalGliner`, `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1`.
- Produces: bench results for `gliner_single_window_warm`, `gliner_multi_window_warm`; parity harness callable by Task 3.

- [ ] **Step 1 — failing determinism test first.** Add a test to `crates/eval-harness/tests/test_bench_correctness.rs`:
```rust
#[test]
fn gliner_fixture_texts_are_deterministic() {
    let f = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    assert_eq!(f.multi_window.split_whitespace().count(), f.multi_window_token_count());
    assert!(f.single_window.contains("Alice Smith"));
}
```
Run: `cargo test -p eval-harness gliner_fixture_texts_are_deterministic` → expect PASS (documents fixture contract).

- [ ] **Step 2 — write `ner_gliner.rs`** that hoists model load out of the timed loop and measures only `extract_candidates` via `iter_custom` (async wall-time):
```rust
use criterion::{Criterion, criterion_group, criterion_main, black_box};
use std::time::Instant;
use memory_mcp::service::{EntityExtractor, create_entity_extractor};
use memory_mcp::config::{NerConfig, NerProviderKind, NerDeviceKind};
use memory_mcp::logging::StdoutLogger;
use std::sync::Arc;

fn gliner() -> Arc<dyn EntityExtractor> {
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(async {
        let config = NerConfig {
            provider: NerProviderKind::LocalGliner,
            model: Some("urchade/gliner_multi-v2.1".into()),
            model_dir: Some(format!("{}/../memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1", env!("CARGO_MANIFEST_DIR"))),
            labels: NerConfig::default().labels,
            threshold: 0.5,
            batch_size: 1,
            max_batch_tokens: 1536,
            max_concurrency: 1,
            device: NerDeviceKind::Cpu,
        };
        create_entity_extractor(&config, env!("CARGO_MANIFEST_DIR"), &StdoutLogger::new("error")).await.unwrap()
    })
}

fn bench_gliner_single_window(c: &mut Criterion) {
    let extractor = gliner();
    let text = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap().single_window;
    c.bench_function("gliner_single_window_warm", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(extractor.extract_candidates(black_box(&text)).await.unwrap());
                });
            }
            start.elapsed()
        })
    });
}
// multi_window variant identical, using fixture.multi_window
criterion_group!(benches, bench_gliner_single_window, bench_gliner_multi_window);
criterion_main!(benches);
```

- [ ] **Step 3 — register bench** in `crates/eval-harness/Cargo.toml`:
```toml
[[bench]]
name = "ner_gliner"
harness = false
```

- [ ] **Step 4 — fix provider label in `ner_cpu.rs`:** rename/group bench output to make explicit it measures the default-service path (Anno + DB), e.g. rename functions to `default_service_single_window`. Do not delete it — it is still the pipeline-overhead probe.

- [ ] **Step 5 — run:** `cargo bench -p eval-harness --bench ner_gliner -- --noplot --sample-size 10` → produces honest GLiNER numbers. Record in the benchmark report.

- [ ] **Step 6 — commit:** `git add crates/eval-harness && git commit -m "bench: add real GLiNER NER bench with warm-model isolation"`

## Task 2: Enable Candle Accelerate (ADR-0028)

**Why:** `gemm` → Accelerate `cblas_*` is the safest available compute upgrade on macOS; zero logic change.

**Files:**
- Modify: `Cargo.toml` (workspace `[workspace.dependencies]` candle-core/candle-nn/candle-transformers lines) OR add a memory-mcp feature. **Decision: feature-gate, per AGENTS.md "large dependencies must be feature-gated".**

**Interfaces:**
- Consumes: existing `metal` feature precedent at `crates/memory-mcp/Cargo.toml:41-49`.
- Produces: `memory-mcp` feature `accelerate = ["candle-core/accelerate"]`; dotted into any downstream crate that wants it.

- [ ] **Step 1 — failing check first:** none applicable for Cargo feature; instead write the parity gate before flipping the switch: run existing ignored parity test against current build:
`cargo test -p memory_mcp --test local_model_integration local_gliner_batching_preserves_exact_default_and_zero_shot_candidates -- --ignored --exact` → must PASS (baseline).

- [ ] **Step 2 — add feature** to `crates/memory-mcp/Cargo.toml`:
```toml
[features]
accelerate = ["candle-core/accelerate"]
```

- [ ] **Step 3 — measure with the Task-1 bench:**
`cargo bench -p eval-harness --bench ner_gliner --features memory_mcp/accelerate -- --noplot --sample-size 10` (confirm the feature path propagates; if criterion can't forward features, bench via `cargo bench -p eval-harness --features memory_mcp/accelerate`).

- [ ] **Step 4 — parity:** rerun the Step-1 parity test **with the feature enabled**:
`cargo test -p memory_mcp --features accelerate --test local_model_integration local_gliner_batching_preserves_exact_default_and_zero_shot_candidates -- --ignored --exact` → candidates must be bitwise identical to Step-1 signature saved in the test output.

- [ ] **Step 5 — eval gates:** `make eval-pr` (feature on) → all v5 gate values unchanged.

- [ ] **Step 6 — commit & doc:** `git add crates/memory-mcp/Cargo.toml docs/adr/0028-enable-candle-accelerate-by-default.md && git commit -m "perf: optional Apple Accelerate backend for Candle (ADR-0028)"`

**Rollback if:** F1 drifts, candidate order changes beyond tolerance, or build fails on any target — then keep feature off by default and note per-platform gating.

## Task 3: Batch linked-entity metadata reads (pipeline I/O)

**Why:** `FactService::add_fact` does per-entity namespace-scanning selects (`crates/memory-mcp/src/service/fact.rs:223-246` via `service_context.rs:193-205`). Batching by ID is the highest-confidence pipeline win; visible in the (fixed) default-service bench.

**Files:**
- Modify: `crates/memory-mcp/src/service/fact.rs`
- Modify: `crates/memory-mcp/src/service/service_context.rs`
- Possibly add: batch-by-id query in `crates/memory-mcp/src/storage/episode_store.rs` or `context_store.rs` (new method, not `select_entities_batch` which is name-keyed).

**Interfaces:**
- Consumes: none beyond existing `DbClient::select_one`.
- Produces: `async fn find_entity_records_by_ids(&self, ids: &[String]) -> Result<Vec<Option<Value>>, MemoryError>` on `ServiceContext`, preserving each id's namespace precedence and missing-slot = `None`, with output aligned 1:1 to input order.

- [ ] **Step 1 — failing test** in `service_context` tests: seed two entities in different namespaces; call new batch finder with `[id_a, id_missing, id_b]`; assert `vec[0]`/`vec[2]` populated, `vec[1]` is `None`, order preserved.
Run: `cargo test -p memory_mcp find_entity_records_by_ids_preserves_order_and_missing` → FAIL (method absent).

- [ ] **Step 2 — implement** `find_entity_records_by_ids` building one Surreal query per namespace (`SELECT * FROM entity WHERE entity_id IN $ids` first match wins by namespace order), mapping results back by id.

- [ ] **Step 3 — wire** `FactService::add_fact` to call the batch finder once per fact's `entity_links`, keeping the same per-link canonical/alias extraction.

- [ ] **Step 4 — tests + eval:**
`cargo test -p memory_mcp` and `make eval-pr --features memory_mcp/accelerate`; gates unchanged. Bench delta visible in the fixed default-service pipeline bench, not `entity_f1`.

- [ ] **Step 5 — commit:** `git add crates/memory-mcp && git commit -m "perf: batch linked-entity record lookups during fact creation"`

## Task 4: Duplicate episode read + source-metadata snapshot

**Why:** `ExtractCapability::extract` and `extract_from_episode` both read the episode; `add_fact` re-reads it per fact for project + source_id. Conditional: needs snapshot semantics proof.

- [ ] **Step 1 — failing equivalence test** (new): extract a seeded episode concurrently with a no-op mutation; assert results identical to serial snapshot. This encodes the snapshot guarantee before we code it.
- [ ] **Step 2 — pass pre-parsed episode** through `extract_from_episode(service, episode_id, labels, preloaded: &Episode, namespace)`; keep old behavior behind the default call path for compat (new internal helper, no public surface change).
- [ ] **Step 3 — thread one source metadata snapshot** (project + source_id) into the per-fact loop inside `extract_facts` instead of re-querying.
- [ ] **Step 4 — run:** `cargo test -p memory_mcp`; `make eval-release`; contention bench on the fixed harness must not regress > measurement noise.
- [ ] **Step 5 — commit.**

**Skip condition:** if the snapshot-equivalence test cannot be made to prove sameness under concurrent writers, drop this task entirely. Quality gate precedes optimization.

## Task 5 (DEFERRED — do not schedule): GLiNER allocation micro-opts

Only if Task-1 bench + a profiler (e.g. `samply`) show `run_forward_batch` / `decode_window` non-model allocation overhead > 5% of single-window latency. Previous span-scoring work already cut that stage 20×. Candidates in order: single window fast path avoiding 2D padding alloc; prompt `Vec<String>` cache per label-set; `index_select` gather for label reps. Each ships only with exact-parity test green.

## Explicitly rejected / forbidden under constraints

- gigatoken or any tokenizer replacement (no WordPiece, no Rust Encoding parity) — recorded in ADR-0028 doc and this plan.
- `f16`/quantized weights, Metal as default (Metal stays opt-in; CPU tolerance does NOT accept-Metal; ADR-0028 covers accelerate only).
- Higher `NER_MAX_CONCURRENCY` as default (measured slower/unstable).
- Hard absolute-metric assertions inside Criterion benches.

## Validation summary for the whole plan

After each task: `cargo test -p memory_mcp -p eval-harness` → green; local GLiNER parity test → bitwise same candidates; `make eval-pr` (+ `eval-release` before final merge) → all v5 gate values identical: recall_at_5=1.0000, mrr=0.9924, top_1_hit_rate=0.9848, entity_f1=0.7500, claim_precision=1.0000, claim_recall=1.0000. Any deviation → revert that task's commit.

---

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-08-03-ner-performance-optimization-plan.md` (v3, this content). Two options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute tasks in this session with checkpoints.

Which approach?
