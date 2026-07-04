# Extract NER Latency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `extract` resistant to client timeouts and materially faster for `local-gliner` while preserving the current entity/fact/retrieval quality contract.

**Architecture:** Keep the existing Candle GLiNER model, labels, threshold, window overlap, NMS, entity resolution, and fact pipeline. First add stage-level measurements, then vectorize span scoring, batch transformer windows under explicit memory limits, and serialize expensive local inference with a bounded concurrency gate. Expose the unchanged `extract` intent through MCP 2025-11-25 Tasks as an optional execution mode, retaining synchronous calls for older clients; add Candle Metal only as an opt-in, benchmark-gated backend.

**Tech Stack:** Rust 2024, `rmcp 2.1.0`, Tokio, Candle 0.11 git revision `31f35b14`, GLiNER `urchade/gliner_multi-v2.1`, SurrealDB, existing structured logger and eval suites.

## Global Constraints

- Preserve the current GLiNER weights, tokenizer, default labels, `NER_THRESHOLD=0.5`, maximum span width, window overlap, and NMS behavior.
- The optimized CPU implementation must return the same ordered `(canonical_name, entity_type)` candidates as the reference implementation on the local-model coverage corpus.
- `extract` remains callable synchronously; MCP task support is `optional`, never `required`, so older clients remain compatible.
- Use native `rmcp` Tasks (`tools/call` with task metadata, `tasks/get`, `tasks/result`, `tasks/cancel`) instead of adding proprietary job tools.
- Default local inference concurrency is `1`; this is a stability and tail-latency guard, not a throughput optimization. Operators may raise it only after separately measuring throughput, p95 latency, and peak memory under concurrent load.
- `NER_BATCH_SIZE` limits windows per transformer forward pass; `NER_MAX_BATCH_TOKENS` additionally caps padded tokens so memory use cannot grow from batch size alone.
- Batched and unbatched CPU forward passes must match within `atol=1e-5` and `rtol=1e-4` on non-padding hidden-state values, and must produce exactly the same ordered final candidates. Bitwise float equality is not required.
- CPU remains the default backend. Candle Metal is additive and opt-in. `NER_DEVICE=metal` is strict and fails startup if Metal is unavailable or unsupported; only `NER_DEVICE=auto` may log the failure and fall back to CPU.
- Do not add ONNX Runtime or CoreML in this plan. That would be a runtime migration, not a correction to the current Candle backend.
- Do not multiply independent speedup estimates. Report each benchmark against the same original release baseline and report the final end-to-end ratio separately.
- Run latency measurements with `--release`, `TEST_THREADS=1`, the same model files, the same machine power mode, and no concurrent unrelated model workload.
- Do not optimize entity resolution, fact embeddings, edge storage, contradiction detection, or community rebuilding here. If NER becomes less than 70% of `extract_from_episode.done`, this NER plan is complete after its quality and benchmark gates; open a separate persistence-pipeline plan rather than expanding this scope.
- Before each commit run the task-specific tests; before handoff run `cargo check`, `cargo clippy --all-targets`, `cargo fmt --all --check`, and `cargo test` with zero warnings, failures, or format drift.

---

## Reviewer Feedback Disposition

| Review point | Decision | Reason |
|---|---|---|
| Vectorize per-span `narrow`/FFN/`matmul` loop | Accept | The current implementation launches tiny tensor operations once per candidate span and copies each score row to the host. |
| Expect 5–15x from release mode | Accept as a measurement, not a promise | The root crate is `opt-level = 0` in dev, but dependencies are already `opt-level = 3`; whole-pipeline gain may be much smaller than community anecdotes. |
| Replace “Metal” with CoreML EP | Reject for the current architecture | The repository uses Candle, whose pinned revision exposes the `metal` feature and `Device::new_metal`. CoreML EP applies to an ONNX Runtime migration. |
| Treat combined 10–40x as an upper bound | Accept | Vectorization, batching, and acceleration can remove overlapping bottlenecks and must not be multiplied. |
| Use native MCP Tasks | Accept | Local `rmcp 2.1.0` already implements task models, capabilities, `OperationProcessor`, handler routes, and `#[task_handler]`. |
| Control thread contention | Accept | Multiple `spawn_blocking` GLiNER calls can oversubscribe Candle/Rayon CPU work; an inference semaphore is required. |
| Bound batching memory | Accept | Padding cost is `batch_count × max_sequence_length`; both window count and padded-token budget are required. |
| Add a span-scoring timer | Accept | Existing `ner.extract.done` and `extract_from_episode.done` cannot prove where a speedup came from. |
| State the concurrency tradeoff | Accept | A default of one protects latency and memory but serializes local inference; throughput must be measured separately and may not improve. |
| Formalize strict Metal versus auto fallback | Accept | Explicit `metal` represents operator intent and must fail loudly; `auto` is the only fallback mode. |
| Compare batched and unbatched hidden states | Accept with tolerance | Padding and different GEMM shapes can change float32 accumulation. Use `atol=1e-5`, `rtol=1e-4`, plus exact candidate parity instead of brittle bitwise equality. |
| Treat the 70% gate as a completion branch | Accept | Once persistence dominates, extending this plan would violate scope; documenting the result and opening a separate plan completes this workstream. |

## Scope Check

This document contains three independently shippable milestones with separate review gates:

1. Tasks 1–5: measured CPU inference optimization and bounded resource use.
2. Task 6: protocol-level timeout avoidance through native MCP Tasks.
3. Task 7: optional Apple accelerator support, entered only after CPU results are known.

Task 8 integrates their evidence. Execution may stop after either milestone without leaving the previous milestone incomplete; ONNX/CoreML migration and persistence-pipeline optimization require separate plans.

## File Structure

### New files

- `src/service/entity_extraction/gliner/scoring.rs` — deterministic span-index enumeration and vectorized span tensor gathering.
- `src/service/entity_extraction/gliner/batching.rs` — window metadata and memory-bounded batch packing.
- `src/service/entity_extraction/gliner/gate.rs` — shared inference concurrency gate and queue-wait measurement.
- `tests/eval_ner_latency.rs` — ignored release-only benchmark/equivalence harness producing machine-readable JSON.
- `docs/performance/NER_PERFORMANCE.md` — reproducible benchmark protocol, accepted configuration, and rollout gates.

### Modified files

- `src/service/entity_extraction/gliner.rs` — stage timing, vectorized scoring, batched transformer forward, inference semaphore, and device selection.
- `src/service/entity_extraction.rs` — pass logger/runtime configuration into GLiNER construction.
- `src/config/constants.rs` — defaults for max batch tokens and inference concurrency.
- `src/config/ner.rs` — parse and validate `NER_BATCH_SIZE`, `NER_MAX_BATCH_TOKENS`, `NER_MAX_CONCURRENCY`, and `NER_DEVICE`.
- `src/mcp/handlers.rs` — advertise Tasks, store `OperationProcessor`, mark `extract` task-optional, and install `#[task_handler]`.
- `tests/local_model_integration.rs` — preserve output parity and exercise batching/device configuration.
- `tests/tools_e2e.rs` — cover native task lifecycle for `extract`.
- `Cargo.toml` / `Cargo.lock` — explicit Tokio task features and additive `metal` feature wiring.
- `Makefile` — release server and NER benchmark targets.
- `README.md` — release run commands, NER tuning, task-capable client behavior, and CPU/Metal guidance.
- `.agents/skills/memory-mcp/SKILL.md` — document optional task execution without changing tool arguments.

---

### Task 1: Add Stage Telemetry and a Reproducible Baseline Harness

**Files:**
- Modify: `src/service/entity_extraction/gliner.rs:36-49,497-608,793-834`
- Modify: `src/service/entity_extraction.rs:100-131`
- Create: `tests/eval_ner_latency.rs`
- Create: `docs/performance/NER_PERFORMANCE.md`

**Interfaces:**
- Consumes: existing `StdoutLogger`, `log_event`, `log_args_with_duration`, `EntityExtractor::extract_candidates`.
- Produces: `GlinerEntityExtractor::new_with_logger(...)`, structured event `ner.gliner.span_scores.done`, and ignored test `run_gliner_latency_eval`.

- [ ] **Step 1: Write the failing constructor and event-shape tests**

Add this test beside the existing GLiNER factory tests in `src/service/entity_extraction.rs`:

```rust
#[test]
fn gliner_span_event_has_stable_operation_name() {
    let event = crate::service::entity_extraction::gliner::build_span_scoring_log_event(
        12,
        72,
        std::time::Duration::from_millis(7),
    );
    assert_eq!(event["op"], serde_json::json!("ner.gliner.span_scores.done"));
    assert_eq!(event["args"]["text_words"], serde_json::json!(12));
    assert_eq!(event["result"]["span_count"], serde_json::json!(72));
}
```

- [ ] **Step 2: Run the focused tests and verify they fail**

Run:

```bash
cargo test gliner_span_event_has_stable_operation_name --lib
```

Expected: compilation fails because `build_span_scoring_log_event` does not exist.

- [ ] **Step 3: Add logger-aware GLiNER construction and the span event builder**

Add `logger: crate::logging::StdoutLogger` to `GlinerEntityExtractor`. Rename the current constructor body to `new_with_logger`, pass `logger` through `build_from_var_builder`, and keep the public three-argument constructor for compatibility:

```rust
pub fn new(
    model_dir: &Path,
    labels: Vec<String>,
    threshold: f64,
) -> Result<Self, MemoryError> {
    Self::new_with_logger(
        model_dir,
        labels,
        threshold,
        crate::logging::StdoutLogger::new("warn"),
    )
}

pub(crate) fn new_with_logger(
    model_dir: &Path,
    labels: Vec<String>,
    threshold: f64,
    logger: crate::logging::StdoutLogger,
) -> Result<Self, MemoryError> {
    let tokenizer_path = model_dir.join("tokenizer.json");
    let config_path = if model_dir.join("gliner_config.json").exists() {
        model_dir.join("gliner_config.json")
    } else {
        model_dir.join("config.json")
    };
    let tokenizer = Tokenizer::from_file(&tokenizer_path)
        .map_err(|err| MemoryError::Storage(format!("failed to load tokenizer: {err}")))?;
    let config_str = std::fs::read_to_string(&config_path)
        .map_err(|err| MemoryError::Storage(format!("failed to read config: {err}")))?;
    let safetensors_path = model_dir.join("model.safetensors");
    let pytorch_path = model_dir.join("pytorch_model.bin");
    let runtime_config = if config_path
        .file_name()
        .is_some_and(|name| name == "gliner_config.json")
    {
        parse_gliner_runtime_config(
            &config_str,
            safetensors_path.is_file().then_some(safetensors_path.as_path()),
        )
        .map_err(|err| MemoryError::Storage(format!("failed to parse config: {err}")))?
    } else {
        let backbone: Config = serde_json::from_str(&config_str)
            .map_err(|err| MemoryError::Storage(format!("failed to parse config: {err}")))?;
        GlinerRuntimeConfig {
            head_hidden_size: backbone.hidden_size,
            max_span_width: DEFAULT_MAX_SPAN_WIDTH,
            max_seq_len: backbone.max_position_embeddings.max(DEFAULT_MAX_SEQ_LEN),
            backbone,
        }
    };
    let device = Device::Cpu;
    let vb = if safetensors_path.is_file() {
        unsafe { VarBuilder::from_mmaped_safetensors(&[&safetensors_path], DTYPE, &device) }
            .map_err(|err| MemoryError::Storage(format!("failed to load safetensors: {err}")))?
    } else if pytorch_path.is_file() {
        VarBuilder::from_pth(pytorch_path.to_str().unwrap_or(""), DTYPE, &device)
            .map_err(|err| MemoryError::Storage(format!("failed to load pytorch weights: {err}")))?
    } else {
        return Err(MemoryError::Storage(
            "no model weights found (expected model.safetensors or pytorch_model.bin)".to_string(),
        ));
    };
    Self::build_from_var_builder(
        tokenizer,
        vb,
        &device,
        runtime_config,
        labels,
        threshold,
        logger,
    )
}

pub(crate) fn build_span_scoring_log_event(
    text_words: usize,
    span_count: usize,
    duration: std::time::Duration,
) -> std::collections::HashMap<String, serde_json::Value> {
    crate::service::log_event(
        "ner.gliner.span_scores.done",
        crate::service::log_args_with_duration(
            serde_json::json!({"text_words": text_words}),
            duration,
        ),
        serde_json::json!({"span_count": span_count}),
        None,
        None,
        None,
    )
}
```

Extend `build_from_var_builder` with a final `logger: StdoutLogger` argument and initialize the new struct field with `logger`. Immediately around the current body of `compute_span_scores`, measure elapsed time and emit `build_span_scoring_log_event(..., LogLevel::Debug)`. Update `create_entity_extractor` to call `new_with_logger(..., logger.clone())`.

- [ ] **Step 4: Add the ignored release benchmark**

Create `tests/eval_ner_latency.rs` with one deterministic corpus, warmup, percentile calculation, and JSON output:

```rust
use std::path::PathBuf;
use std::time::Instant;

use memory_mcp::config::{NerConfig, NerProviderKind};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::create_entity_extractor;
use serde::Serialize;

#[derive(Serialize)]
struct NerLatencyReport {
    provider: &'static str,
    iterations: usize,
    content_words: usize,
    p50_ms: f64,
    p95_ms: f64,
    candidates: Vec<(String, String)>,
}

fn percentile(sorted: &[f64], percentile: f64) -> f64 {
    let index = ((sorted.len() - 1) as f64 * percentile).ceil() as usize;
    sorted[index]
}

#[tokio::test]
#[ignore = "requires the local GLiNER model and release-mode timing"]
async fn run_gliner_latency_eval() {
    let model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/models/ner/urchade--gliner_multi-v2.1");
    let mut config = NerConfig::from_env().expect("load NER environment");
    config.provider = NerProviderKind::LocalGliner;
    config.model = Some("urchade/gliner_multi-v2.1".to_string());
    config.model_dir = Some(model_dir.to_string_lossy().to_string());
    let extractor = create_entity_extractor(
        &config,
        env!("CARGO_MANIFEST_DIR"),
        &StdoutLogger::new("debug"),
    )
    .await
    .expect("load local GLiNER model");
    let paragraph = "Alice Smith from OpenAI presented Project Atlas in Moscow using Rust and Kubernetes. ";
    let content = paragraph.repeat(40);

    extractor.extract_candidates(&content).await.expect("warm GLiNER");
    let mut samples = Vec::with_capacity(10);
    let mut last_candidates = Vec::new();
    for _ in 0..10 {
        let started = Instant::now();
        let candidates = extractor.extract_candidates(&content).await.expect("extract entities");
        samples.push(started.elapsed().as_secs_f64() * 1_000.0);
        last_candidates = candidates;
    }
    samples.sort_by(f64::total_cmp);
    let report = NerLatencyReport {
        provider: extractor.provider_name(),
        iterations: samples.len(),
        content_words: content.split_whitespace().count(),
        p50_ms: percentile(&samples, 0.50),
        p95_ms: percentile(&samples, 0.95),
        candidates: last_candidates
            .into_iter()
            .map(|candidate| (candidate.canonical_name, candidate.entity_type))
            .collect(),
    };
    println!("{}", serde_json::to_string(&report).expect("serialize report"));
}
```

- [ ] **Step 5: Capture the original release baseline**

Run:

```bash
TEST_THREADS=1 cargo test --release --test eval_ner_latency run_gliner_latency_eval -- --ignored --exact --nocapture
```

Expected: one JSON report is printed and direct-extractor logs contain `ner.gliner.span_scores.done`. Store the command, machine/OS/CPU description, report JSON, span timer, model name, labels, and threshold in `docs/performance/NER_PERFORMANCE.md` under `Original release baseline`. Record `ner.extract.done` and `extract_from_episode.done` from the end-to-end workload in Task 8; those outer timers are intentionally absent from this direct NER benchmark.

- [ ] **Step 6: Run tests and commit**

Run:

```bash
cargo test --lib entity_extraction
cargo test --test local_model_integration --no-run
cargo fmt --all --check
```

Expected: all commands exit 0.

```bash
git add src/service/entity_extraction.rs src/service/entity_extraction/gliner.rs tests/eval_ner_latency.rs docs/performance/NER_PERFORMANCE.md
git commit -m "perf: baseline gliner extraction stages"
```

---

### Task 2: Make Release Execution the Documented Operational Default

**Files:**
- Modify: `Makefile`
- Modify: `README.md:89-119,638-645`
- Modify: `docs/performance/NER_PERFORMANCE.md`

**Interfaces:**
- Consumes: existing `serve` CLI mode and release profile.
- Produces: `make serve-release` and `make eval-ner-latency` operator commands.

- [ ] **Step 1: Add executable-target checks**

Document these acceptance commands in `docs/performance/NER_PERFORMANCE.md`:

```bash
make -n serve-release
make -n eval-ner-latency
```

Expected before implementation: both commands fail with `No rule to make target`.

- [ ] **Step 2: Add the Make targets**

Add exactly these targets without changing existing targets:

```make
.PHONY: serve-release eval-ner-latency

serve-release:
	cargo run --release -- serve

eval-ner-latency:
	TEST_THREADS=1 cargo test --release --test eval_ner_latency run_gliner_latency_eval -- --ignored --exact --nocapture
```

- [ ] **Step 3: Correct development-oriented server examples**

In README server examples, use `cargo run --release -- serve`; retain `cargo run` only in sections explicitly labeled development. Add this warning directly after the run example:

```markdown
For local NER workloads, run the MCP server from a release build. The development
profile leaves the `memory_mcp` crate at `opt-level = 0`; dependency code is optimized,
but GLiNER window orchestration and span enumeration are not. Performance claims and
timeout investigations are valid only for release builds.
```

- [ ] **Step 4: Verify and commit**

Run:

```bash
make -n serve-release
make -n eval-ner-latency
cargo build --release
```

Expected: dry runs print the exact release commands and the release build exits 0.

```bash
git add Makefile README.md docs/performance/NER_PERFORMANCE.md
git commit -m "docs: make release mode the ner runtime default"
```

---

### Task 3: Vectorize GLiNER Span Scoring

**Files:**
- Create: `src/service/entity_extraction/gliner/scoring.rs`
- Modify: `src/service/entity_extraction/gliner.rs:12-22,793-834`
- Test: `src/service/entity_extraction/gliner/scoring.rs`
- Test: `tests/local_model_integration.rs:451-499`

**Interfaces:**
- Consumes: `Tensor` shaped `[text_words, hidden]`, label representations shaped `[labels, hidden]`, and `SpanRepresentationLayer::forward`.
- Produces: `enumerate_span_indices(text_len, max_span_width) -> Vec<SpanIndex>` and one vectorized score matrix `[spans, labels]`.

- [ ] **Step 1: Write deterministic span-index tests**

Create `src/service/entity_extraction/gliner/scoring.rs` with the tests first:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SpanIndex {
    pub(super) start: usize,
    pub(super) end: usize,
}

pub(super) fn enumerate_span_indices(text_len: usize, max_span_width: usize) -> Vec<SpanIndex> {
    let mut indices = Vec::new();
    for start in 0..text_len {
        for end in start..std::cmp::min(start + max_span_width, text_len) {
            indices.push(SpanIndex { start, end });
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_the_same_inclusive_spans_as_the_reference_loop() {
        assert_eq!(
            enumerate_span_indices(4, 2),
            vec![
                SpanIndex { start: 0, end: 0 },
                SpanIndex { start: 0, end: 1 },
                SpanIndex { start: 1, end: 1 },
                SpanIndex { start: 1, end: 2 },
                SpanIndex { start: 2, end: 2 },
                SpanIndex { start: 2, end: 3 },
                SpanIndex { start: 3, end: 3 },
            ]
        );
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(enumerate_span_indices(0, 12).is_empty());
    }
}
```

- [ ] **Step 2: Run the span tests**

Run:

```bash
cargo test scoring::tests --lib
```

Expected before `mod scoring;` is added: the new tests are not discovered. Add `mod scoring;` to `gliner.rs`, rerun, and expect 2 passed.

- [ ] **Step 3: Replace the per-span tensor loop with one gather/FFN/matmul**

Replace `compute_span_scores` with:

```rust
fn compute_span_scores(
    &self,
    text_hidden: &Tensor,
    label_representations: &Tensor,
) -> Result<Vec<(usize, usize, Vec<f32>)>, MemoryError> {
    let timer = std::time::Instant::now();
    let text_len = text_hidden
        .dim(0)
        .map_err(|err| MemoryError::Storage(format!("dim error: {err}")))?;
    let span_indices = scoring::enumerate_span_indices(text_len, self.max_span_width);
    if span_indices.is_empty() {
        return Ok(Vec::new());
    }

    let starts = span_indices.iter().map(|span| span.start as u32).collect::<Vec<_>>();
    let ends = span_indices.iter().map(|span| span.end as u32).collect::<Vec<_>>();
    let start_indices = Tensor::new(starts.as_slice(), &self.device)
        .map_err(|err| MemoryError::Storage(format!("start index tensor failed: {err}")))?;
    let end_indices = Tensor::new(ends.as_slice(), &self.device)
        .map_err(|err| MemoryError::Storage(format!("end index tensor failed: {err}")))?;
    let start_hidden = text_hidden
        .index_select(&start_indices, 0)
        .map_err(|err| MemoryError::Storage(format!("start gather failed: {err}")))?;
    let end_hidden = text_hidden
        .index_select(&end_indices, 0)
        .map_err(|err| MemoryError::Storage(format!("end gather failed: {err}")))?;
    let span_representations = self
        .span_rep_layer
        .forward(&start_hidden, &end_hidden)
        .map_err(|err| MemoryError::Storage(format!("span projection failed: {err}")))?;
    let label_transposed = label_representations
        .t()
        .map_err(|err| MemoryError::Storage(format!("label transpose failed: {err}")))?;
    let score_rows = span_representations
        .matmul(&label_transposed)
        .map_err(|err| MemoryError::Storage(format!("span score matmul failed: {err}")))?
        .to_vec2::<f32>()
        .map_err(|err| MemoryError::Storage(format!("span score transfer failed: {err}")))?;

    let spans = span_indices
        .into_iter()
        .zip(score_rows)
        .map(|(span, scores)| (span.start, span.end, scores))
        .collect::<Vec<_>>();
    self.logger.log(
        build_span_scoring_log_event(text_len, spans.len(), timer.elapsed()),
        crate::logging::LogLevel::Debug,
    );
    Ok(spans)
}
```

This preserves span order exactly and performs one host transfer instead of one per span.

- [ ] **Step 4: Verify output quality parity**

Run:

```bash
TEST_THREADS=1 cargo test --release --test local_model_integration local_gliner_extractor_detects_all_default_supported_entities_across_diverse_texts -- --ignored --exact --nocapture
TEST_THREADS=1 cargo test --release --test local_model_integration local_gliner_extractor_detects_custom_zero_shot_entities -- --ignored --exact --nocapture
```

Expected: both pass with the same candidate names/types as the original baseline report. If ordering or accepted spans differ, do not loosen thresholds; fix shape/order/numerical handling.

- [ ] **Step 5: Measure the isolated and full effect**

Run:

```bash
make eval-ner-latency
```

Expected: the JSON candidate list matches the original release baseline. Record span-scoring p50 and full NER p50/p95 as separate ratios against the original baseline; do not multiply them.

- [ ] **Step 6: Commit**

```bash
cargo test scoring::tests --lib
cargo fmt --all --check
git add src/service/entity_extraction/gliner.rs src/service/entity_extraction/gliner/scoring.rs tests/local_model_integration.rs docs/performance/NER_PERFORMANCE.md
git commit -m "perf: vectorize gliner span scoring"
```

---

### Task 4: Batch Transformer Windows Under a Token Budget

**Files:**
- Create: `src/service/entity_extraction/gliner/batching.rs`
- Modify: `src/service/entity_extraction/gliner.rs:36-57,630-790,923-999`
- Modify: `src/config/constants.rs:21-26`
- Modify: `src/config/ner.rs:22-103`
- Modify: `src/service/entity_extraction.rs:100-131`
- Test: `src/service/entity_extraction/gliner/batching.rs`
- Test: `src/config/ner.rs`

**Interfaces:**
- Consumes: encoded windows with `input_ids`, `word_ids`, and source word boundaries.
- Produces: `pack_window_batches(&[EncodedWindow], batch_size, max_batch_tokens) -> Vec<Range<usize>>` and `run_forward_batch(&[EncodedWindow]) -> Result<Vec<Tensor>, MemoryError>`.

- [ ] **Step 1: Write batch-packing tests**

Create `batching.rs` with:

```rust
use std::ops::Range;

#[derive(Debug)]
pub(super) struct EncodedWindow {
    pub(super) input_ids: Vec<u32>,
    pub(super) word_ids: Vec<Option<u32>>,
    pub(super) window_start: usize,
    pub(super) window_end: usize,
}

pub(super) fn pack_window_batches(
    windows: &[EncodedWindow],
    max_windows: usize,
    max_padded_tokens: usize,
) -> Vec<Range<usize>> {
    let mut batches = Vec::new();
    let mut start = 0;
    while start < windows.len() {
        let mut end = start;
        let mut longest = 0;
        while end < windows.len() && end - start < max_windows {
            let candidate_longest = longest.max(windows[end].input_ids.len());
            let candidate_count = end - start + 1;
            if candidate_count > 1 && candidate_longest * candidate_count > max_padded_tokens {
                break;
            }
            longest = candidate_longest;
            end += 1;
        }
        batches.push(start..end.max(start + 1));
        start = end.max(start + 1);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(tokens: usize) -> EncodedWindow {
        EncodedWindow {
            input_ids: vec![1; tokens],
            word_ids: vec![Some(0); tokens],
            window_start: 0,
            window_end: tokens,
        }
    }

    #[test]
    fn respects_window_and_padded_token_limits() {
        let windows = vec![window(100), window(120), window(300), window(300)];
        assert_eq!(pack_window_batches(&windows, 4, 480), vec![0..2, 2..3, 3..4]);
    }

    #[test]
    fn always_makes_progress_for_one_oversized_window() {
        let windows = vec![window(600)];
        assert_eq!(pack_window_batches(&windows, 4, 384), vec![0..1]);
    }
}
```

- [ ] **Step 2: Add and validate configuration**

Add constants:

```rust
pub const DEFAULT_NER_MAX_BATCH_TOKENS: usize = 1536;
pub const DEFAULT_NER_MAX_CONCURRENCY: usize = 1;
```

Add `max_batch_tokens: usize` and `max_concurrency: usize` to `NerConfig`, parse `NER_MAX_BATCH_TOKENS` and `NER_MAX_CONCURRENCY`, and reject zero values:

```rust
if batch_size == 0 || max_batch_tokens == 0 || max_concurrency == 0 {
    return Err(MemoryError::ConfigInvalid(
        "NER_BATCH_SIZE, NER_MAX_BATCH_TOKENS, and NER_MAX_CONCURRENCY must be greater than zero"
            .to_string(),
    ));
}
```

Add this test helper and table-driven coverage in a new `#[cfg(test)] mod tests` in `src/config/ner.rs`:

```rust
use std::sync::{Mutex, OnceLock};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn with_ner_env(vars: &[(&str, Option<&str>)], test: impl FnOnce()) {
    let _guard = env_lock().lock().expect("NER env lock");
    let saved = vars
        .iter()
        .map(|(key, _)| ((*key).to_string(), std::env::var(key).ok()))
        .collect::<Vec<_>>();
    for (key, value) in vars {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(test));
    for (key, value) in saved {
        unsafe {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
    }
    outcome.expect("NER config test body");
}

#[test]
fn ner_runtime_limits_have_safe_defaults() {
    with_ner_env(
        &[
            ("NER_BATCH_SIZE", None),
            ("NER_MAX_BATCH_TOKENS", None),
            ("NER_MAX_CONCURRENCY", None),
        ],
        || {
            let config = NerConfig::from_env().expect("default NER config");
            assert_eq!(config.batch_size, 4);
            assert_eq!(config.max_batch_tokens, 1536);
            assert_eq!(config.max_concurrency, 1);
        },
    );
}

#[test]
fn ner_runtime_limits_reject_zero() {
    for key in ["NER_BATCH_SIZE", "NER_MAX_BATCH_TOKENS", "NER_MAX_CONCURRENCY"] {
        with_ner_env(&[(key, Some("0"))], || {
            assert!(matches!(NerConfig::from_env(), Err(MemoryError::ConfigInvalid(_))));
        });
    }
}
```

- [ ] **Step 3: Implement padded batched transformer forward**

Add `batch_size` and `max_batch_tokens` fields to `GlinerEntityExtractor`. Keep `new(...)` and `new_with_logger(...)` as compatibility wrappers using the constants, move the loader body into this constructor, and make `create_entity_extractor` pass configured values:

```rust
pub(crate) fn new_with_runtime(
    model_dir: &Path,
    labels: Vec<String>,
    threshold: f64,
    batch_size: usize,
    max_batch_tokens: usize,
    logger: crate::logging::StdoutLogger,
) -> Result<Self, MemoryError> {
    if batch_size == 0 || max_batch_tokens == 0 {
        return Err(MemoryError::ConfigInvalid(
            "NER batch limits must be greater than zero".to_string(),
        ));
    }
    Self::load_with_runtime(
        model_dir,
        labels,
        threshold,
        batch_size,
        max_batch_tokens,
        logger,
    )
}
```

Rename the Task 1 loader body to `load_with_runtime`, extend `build_from_var_builder` with `batch_size` and `max_batch_tokens`, and initialize both struct fields. Then add this method to `GlinerEntityExtractor`:

```rust
fn run_forward_batch(
    &self,
    windows: &[batching::EncodedWindow],
) -> Result<Vec<Tensor>, MemoryError> {
    let batch_size = windows.len();
    let max_len = windows.iter().map(|window| window.input_ids.len()).max().unwrap_or(0);
    let pad_id = self.tokenizer.get_padding().map_or(0, |padding| padding.pad_id);
    let mut ids = vec![vec![pad_id; max_len]; batch_size];
    let mut masks = vec![vec![0u32; max_len]; batch_size];
    for (row, window) in windows.iter().enumerate() {
        ids[row][..window.input_ids.len()].copy_from_slice(&window.input_ids);
        masks[row][..window.input_ids.len()].fill(1);
    }
    let input_ids = Tensor::new(ids, &self.device)
        .map_err(|err| MemoryError::Storage(format!("batched input tensor failed: {err}")))?;
    let attention_mask = Tensor::new(masks, &self.device)
        .map_err(|err| MemoryError::Storage(format!("batched mask tensor failed: {err}")))?;
    let type_ids = Tensor::zeros_like(&input_ids)
        .map_err(|err| MemoryError::Storage(format!("batched type ids failed: {err}")))?;
    let hidden = self.model
        .forward(&input_ids, Some(type_ids), Some(attention_mask))
        .map_err(|err| MemoryError::Storage(format!("batched forward pass failed: {err}")))?;
    windows.iter().enumerate().map(|(row, window)| {
        hidden
            .narrow(0, row, 1)
            .and_then(|tensor| tensor.squeeze(0))
            .and_then(|tensor| tensor.narrow(0, 0, window.input_ids.len()))
            .map_err(|err| MemoryError::Storage(format!("batched hidden split failed: {err}")))
    }).collect()
}
```

- [ ] **Step 4: Rework `extract_inner_with_labels` into encode, pack, forward, decode phases**

Add this window decoder method:

```rust
fn decode_window(
    &self,
    text: &str,
    text_words: &[(String, (usize, usize))],
    labels: &[String],
    prompt_word_count: usize,
    window: &batching::EncodedWindow,
    hidden: &Tensor,
    all_spans: &mut Vec<ScoredSpan>,
) -> Result<(), MemoryError> {
    let entity_token_positions = self.collect_prompt_entity_positions(
        &window.input_ids,
        &window.word_ids,
        prompt_word_count,
    );
    if entity_token_positions.len() != labels.len() {
        return Err(MemoryError::Storage(format!(
            "GLiNER prompt extraction mismatch: expected {} entity tokens, found {}",
            labels.len(),
            entity_token_positions.len()
        )));
    }
    let label_representations =
        self.build_label_representations(hidden, &entity_token_positions)?;
    let window_offsets = text_words[window.window_start..window.window_end]
        .iter()
        .map(|(_, offsets)| *offsets)
        .collect::<Vec<_>>();
    let (word_hidden, word_offsets) = self.extract_word_representations(
        hidden,
        &window.word_ids,
        prompt_word_count,
        &window_offsets,
    )?;
    let text_hidden = self
        .rnn
        .forward(&word_hidden)
        .map_err(|err| MemoryError::Storage(format!("rnn forward failed: {err}")))?;
    let spans_data = self.compute_span_scores(&text_hidden, &label_representations)?;
    all_spans.extend(self.extract_spans(text, &spans_data, &word_offsets));
    Ok(())
}
```

Replace the sequential window loop in `extract_inner_with_labels` with this complete encode-and-batch flow, preserving the existing overlap rule:

```rust
let mut windows = Vec::new();
let mut window_start = 0;
while window_start < text_words.len() {
    let (encoding, window_end) = self.encode_window(&prompt_words, &text_words, window_start)?;
    windows.push(batching::EncodedWindow {
        input_ids: encoding.get_ids().to_vec(),
        word_ids: encoding.get_word_ids().to_vec(),
        window_start,
        window_end,
    });
    if window_end >= text_words.len() {
        break;
    }
    window_start = window_end.saturating_sub(1).max(window_start + 1);
}

for range in batching::pack_window_batches(
    &windows,
    self.batch_size,
    self.max_batch_tokens,
) {
    let batch = &windows[range];
    let hidden_rows = self.run_forward_batch(batch)?;
    for (window, hidden) in batch.iter().zip(hidden_rows) {
        self.decode_window(
            text,
            &text_words,
            labels,
            prompt_word_count,
            window,
            &hidden,
            &mut all_spans,
        )?;
    }
}
```

- [ ] **Step 5: Compare padded batched hidden states with the unbatched CPU path**

Add this ignored unit test inside `src/service/entity_extraction/gliner.rs`, where it can call the private encoding and forward methods. The shorter target window is deliberately batched with a longer window so the target receives real padding:

```rust
#[tokio::test(flavor = "current_thread")]
#[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
async fn batched_forward_matches_unbatched_hidden_states_with_padding() {
    const ATOL: f32 = 1e-5;
    const RTOL: f32 = 1e-4;

    let model_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/models/ner/urchade--gliner_multi-v2.1");
    let labels = vec![
        "person".to_string(),
        "company".to_string(),
        "location".to_string(),
        "product".to_string(),
        "event".to_string(),
        "technology".to_string(),
    ];
    let extractor = GlinerEntityExtractor::new(&model_dir, labels.clone(), 0.5)
        .expect("load local GLiNER model");
    let prompt_words = extractor.build_prompt_words_for_labels(&labels);

    let encode = |text: &str| {
        let words = GlinerEntityExtractor::split_text_words(text);
        let (encoding, window_end) = extractor
            .encode_window(&prompt_words, &words, 0)
            .expect("encode GLiNER window");
        batching::EncodedWindow {
            input_ids: encoding.get_ids().to_vec(),
            word_ids: encoding.get_word_ids().to_vec(),
            window_start: 0,
            window_end,
        }
    };

    let short = encode("Alice Smith joined OpenAI in Moscow.");
    let long = encode(
        "Alice Smith joined OpenAI in Moscow and presented Project Atlas using Rust, Kubernetes, PostgreSQL, and Candle at the annual engineering summit.",
    );
    assert!(short.input_ids.len() < long.input_ids.len());

    let solo = extractor
        .run_forward(&short.input_ids)
        .expect("unbatched forward")
        .to_vec2::<f32>()
        .expect("unbatched hidden values");
    let batched = extractor
        .run_forward_batch(&[short, long])
        .expect("batched forward");
    let padded_short = batched[0]
        .to_vec2::<f32>()
        .expect("batched hidden values");

    assert_eq!(solo.len(), padded_short.len());
    assert_eq!(solo.first().map(Vec::len), padded_short.first().map(Vec::len));
    for (expected, actual) in solo.iter().flatten().zip(padded_short.iter().flatten()) {
        assert!(expected.is_finite() && actual.is_finite());
        let tolerance = ATOL + RTOL * expected.abs();
        assert!(
            (expected - actual).abs() <= tolerance,
            "hidden-state mismatch: expected={expected} actual={actual} tolerance={tolerance}"
        );
    }
}
```

Run:

```bash
TEST_THREADS=1 cargo test --release batched_forward_matches_unbatched_hidden_states_with_padding --lib -- --ignored --exact --nocapture
```

Expected: PASS on CPU. A failure blocks batching even if the final candidate set happens to match.

- [ ] **Step 6: Verify candidate parity and memory bounds**

Run the same local-model corpus twice with `NER_BATCH_SIZE=1` and `NER_BATCH_SIZE=4`, keeping `NER_MAX_BATCH_TOKENS=1536`:

```bash
NER_BATCH_SIZE=1 TEST_THREADS=1 cargo test --release --test local_model_integration memory_service_uses_local_gliner_defaults_across_diverse_texts -- --ignored --exact --nocapture
NER_BATCH_SIZE=4 NER_MAX_BATCH_TOKENS=1536 TEST_THREADS=1 cargo test --release --test local_model_integration memory_service_uses_local_gliner_defaults_across_diverse_texts -- --ignored --exact --nocapture
```

Expected: both pass and return identical candidates. Add a debug log containing `window_count`, `batch_count`, `largest_batch`, and `max_padded_tokens`; verify no batch exceeds the configured token budget except a single intrinsically oversized window.

- [ ] **Step 7: Measure and commit**

```bash
make eval-ner-latency
cargo test batching::tests --lib
cargo test config::ner --lib
cargo fmt --all --check
git add src/config/constants.rs src/config/ner.rs src/service/entity_extraction.rs src/service/entity_extraction/gliner.rs src/service/entity_extraction/gliner/batching.rs tests/local_model_integration.rs docs/performance/NER_PERFORMANCE.md
git commit -m "perf: batch gliner windows within a token budget"
```

---

### Task 5: Bound Concurrent Local NER Inference

**Files:**
- Create: `src/service/entity_extraction/gliner/gate.rs`
- Modify: `src/service/entity_extraction/gliner.rs:36-49,556-608,1028-1042`
- Test: `src/service/entity_extraction/gliner/gate.rs`

**Interfaces:**
- Consumes: `NerConfig::max_concurrency`.
- Produces: `InferenceGate::acquire() -> (OwnedSemaphorePermit, Duration)`, shared by all calls through one GLiNER extractor, plus queue-wait telemetry.

- [ ] **Step 1: Write the concurrency test**

Create `src/service/entity_extraction/gliner/gate.rs` with the implementation and deterministic tests:

```rust
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Debug, Clone)]
pub(super) struct InferenceGate {
    permits: Arc<Semaphore>,
}

impl InferenceGate {
    pub(super) fn new(max_concurrency: usize) -> Self {
        Self { permits: Arc::new(Semaphore::new(max_concurrency)) }
    }

    pub(super) async fn acquire(
        &self,
    ) -> Result<(OwnedSemaphorePermit, Duration), tokio::sync::AcquireError> {
        let started = Instant::now();
        let permit = self.permits.clone().acquire_owned().await?;
        Ok((permit, started.elapsed()))
    }

    pub(super) fn available_permits(&self) -> usize {
        self.permits.available_permits()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn second_caller_waits_until_the_only_permit_is_released() {
        let gate = InferenceGate::new(1);
        let (first, _) = gate.acquire().await.expect("first permit");
        assert!(tokio::time::timeout(Duration::from_millis(10), gate.acquire()).await.is_err());
        drop(first);
        assert!(tokio::time::timeout(Duration::from_millis(100), gate.acquire()).await.is_ok());
    }

    #[tokio::test]
    async fn configured_parallelism_is_available() {
        let gate = InferenceGate::new(2);
        let (_first, _) = gate.acquire().await.expect("first permit");
        let (_second, _) = gate.acquire().await.expect("second permit");
        assert_eq!(gate.available_permits(), 0);
    }
}
```

- [ ] **Step 2: Add the inference semaphore**

Add `mod gate;`, extend `new_with_runtime`, `load_with_runtime`, and `build_from_var_builder` with `max_concurrency: usize`, and make `create_entity_extractor` pass `config.max_concurrency`. Reject zero before model loading:

```rust
if max_concurrency == 0 {
    return Err(MemoryError::ConfigInvalid(
        "NER_MAX_CONCURRENCY must be greater than zero".to_string(),
    ));
}
```

Initialize `InferenceGate::new(max_concurrency)` and store this field:

```rust
inference_gate: gate::InferenceGate,
```

Acquire it in both GLiNER trait methods before entering synchronous inference:

```rust
let (_permit, queue_wait) = self
    .inference_gate
    .acquire()
    .await
    .map_err(|_| MemoryError::Storage("GLiNER inference gate closed".to_string()))?;
self.logger.log(
    crate::service::log_event(
        "ner.gliner.queue.done",
        crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
        serde_json::json!({"available_permits": self.inference_gate.available_permits()}),
        None,
        None,
        None,
    ),
    crate::logging::LogLevel::Debug,
);
self.extract_inner(content)
```

Implement the custom-label method with the same single permit:

```rust
async fn extract_candidates_with_labels(
    &self,
    content: &str,
    zero_shot_labels: &[String],
) -> Result<Vec<EntityCandidate>, MemoryError> {
    let (_permit, queue_wait) = self
        .inference_gate
        .acquire()
        .await
        .map_err(|_| MemoryError::Storage("GLiNER inference gate closed".to_string()))?;
    self.logger.log(
        crate::service::log_event(
            "ner.gliner.queue.done",
            crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
            serde_json::json!({"available_permits": self.inference_gate.available_permits()}),
            None,
            None,
            None,
        ),
        crate::logging::LogLevel::Debug,
    );
    self.extract_inner_with_labels(content, zero_shot_labels)
}
```

Do not acquire a second permit in `extract_inner` or `extract_inner_with_labels`.

- [ ] **Step 3: Exercise contention in release mode**

Extend `eval_ner_latency` with a four-request scenario using `tokio::join!`, reporting `wall_ms`, individual completion times, and queue wait. Run with concurrency 1 and 2.

Expected: concurrency 1 prevents simultaneous model forwards and avoids throughput collapse, but may increase queue wait and does not claim higher throughput. Report throughput, queue-wait p95, request p95, and peak RSS as separate metrics. Concurrency 2 is accepted only if total throughput improves without request-p95 regression above 20% and without exceeding the deployment memory budget.

- [ ] **Step 4: Commit**

```bash
cargo test gate::tests --lib
make eval-ner-latency
cargo fmt --all --check
git add src/service/entity_extraction/gliner.rs src/service/entity_extraction/gliner/gate.rs tests/eval_ner_latency.rs docs/performance/NER_PERFORMANCE.md
git commit -m "perf: bound concurrent gliner inference"
```

---

### Task 6: Expose `extract` Through Native MCP Tasks

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/mcp/handlers.rs:1-125,201-215`
- Modify: `tests/tools_e2e.rs`
- Modify: `.agents/skills/memory-mcp/SKILL.md`

**Interfaces:**
- Consumes: `rmcp::task_manager::OperationProcessor`, `#[task_handler]`, and existing `MemoryMcp::extract` tool handler.
- Produces: server Tasks capability, task-optional `extract`, and native task lifecycle endpoints without new public tools.

- [ ] **Step 1: Write capability and tool-metadata tests**

Add to `src/mcp/handlers.rs` tests:

```rust
#[test]
fn build_server_info_enables_native_tasks() {
    let value = serde_json::to_value(MemoryMcp::build_server_info()).expect("server info json");
    assert!(value["capabilities"]["tasks"].is_object());
    assert!(value["capabilities"]["tasks"]["requests"]["tools"]["call"].is_object());
}

#[tokio::test]
async fn only_extract_allows_task_execution() {
    let mcp = create_test_mcp().await;
    assert_eq!(mcp.get_tool("extract").expect("extract tool").task_support(), rmcp::model::TaskSupport::Optional);
    assert_eq!(mcp.get_tool("ingest").expect("ingest tool").task_support(), rmcp::model::TaskSupport::Forbidden);
}
```

- [ ] **Step 2: Run the tests and verify failure**

```bash
cargo test build_server_info_enables_native_tasks --lib
cargo test only_extract_allows_task_execution --lib
```

Expected: the Tasks capability assertion and extract task-support assertion fail.

- [ ] **Step 3: Install `OperationProcessor` and task handler generation**

Make Tokio task features explicit:

```toml
tokio = { version = "1.52.3", features = [
    "rt-multi-thread",
    "macros",
    "io-std",
    "sync",
    "time"
] }
```

Update imports, state, constructor, capability, and handler attributes:

```rust
use rmcp::model::TasksCapability;
use rmcp::{ErrorData, RoleServer, ServerHandler, task_handler, tool, tool_handler, tool_router};

type McpError = rmcp::ErrorData;

#[derive(Clone)]
pub struct MemoryMcp {
    service: Arc<MemoryService>,
    session_manager: SessionManager,
    task_processor: Arc<tokio::sync::Mutex<rmcp::task_manager::OperationProcessor>>,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

pub fn new(service: MemoryService) -> Self {
    Self {
        service: Arc::new(service),
        session_manager: SessionManager::new(),
        task_processor: Arc::new(tokio::sync::Mutex::new(
            rmcp::task_manager::OperationProcessor::new(),
        )),
        tool_router: Self::tool_router(),
    }
}

#[tool_handler(router = self.tool_router)]
#[task_handler(processor = self.task_processor)]
impl ServerHandler for MemoryMcp {
```

Add `.enable_tasks_with(TasksCapability::server_default())` to `build_server_info`.

Note: `McpError` is a private type alias in rmcp 2.1.0; define `type McpError = rmcp::ErrorData;` in scope for the `#[task_handler]` macro. Use `enable_tasks_with(TasksCapability::server_default())` (not `enable_tasks()`) to populate `list`, `cancel`, and `requests.tools.call` sub-capabilities.

- [ ] **Step 4: Mark only `extract` as task-optional**

Change the extract attribute to:

```rust
#[tool(
    execution(task_support = "optional"),
    description = "Extract entities, facts, and relationships from remembered content. Use this tool when you need structured knowledge from an existing episode or from new inline content. Prefer task-based invocation when the client supports MCP Tasks or when local NER may exceed the client's synchronous timeout. Do not use this tool for retrieval. Arguments must be a flat snake_case object. Provide exactly one input source: `episode_id` for stored content, or inline `content`/`text`; optional fields are `source_type`, `source_id`, `t_ref`, `scope`, and `zero_shot_labels`. Do not wrap arguments in `payload`. If you pass inline content, the server ingests it first and then extracts facts. Returns extracted entities, facts, and links."
)]
```

Do not add `task_id` to `ToolResponse`; native `CreateTaskResult` is the task-mode response.

- [ ] **Step 5: Add an end-to-end task lifecycle test**

In `tests/tools_e2e.rs`, build a task-augmented `CallToolRequestParams`, dispatch it through `ServerHandler::handle_request`, assert `TaskStatus::Working`, poll `get_task_info` until terminal, and obtain `GetTaskPayloadResult`. The final deserialized payload must equal the synchronous `ToolResponse<ExtractResult>` shape and include the same `episode_id`, entities, facts, and links.

Use this context helper and a 5-second test deadline:

```rust
fn task_test_context(id: i64) -> rmcp::service::RequestContext<rmcp::RoleServer> {
    let (peer, _receiver) = rmcp::service::Peer::new(
        std::sync::Arc::new(rmcp::service::AtomicU32RequestIdProvider::default()),
        None,
    );
    rmcp::service::RequestContext::new(rmcp::model::RequestId::Number(id), peer)
}

let task_request: rmcp::model::CallToolRequestParams = serde_json::from_value(serde_json::json!({
    "name": "extract",
    "arguments": {"episode_id": episode_id},
    "task": {"ttl": 60_000}
}))
.expect("task-augmented extract params");
let create_result = rmcp::ServerHandler::enqueue_task(
    &mcp,
    task_request,
    task_test_context(1),
)
.await
.expect("enqueue extract task");
assert_eq!(create_result.task.status, rmcp::model::TaskStatus::Working);
let task_id = create_result.task.task_id;
let get_params: rmcp::model::GetTaskInfoParams = serde_json::from_value(
    serde_json::json!({"taskId": task_id}),
)
.expect("get task params");
let result_params: rmcp::model::GetTaskResultParams = serde_json::from_value(
    serde_json::json!({"taskId": task_id}),
)
.expect("get task result params");
let context = task_test_context(2);

let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
    loop {
        let status = mcp
            .get_task_info(get_params.clone(), context.clone())
            .await
            .expect("read task status");
        if matches!(status.task.status, rmcp::model::TaskStatus::Completed) {
            break mcp
                .get_task_result(result_params.clone(), context.clone())
                .await
                .expect("read task result");
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
})
.await
.expect("task lifecycle must not hang");
```

Also test cancellation and unknown task IDs. State explicitly in the test name that cancelling a Tokio task cannot preempt a Candle kernel already running inside `spawn_blocking`; cancellation prevents subsequent pipeline stages and discards the result.

- [ ] **Step 6: Update the memory skill contract**

Add this paragraph under `Extract Entities and Facts` in `.agents/skills/memory-mcp/SKILL.md`:

```markdown
When the client supports MCP Tasks, prefer task-based invocation for `extract` with
local GLiNER or long documents. The tool is task-optional: legacy clients may call it
synchronously, while task-capable clients receive a native MCP task and use
`tasks/get` followed by `tasks/result`. Do not invent a `job_id` argument or call a
separate polling tool.
```

- [ ] **Step 7: Verify and commit**

```bash
cargo test build_server_info_enables_native_tasks --lib
cargo test only_extract_allows_task_execution --lib
cargo test --test tools_e2e
cargo check
cargo fmt --all --check
git add Cargo.toml Cargo.lock src/mcp/handlers.rs tests/tools_e2e.rs .agents/skills/memory-mcp/SKILL.md
git commit -m "feat: run extract through optional mcp tasks"
```

---

### Task 7: Add an Opt-In Candle Metal Backend With CPU Fallback

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/config/ner.rs`
- Modify: `src/service/entity_extraction/gliner.rs:497-555`
- Modify: `tests/local_model_integration.rs`
- Modify: `README.md`
- Modify: `docs/performance/NER_PERFORMANCE.md`

**Interfaces:**
- Consumes: Candle `metal` feature and `Device::new_metal(0)`.
- Produces: `NerDeviceKind::{Cpu, Metal, Auto}` and `NER_DEVICE=cpu|metal|auto`; no ONNX/CoreML dependency.

- [ ] **Step 1: Write device parsing tests**

Add the public config enum and `device` field to `NerConfig`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NerDeviceKind {
    Cpu,
    Metal,
    Auto,
}
```

Set `device: NerDeviceKind::Cpu` in `Default`. In `from_env`, parse the variable with:

```rust
let device = match std::env::var("NER_DEVICE")
    .unwrap_or_else(|_| "cpu".to_string())
    .trim()
    .to_ascii_lowercase()
    .as_str()
{
    "cpu" => NerDeviceKind::Cpu,
    "metal" => NerDeviceKind::Metal,
    "auto" => NerDeviceKind::Auto,
    other => {
        return Err(MemoryError::ConfigInvalid(format!(
            "unsupported NER_DEVICE `{other}`; expected cpu, metal, or auto"
        )));
    }
};
```

Store `device` in the returned config, then add these tests to the existing `src/config/ner.rs` test module:

```rust
#[test]
fn ner_device_defaults_to_cpu() {
    with_ner_env(&[("NER_DEVICE", None)], || {
        assert_eq!(NerConfig::from_env().unwrap().device, NerDeviceKind::Cpu);
    });
}

#[test]
fn ner_device_accepts_metal_and_auto() {
    for (raw, expected) in [("metal", NerDeviceKind::Metal), ("auto", NerDeviceKind::Auto)] {
        with_ner_env(&[("NER_DEVICE", Some(raw))], || {
            assert_eq!(NerConfig::from_env().unwrap().device, expected);
        });
    }
}

#[test]
fn ner_device_rejects_unknown_backend() {
    with_ner_env(&[("NER_DEVICE", Some("coreml"))], || {
        assert!(matches!(NerConfig::from_env(), Err(MemoryError::ConfigInvalid(_))));
    });
}
```

- [ ] **Step 2: Add additive Cargo features**

Use this feature mapping:

```toml
[features]
default = []
cli-watch = ["notify"]
mcp-apps = []
metal = ["candle-core/metal", "candle-nn/metal", "candle-transformers/metal"]
```

- [ ] **Step 3: Add explicit device selection**

Implement:

```rust
fn select_device(
    kind: crate::config::NerDeviceKind,
    logger: &crate::logging::StdoutLogger,
) -> Result<Device, MemoryError> {
    match kind {
        crate::config::NerDeviceKind::Cpu => Ok(Device::Cpu),
        crate::config::NerDeviceKind::Metal => {
            #[cfg(feature = "metal")]
            {
                Device::new_metal(0).map_err(|err| {
                    MemoryError::ConfigInvalid(format!("failed to initialize Metal NER device: {err}"))
                })
            }
            #[cfg(not(feature = "metal"))]
            {
                Err(MemoryError::ConfigInvalid(
                    "NER_DEVICE=metal requires building with --features metal".to_string(),
                ))
            }
        }
        crate::config::NerDeviceKind::Auto => {
            #[cfg(feature = "metal")]
            {
                match Device::new_metal(0) {
                    Ok(device) => Ok(device),
                    Err(error) => {
                        logger.log(
                            crate::service::log_event(
                                "ner.device.fallback",
                                serde_json::json!({"requested": "metal", "error": error.to_string()}),
                                serde_json::json!({"selected": "cpu"}),
                                None,
                                None,
                                None,
                            ),
                            crate::logging::LogLevel::Warn,
                        );
                        Ok(Device::Cpu)
                    }
                }
            }
            #[cfg(not(feature = "metal"))]
            {
                Ok(Device::Cpu)
            }
        }
    }
}
```

Extend `new_with_runtime` and `load_with_runtime` with `device_kind: NerDeviceKind`, make `create_entity_extractor` pass `config.device`, and replace `let device = Device::Cpu;` with `let device = select_device(device_kind, &logger)?;`. The public compatibility constructor continues to pass `NerDeviceKind::Cpu`. Log the selected device at startup.

- [ ] **Step 4: Run the Metal compatibility and quality gate**

On Apple Silicon:

```bash
NER_DEVICE=metal TEST_THREADS=1 cargo test --release --features metal --test local_model_integration memory_service_uses_local_gliner_defaults_across_diverse_texts -- --ignored --exact --nocapture
NER_DEVICE=metal TEST_THREADS=1 cargo test --release --features metal --test eval_ner_latency run_gliner_latency_eval -- --ignored --exact --nocapture
```

Expected: every operator used by the real model runs on Metal, candidate output matches CPU, and the benchmark completes. With `NER_DEVICE=metal`, initialization or operator failure is a hard error and must never fall back. If an operator fails or candidates differ, keep Metal opt-in and document the failing operation; do not select Metal for `auto` until parity passes.

- [ ] **Step 5: Apply the rollout gate**

Enable `NER_DEVICE=auto` in deployment documentation only if all conditions hold:

- default and zero-shot candidate sets match CPU;
- extraction eval entity recall/F1 do not decrease;
- end-to-end NER p95 improves by at least 20% against the original CPU release baseline;
- peak RSS stays inside the deployment limit;
- four-client contention does not regress total throughput.

Otherwise document `NER_DEVICE=cpu` as the production setting. Do not introduce ONNX Runtime/CoreML as a fallback inside this task.

- [ ] **Step 6: Commit**

```bash
cargo test config::ner --lib
cargo check --features metal
cargo fmt --all --check
git add Cargo.toml Cargo.lock src/config/ner.rs src/service/entity_extraction/gliner.rs tests/local_model_integration.rs README.md docs/performance/NER_PERFORMANCE.md
git commit -m "feat: add opt-in candle metal ner backend"
```

---

### Task 8: Run Quality, Latency, Load, and Compatibility Gates

**Files:**
- Modify: `docs/performance/NER_PERFORMANCE.md`
- Modify: `docs/EVAL_BASELINE.md`
- Modify: `README.md`

**Interfaces:**
- Consumes: all prior tasks and the original release baseline.
- Produces: one decision-ready rollout record with per-stage and end-to-end ratios, selected runtime settings, and compatibility results.

- [ ] **Step 1: Run deterministic quality checks**

```bash
cargo test
TEST_THREADS=1 cargo test --release --test local_model_integration -- --ignored --nocapture
TEST_THREADS=1 cargo test --test eval_extraction run_extraction_evals -- --ignored --exact --nocapture
make eval-quick
```

Expected: zero failures; GLiNER candidate parity holds; extraction entity recall and F1 are not below the recorded baseline; retrieval/fact metrics do not regress.

- [ ] **Step 2: Run the final CPU benchmark**

```bash
NER_DEVICE=cpu NER_BATCH_SIZE=4 NER_MAX_BATCH_TOKENS=1536 NER_MAX_CONCURRENCY=1 make eval-ner-latency
NER_DEVICE=cpu NER_BATCH_SIZE=4 NER_MAX_BATCH_TOKENS=1536 NER_MAX_CONCURRENCY=1 TEST_THREADS=1 cargo test --release --test local_model_integration memory_service_uses_local_gliner_defaults_across_diverse_texts -- --ignored --exact --nocapture
```

Record, against the original release baseline:

- span-scoring p50/p95;
- NER p50/p95 for one-window and multi-window inputs;
- four-client wall time and per-request p95;
- `extract_from_episode.done` p50/p95;
- peak RSS;
- exact final candidate list.

State the final end-to-end ratio directly. Do not calculate it by multiplying stage ratios.

- [ ] **Step 3: Verify task and legacy client compatibility**

```bash
cargo test --test tools_e2e
cargo test --lib mcp::handlers::tests
```

Expected: synchronous `extract` still returns its original structured result; task-mode `extract` reaches `completed` and returns the same result; forbidden tools reject task augmentation; cancellation and unknown IDs return deterministic errors.

- [ ] **Step 4: Run the full quality gate**

```bash
cargo check
cargo clippy --all-targets
cargo fmt --all --check
cargo test
```

Expected: zero warnings, zero errors, zero format drift, zero failures.

- [ ] **Step 5: Publish the measured operating recommendation**

Update README and both performance documents with the measured configuration. The recommendation must include release mode, provider, device, batch size, max batch tokens, max concurrency, task-capable client behavior, and the observed rather than projected speedup.

If `ner.extract.done / extract_from_episode.done < 0.70`, add this explicit next action:

```markdown
NER is no longer the dominant extract cost. The next optimization must target batched
entity resolution, fact embedding, edge persistence, contradiction queries, and community
updates in a separate persistence-pipeline implementation plan.
```

This branch completes the present plan after the NER changes pass their quality gates and the measurements are published. Persistence optimization is follow-up work, not an unfinished task in this plan. The same exit applies immediately when production uses `NER_PROVIDER=anno` and telemetry confirms that rule-based NER is not the dominant cost.

- [ ] **Step 6: Commit the rollout record**

```bash
git add README.md docs/performance/NER_PERFORMANCE.md docs/EVAL_BASELINE.md
git commit -m "docs: record extract latency rollout results"
```

---

## Acceptance Summary

- The same model and extraction semantics pass the existing default-label and zero-shot coverage tests.
- Span scoring is one gather/FFN/matmul pipeline with one host score transfer per window.
- `NER_BATCH_SIZE` is active and constrained by `NER_MAX_BATCH_TOKENS`.
- Batched CPU forward output matches the unbatched non-padding hidden states within `atol=1e-5`, `rtol=1e-4`, and produces exactly identical ordered candidates.
- Parallel clients cannot exceed `NER_MAX_CONCURRENCY` local model executions; the default of one explicitly trades parallel throughput for bounded memory, predictable tail latency, and protection from oversubscription.
- The server advertises native Tasks; only `extract` is task-optional; synchronous behavior remains intact.
- CPU is always available; Metal is optional, feature-gated, parity-tested, and never presented as CoreML. Explicit `NER_DEVICE=metal` fails hard, while only `NER_DEVICE=auto` may fall back to CPU.
- Performance documents show original and final release measurements, per-stage ratios, peak memory, and one final end-to-end ratio without multiplying estimates.
- If NER falls below 70% of total extraction time, the published measurement and separate persistence follow-up close this plan without broadening its implementation scope.
- The full Rust quality gate passes with zero warnings and failures.
