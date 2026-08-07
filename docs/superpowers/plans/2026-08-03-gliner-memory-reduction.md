# GLiNER Memory Reduction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Reduce steady-state and peak RSS of the `memory_mcp` server (currently observed up to 7.3 GB on macOS) so that idle RSS collapses to the SurrealDB floor (~50–300 MB) and peak RSS during a single-shot GLiNER extract stays bounded (~1.6–2.2 GB), without changing the GLiNER model, entity quality, or the MCP tool surface. **Two metrics matter and are fixed by different levers** (see Evidence): unload alone collapses the physical footprint; the RSS number the user watches collapses only when unload is combined with the allocator fix.

**Architecture:** Three additive changes attack three proven memory terms (see Evidence below): (1) **lazy-load + idle unload** of the GLiNER model via a new `GLINER_IDLE_UNLOAD_SECS` env var (default `0` = off, Ollama-style `keep_alive`), implemented with a generic `LazyModel<T>` state machine (load-on-demand, exactly-once under concurrency; the idle clock is armed at USE COMPLETION so long extracts are never interrupted); (2) **heap-backed safetensors loading** (`from_buffered_safetensors`) — a determinism/cleanliness change since the weights already live in the heap (verified), making `drop()` of the model release the weight bytes to the allocator; (3) an **optional `mimalloc` allocator feature** (default off) that returns freed pages to the OS — the lever that actually collapses the per-process RSS by eliminating the 5.2 GB of retained-but-empty malloc arenas. `NER_MAX_BATCH_TOKENS` is deliberately NOT touched: verified that it only bounds batch packing, not padding, so it has no memory effect at `batch_size=1` (ADR-0031, Rejected).

**Tech Stack:**
- Rust 1.88+ (edition 2024), Tokio 1.53 (`rt-multi-thread`, `sync`, `time`, `macros`)
- Candle 0.11.0 (git pin `21cca0b`), `VarBuilder::from_buffered_safetensors`
- GLiNER multi-v2.1 fixture at `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1` (committed, 1.5 GB total fixture dir)
- Optional: `mimalloc = "0.1"` (pin latest 0.1.x at implementation time) behind feature `mimalloc` (default off)
- macOS 26 (Tahoe), Apple Silicon

---

## Evidence (measured 2026-08-03, live process PID 13188)

Run for 11h18m, parent = `zed`. The user launches `memory_mcp` in the background from zed.

```
RSS:                     7,669,600 KB  (~7.3 GB, 11.4% of physical RAM)
Physical footprint:      1.8 GB        (peak 7.1 GB)
vmmap writable regions:  total 7.7 GB, written 7.3 GB (95%), resident 7.3 GB (94%)

MALLOC_LARGE            1.5 GB resident   → live GLiNER weights (heap-copied, 112 regions)
MALLOC_SMALL            585 MB resident (242 MB dirty)
MALLOC_SMALL (empty)    5.2 GB resident, only 1.2 MB dirty, 1346 regions
                         → freed-but-retained macOS malloc arena pages  ← dominant term
```

Env of the live process: `NER_PROVIDER=local-gliner`, `NER_MODEL=urchade/gliner_multi-v2.1`, `NER_DEVICE=auto`, `NER_MAX_CONCURRENCY=2`, `EMBEDDINGS_PROVIDER=openai-compatible` (remote NVIDIA API — no local embedding model loaded), `SURREALDB_EMBEDDED=true`.

**Root cause chain:**
1. `builder.rs:205-206` calls `create_entity_extractor()` at startup → GLiNER weights (~1.1 GB f32, ~1.5 GB with support structures) are loaded eagerly and live for the **entire process lifetime** (nothing ever drops them). This is the ~1.5 GB floor. Verified heap-backed: `vmmap` shows the weights in `MALLOC_LARGE` (1.5 GB resident), and **no file-backed region for the model exists** — candle already copies mmap'd weights into heap tensors during model build, so the mmap is transient.
2. With `NER_MAX_CONCURRENCY=2` and hours of uptime, allocation churn (extract activations, tensors, tokenizer, SurrealDB, service state) leaves macOS malloc per-thread arenas that are freed but never returned to the OS — `MALLOC_SMALL (empty)` = 5.2 GB resident with only 1.2 MB dirty. **macOS malloc does not return freed pages to the OS**, so RSS ratchets up to the observed 4–7 GB. (Note: activations are NOT padded to `NER_MAX_BATCH_TOKENS` — `run_forward_batch` pads to the longest window in the batch, and windows are capped at `max_len=384` per `gliner_config.json`. So the padding knob is a red herring; see ADR-0031.)
3. `Physical footprint` (what drives macOS memory pressure and Activity Monitor's system-wide "Memory Used") is only **1.8 GB** — the 5.2 GB empty arenas are clean/reclaimable pages that do not count toward footprint but DO count toward the process RSS the user observes. Two different numbers; the plan fixes both by different levers.

**Expected outcome (honest targets, two metrics):**
- **Footprint** (memory pressure): after idle unload → **~0.3 GB** (model freed). Fixed by unload alone, even with default malloc.
- **RSS** (what `ps`/Activity Monitor per-process shows): after unload **without** mimalloc → drops by ~1.5 GB (freed `MALLOC_LARGE` unmaps) but the 5.2 GB empty arenas stay until memory pressure — RSS ≈ 5–6 GB. With **mimalloc** → **~50–300 MB** idle (SurrealDB/RocksDB floor). The 1 GB user target is achievable only in the idle state **and only with the allocator fix**.
- During a single-shot extract: **~1.6–2.2 GB** (1.1 GB weights + activations). **Below 1 GB during an active extract is impossible without a smaller model** — the weights alone are 1.1 GB. State this to the user.
- mimalloc without idle unload: RSS converges to ~1.5–2 GB (live model) and stays flat instead of ratcheting to 7 GB.

---

## Global Constraints

1. **Model unchanged**: `urchade/gliner_multi-v2.1`; no re-download, no re-tokenize, no quality-affecting changes. `tests/models/ner/urchade--gliner_multi-v2.1` fixture is the parity oracle.
2. **Final implemented behavior**: the model loads lazily on first extraction for every configuration. `GLINER_IDLE_UNLOAD_SECS=0` disables idle unloading and retains the model after that first load; a positive value enables idle unloading.
3. **Feature flags additive**: `default = []`; `mimalloc = ["dep:mimalloc"]`; existing `cli-watch`, `mcp-apps`, `prometheus`, `metal`, `eval-support` untouched.
4. **`main.rs` stays thin**: only the `#[global_allocator]` static may be added; no business logic.
5. **Cargo.toml changes require explicit user approval** (AGENTS.md): the `mimalloc` workspace dep + crate feature. No other dependency changes in this plan (tests use real-time sleeps — no `tokio`/`test-util` feature needed).
6. **Quality gate**: the model-backed tests in `crates/memory-mcp/tests/local_model_integration.rs` (candidate signature assertions against the committed fixture) must pass after every task that touches the GLiNER load or extract path. This plan makes NO change to NER runtime defaults (ADR-0031, Rejected).
7. **Lint gate (before shipping)**: `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` and `cargo fmt --all --check` must both be clean.
8. **No `unwrap()` in production code** — `Result`/`?` only.
9. **Coordinate with the latency plan** (`2026-08-03-ner-performance-optimization-plan.md`): do NOT change `NER_DEVICE` defaults here (it may flip to Metal there); both plans may be applied together — test together.
10. **Never delete facts / no new MCP tools** — tool surface is frozen; this plan touches only NER lifecycle + allocator.

---

## Task 1: Baseline Memory Profile (no code)

**Why:** Record the exact current numbers before touching anything, so the final soak task has a before/after. Most of this is already measured above; re-run to confirm drift.

**Files:**
- Create: `docs/superpowers/plans/2026-08-03-gliner-memory-reduction.baseline.txt`

**Interfaces:**
- Produces: the `BASELINE` numbers quoted by Task 2 (ADR-0030) and Task 11 (soak).

- [x] **Step 1: Locate the live process**

```bash
ps -axo pid,rss,etime,command | grep -i memory_mcp | grep -v grep
```
Expected: one line per running instance. Record `pid` and `rss` (KB).

- [x] **Step 2: Capture region-level attribution**

```bash
vmmap -summary <pid> 2>&1 | head -45
```
Record `Physical footprint`, `Physical footprint (peak)`, and the `MALLOC_LARGE` / `MALLOC_SMALL (empty)` rows. Expected: `MALLOC_SMALL (empty)` resident ≈ GBs with dirty ≈ MBs (retained arenas).

- [x] **Step 3: Capture the process env (which providers are active)**

```bash
ps eww -p <pid> | tr ' ' '\n' | grep -E '^(NER_|EMBEDDINGS_|SURREALDB_)' | sort
```
Expected: `NER_PROVIDER=local-gliner`, `EMBEDDINGS_PROVIDER=openai-compatible`.

- [x] **Step 4: Save the numbers**

```bash
{
  echo "date: $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  echo "pid: <pid>  rss_kb: <from Step 1>"
  echo "footprint / peak / malloc-large / malloc-small-empty: <from Step 2>"
  echo "env: <from Step 3>"
} > docs/superpowers/plans/2026-08-03-gliner-memory-reduction.baseline.txt
```

- [x] **Step 5: Commit**

```bash
git add docs/superpowers/plans/2026-08-03-gliner-memory-reduction.baseline.txt
git commit -m "docs: record GLiNER memory baseline (7.3 GB RSS)"
```

---

## Task 2: ADR-0035 — GLiNER Lazy Load + Idle Unload

**Why:** Records the primary design decision before code.

**Files:**
- Create: `docs/adr/0035-gliner-lazy-load-with-idle-unload.md` (renumbered from the original duplicate ADR-0030 on 2026-08-07)

**Interfaces:**
- Consumes: baseline numbers from Task 1.
- Produces: the decision text quoted in Tasks 6–9.

- [x] **Step 1: Write the ADR**

Follow the existing format in `docs/adr/0026-adopt-durable-work-mechanics.md` (Context / Decision / Consequences sections). Content:

```markdown
# ADR-0035: GLiNER Lazy Load with Idle Unload

## Status
Accepted

## Context
The GLiNER model (~1.1 GB f32 weights) is loaded eagerly at service startup
(core/builder.rs) and retained for the process lifetime. Live measurement
showed RSS of 7.3 GB, dominated by (a) ~1.5 GB live heap weights and (b)
~5.2 GB of freed-but-retained macOS malloc arenas accumulated from repeated
extract activity. 99% of usage is single-shot extract followed by long idle.

## Decision
Load the GLiNER model lazily on first extraction for every configuration and
optionally unload it after N seconds of inactivity, controlled by
GLINER_IDLE_UNLOAD_SECS. The default 0 disables idle unloading: the model is
still loaded on first extraction, then retained for the process lifetime.
Implementation: a
generic LazyModel<T> state machine (tokio Mutex + spawn_blocking load +
tokio::time::sleep unload task). Unload is armed AFTER each extract
completes (arm_unload), so the idle clock measures time since last USE
COMPLETION, not load time — long extracts cannot trigger a mid-inference
unload. Unload semantics: guard.loaded = None; in-flight extracts keep their
Arc<T> clone alive until they finish (never dropped mid-use). Model weights
are loaded via from_buffered_safetensors so the buffer is the single owner
of the weight bytes and is freed deterministically.

Note on the allocator: unload alone returns the ~1.5 GB MALLOC_LARGE model
allocation to the OS even with default malloc (large zones unmap on free),
but the ~5.2 GB MALLOC_SMALL (empty) arena term persists without mimalloc
(ADR-0032). Both levers are required to bring the per-process RSS number
down; unload alone fixes the physical footprint.

## Consequences
+ Idle footprint (memory pressure) collapses to the SurrealDB floor.
+ Idle RSS collapses to ~50-300 MB when built with mimalloc (ADR-0032).
+ Peak during an active extract is unchanged (~1.6-2.2 GB) — unavoidable
  without a smaller model (weights are 1.1 GB).
+ First extract after idle pays cold-load latency (~1-2 s, single-shot OK).
+ Concurrency is safe: exactly-once load under the state lock; unload task
  re-checks last_used before dropping; arms only after use completes.
- Lazy loading is always active. Idle unloading is opt-in; the default `0`
  retains a model after its first extraction.
- RSS benefit of unload without mimalloc is partial (arena retention).
```

- [x] **Step 2: Commit**

```bash
git add docs/adr/0035-gliner-lazy-load-with-idle-unload.md
git commit -m "docs: ADR-0035 GLiNER lazy load with idle unload"
```

---

## Task 3: ADR-0031 — NER Runtime Defaults for Memory

**Files:**
- Create: `docs/adr/0031-ner-runtime-defaults-for-memory.md`

- [x] **Step 1: Write the ADR**

```markdown
# 0031: NER Runtime Defaults for Memory

## Status
Rejected

## Context
Windows are capped at 384 tokens (gliner_config.json `max_len: 384`).
`run_forward_batch` pads each batch to the LONGEST window in it, not to
NER_MAX_BATCH_TOKENS; that value only bounds how many windows are packed
into one batch (pack_window_batches, gliner.rs:1239).

## Decision
Do NOT change DEFAULT_NER_MAX_BATCH_TOKENS. At the default batch_size=1
(and the user's config), every batch is a single window, so max_batch_tokens
never binds and lowering it changes nothing about memory. It would only
reduce window packing for batch_size>1 — a throughput regression with no
memory benefit. Activation memory is already sized to the actual window
(<=384 tokens); the 1536 default is a packing ceiling, not a padding target.

## Consequences
+ No change; the padding knob was a red herring (verified in run_forward_batch).
- Future work on activation memory must target the forward-pass itself
  (e.g., Metal/Accelerate per the latency plan), not this constant.
```

- [x] **Step 2: Commit**

```bash
git add docs/adr/0031-ner-runtime-defaults-for-memory.md
git commit -m "docs: ADR-0031 NER runtime defaults for memory"
```

---

## Task 4: ADR-0032 — Optional mimalloc Allocator

**Files:**
- Create: `docs/adr/0032-optional-mimalloc-allocator.md`

- [x] **Step 1: Write the ADR**

```markdown
# 0032: Optional mimalloc Allocator

## Status
Accepted

## Context
vmmap shows 5.2 GB of MALLOC_SMALL (empty) regions — freed pages macOS
malloc retains in per-thread arenas (1346 regions). This is the dominant
RSS term and does NOT shrink with model unload alone: it is not counted in
the physical footprint (1.8 GB), but it is what the per-process RSS number
the user watches stays stuck at after unload without an allocator that
returns freed memory.

## Decision
Add an optional Cargo feature `mimalloc` (default off) that installs
mimalloc as the process global allocator via #[global_allocator] in main.rs.
mimalloc returns freed spans to the OS aggressively, bounding RSS to live
allocations. Feature-gated so the default build is untouched.

## Consequences
+ RSS converges to ~live model (1.5 GB) instead of ratcheting to 7 GB.
+ REQUIRED to realize the idle RSS target after unload (~50-300 MB);
  without it, freed model memory can land in retained arenas and RSS stays
  high until kernel memory pressure reclaims the clean pages.
- Requires user approval for the Cargo.toml change (AGENTS.md).
- Binary-only: the static lives in main.rs, so library tests don't exercise
  it; soak verification (Task 11) validates it on the real binary.
```

- [x] **Step 2: Commit**

```bash
git add docs/adr/0032-optional-mimalloc-allocator.md
git commit -m "docs: ADR-0032 optional mimalloc allocator"
```

---

## Task 5: ADR-0033 — SurrealDB/RocksDB Memory Footprint

**Why:** Rule out (or record) the embedded-database term so the allocator/lazy changes are not blamed for what belongs to the DB.

**Files:**
- Create: `docs/adr/0033-surrealdb-rocksdb-memory-footprint.md`

- [x] **Step 1: Verify whether surrealdb exposes RocksDB cache controls**

Check the surrealdb 3.0.0 public API for RocksDB options (block cache / write buffer sizing):

```bash
cargo doc -p memory_mcp --no-deps -q 2>/dev/null
grep -rn "rocksdb" ~/.cargo/registry/src/*/surrealdb-3.0.0/src/ 2>/dev/null | grep -iE "cache|options" | head -10 || echo "no public RocksDB cache options exposed"
```

- [x] **Step 2: Write the ADR recording the finding**

```markdown
# 0033: SurrealDB/RocksDB Memory Footprint

## Status
Accepted (investigation)

## Context
Embedded SurrealDB (kv-rocksdb/kv-mem) contributes to the process floor.

## Decision
If Step 1 finds no public cache-sizing controls in surrealdb 3.0.0, document
that the RocksDB term is bounded by engine defaults (write buffers, not the
GLiNER-scale 1+ GB terms) and is NOT addressed by this plan. Revisit only if
post-fix profiling shows the DB term dominating idle RSS.
```

- [x] **Step 3: Commit**

```bash
git add docs/adr/0033-surrealdb-rocksdb-memory-footprint.md
git commit -m "docs: ADR-0033 SurrealDB/RocksDB memory footprint"
```

---

## Task 6: `GLINER_IDLE_UNLOAD_SECS` Configuration (TDD)

**Files:**
- Modify: `crates/memory-mcp/src/config/constants.rs`
- Modify: `crates/memory-mcp/src/config/ner.rs:34-53,55-69,78-154`
- Modify: `crates/memory-mcp/tests/local_model_integration.rs:379-389` (struct literal)
- Modify: `crates/eval-harness/benches/ner_cpu.rs:15-28` (struct literal)
- Test: `crates/memory-mcp/src/config/ner.rs` (`mod tests`)

**Why the extra files:** both `local_model_integration.rs:379` and `ner_cpu.rs:15` build `NerConfig { ... }` as full struct literals WITHOUT `..Default::default()` — adding the field breaks their compilation. `cargo test -p memory_mcp` will NOT catch the eval-harness break, so the verification below runs `cargo check --workspace`.

**Interfaces:**
- Consumes: `parse_env::<u64>` (`config/helpers.rs`), `with_ner_env` helper, `super::super::env_lock` (existing patterns).
- Produces:
  - `pub const DEFAULT_GLINER_IDLE_UNLOAD_SECS: u64 = 0;`
  - `NerConfig { ..., pub gliner_idle_unload_secs: u64 }` (Default + `from_env`)
  - Env var `GLINER_IDLE_UNLOAD_SECS` (seconds; `0` = keep loaded forever)

- [x] **Step 1: Write the failing tests**

In `crates/memory-mcp/src/config/ner.rs`, inside `mod tests`:

```rust
#[test]
fn gliner_idle_unload_defaults_to_zero_off() {
    with_ner_env(&[("GLINER_IDLE_UNLOAD_SECS", None)], || {
        let config = NerConfig::from_env().expect("default NER config");
        assert_eq!(config.gliner_idle_unload_secs, 0);
    });
}

#[test]
fn gliner_idle_unload_reads_env_override() {
    with_ner_env(&[("GLINER_IDLE_UNLOAD_SECS", Some("60"))], || {
        let config = NerConfig::from_env().expect("NER config with idle unload");
        assert_eq!(config.gliner_idle_unload_secs, 60);
    });
}

#[test]
fn gliner_idle_unload_rejects_non_numeric() {
    with_ner_env(&[("GLINER_IDLE_UNLOAD_SECS", Some("soon"))], || {
        assert!(matches!(
            NerConfig::from_env(),
            Err(MemoryError::ConfigInvalid(_))
        ));
    });
}
```

- [x] **Step 2: Run to verify they fail**

```bash
cargo test -p memory_mcp ner::tests::gliner_idle_unload -- --nocapture
```
Expected: FAIL — `no field gliner_idle_unload_secs on type NerConfig`.

- [x] **Step 3: Implement**

In `crates/memory-mcp/src/config/constants.rs`, after `DEFAULT_NER_MAX_CONCURRENCY`:

```rust
/// Default idle timeout before unloading the GLiNER model (seconds).
///
/// `0` disables idle unloading — the model stays loaded for the process
/// lifetime (today's behavior). A positive value unloads the model after
/// that many seconds without an extract (Ollama `keep_alive` style).
pub const DEFAULT_GLINER_IDLE_UNLOAD_SECS: u64 = 0;
```

In `crates/memory-mcp/src/config/ner.rs`, add to the struct (after `device`):

```rust
    /// Seconds of inactivity before the GLiNER model is unloaded.
    /// `0` keeps the model loaded for the process lifetime.
    pub gliner_idle_unload_secs: u64,
```

In `Default`:

```rust
            device: NerDeviceKind::Cpu,
            gliner_idle_unload_secs: DEFAULT_GLINER_IDLE_UNLOAD_SECS,
```

In `from_env()`, after the `device` match, add to the returned struct literal:

```rust
            gliner_idle_unload_secs: parse_env::<u64>("GLINER_IDLE_UNLOAD_SECS")?
                .unwrap_or(DEFAULT_GLINER_IDLE_UNLOAD_SECS),
```

Update the two full struct literals (they omit `..Default::default()`):

In `crates/memory-mcp/tests/local_model_integration.rs:388` (after `device: NerDeviceKind::Cpu,`):

```rust
        device: NerDeviceKind::Cpu,
        gliner_idle_unload_secs: 0,
    };
```

In `crates/eval-harness/benches/ner_cpu.rs:27` (after `device: NerDeviceKind::Cpu,`):

```rust
            device: NerDeviceKind::Cpu,
            gliner_idle_unload_secs: 0,
        };
```

- [x] **Step 4: Run to verify they pass**

```bash
cargo test -p memory_mcp ner::tests::gliner_idle_unload
cargo check --workspace --all-targets
```
Expected: PASS (all three tests); `cargo check --workspace --all-targets` compiles every target — including the `eval-harness` bench (`ner_cpu.rs`) and the `local_model_integration` test binary.

- [x] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/config/constants.rs crates/memory-mcp/src/config/ner.rs crates/memory-mcp/tests/local_model_integration.rs crates/eval-harness/benches/ner_cpu.rs
git commit -m "feat: add GLINER_IDLE_UNLOAD_SECS config (default off)"
```

---

## Task 7: Split `LoadedGliner` from `GlinerEntityExtractor` (mechanical refactor)

**Why:** Today the model is owned directly by the struct; to make it optional we need a `GlinerLoader` (immutable recipe, can rebuild) + `LoadedGliner` (fully built model) + a thin outer type. Behavior must be bit-identical at the end of this task.

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs` (struct at `:40-58`, constructors `:580-719`, `build_from_var_builder` `:721-782`, trait impl `:1347-1366`)
- Modify: `crates/memory-mcp/src/service/entity_extraction.rs:126-135` (factory call)
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner/batching.rs:65-158` (test calls inference methods directly — see Step 7)
- Test (gate, unchanged): `crates/memory-mcp/tests/local_model_integration.rs`

**Interfaces:**
- Consumes: `config.gliner_idle_unload_secs` (Task 6); existing `gate::InferenceGate::new(max_concurrency)` / `acquire()`.
- Produces:
  - `struct GlinerLoader { model_dir: PathBuf, labels: Vec<String>, threshold: f64, batch_size: usize, max_batch_tokens: usize, max_concurrency: usize, device_kind: crate::config::NerDeviceKind, logger: crate::logging::StdoutLogger }` with `fn load(&self) -> Result<LoadedGliner, MemoryError>`
  - `struct LoadedGliner { model: DebertaV2Model, tokenizer: Tokenizer, device: Device, labels: Vec<String>, threshold: f64, max_span_width: usize, max_seq_len: usize, ent_token_id: u32, token_projection: TokenProjectionLayer, rnn: BiLstmLayer, span_rep_layer: SpanRepresentationLayer, prompt_rep_layer: FeedForwardProjection, logger: crate::logging::StdoutLogger, batch_size: usize, max_batch_tokens: usize }` (old struct minus `inference_gate`)
  - `struct GlinerEntityExtractor { loader: Arc<GlinerLoader>, loaded: Arc<LoadedGliner>, inference_gate: gate::InferenceGate }` — **temporarily eager**; Task 8 makes it lazy
  - `GlinerEntityExtractor::new_with_runtime(model_dir, labels, threshold, batch_size, max_batch_tokens, max_concurrency, device_kind, idle_unload_secs: u64, logger) -> Result<Self, MemoryError>` — `idle_unload_secs` accepted but unused until Task 8

- [x] **Step 1: Rename the struct and drop the gate field**

Rename `pub struct GlinerEntityExtractor` (`:40`) to `pub struct LoadedGliner` and delete the `inference_gate: gate::InferenceGate,` field (`:57`). Rename the `impl GlinerEntityExtractor {` block(s) that contain inference methods to `impl LoadedGliner {`. The methods whose bodies use only model fields move with it — no body changes: `encode_window`, `collect_prompt_entity_positions`, `extract_word_representations`, `run_forward`, `run_forward_batch`, `decode_window`, `build_label_representations`, `compute_span_scores`, `extract_spans`, `apply_nms`, `extract_inner`, `extract_inner_with_labels`, `build_prompt_words_for_labels`.

**`split_text_words` is special** (`:797-802`): it has no `&self`, only references the module-level `WHITESPACE_WORD_SPLITTER`. Keep it OUT of the `impl LoadedGliner` block — leave it as a plain private `fn` free function in `gliner.rs` at module scope. Update the only internal caller in `extract_inner_with_labels` (`:1213`): replace `let text_words = Self::split_text_words(text);` with `let text_words = split_text_words(text);`. Step 7 updates the test's external caller. **Visibility note**: no `pub(crate)` needed because both callers are descendants of `gliner` (Rust re-entry rule: a descendant module can reference a private ancestor item via the absolute `crate::` path — verified empirically; see the project's own `crate::service::entity_extraction::gliner::build_span_scoring_log_event` for a contrasting sibling-crossing case that DOES need `pub(crate)`). Rationale: the function is a pure utility over a constant; putting it on a struct ties it to a lifetime it doesn't need and forces call-site churn if the wrapper type changes again.

- [x] **Step 2: Move construction into `GlinerLoader`**

Move `build_from_var_builder` into `impl LoadedGliner`, remove its `max_concurrency: usize` parameter, and delete the `inference_gate: gate::InferenceGate::new(max_concurrency),` line from its struct literal (`:780`). Move `resolve_ent_token` alongside it.

Move the body of `load_with_runtime` (`:641-719`) into `impl GlinerLoader { fn load(&self) -> Result<LoadedGliner, MemoryError> }`, replacing every free argument with `self.` fields:
- `model_dir` → `&self.model_dir`
- `labels` → `self.labels.clone()`
- `threshold` → `self.threshold`
- `batch_size` → `self.batch_size`
- `max_batch_tokens` → `self.max_batch_tokens`
- `device_kind` → `self.device_kind`
- `logger` → `self.logger.clone()`
- Drop `max_concurrency` from the `build_from_var_builder` call, which becomes:

```rust
        LoadedGliner::build_from_var_builder(
            tokenizer,
            vb,
            &device,
            runtime_config,
            self.labels.clone(),
            self.threshold,
            self.logger.clone(),
            self.batch_size,
            self.max_batch_tokens,
        )
```

Keep the validation (`batch_size == 0`, `max_batch_tokens == 0`, `max_concurrency == 0`) in the new outer `new_with_runtime`, not in `load`.

- [x] **Step 3: Build the thin outer type**

Replace the old constructors on the outer type:

```rust
impl GlinerEntityExtractor {
    /// Creates a GLiNER extractor. Model weights are loaded eagerly today;
    /// lazy-load lands in the next task. `idle_unload_secs` is reserved.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new_with_runtime(
        model_dir: &Path,
        labels: Vec<String>,
        threshold: f64,
        batch_size: usize,
        max_batch_tokens: usize,
        max_concurrency: usize,
        device_kind: crate::config::NerDeviceKind,
        idle_unload_secs: u64,
        logger: crate::logging::StdoutLogger,
    ) -> Result<Self, MemoryError> {
        if batch_size == 0 || max_batch_tokens == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER batch limits must be greater than zero".to_string(),
            ));
        }
        if max_concurrency == 0 {
            return Err(MemoryError::ConfigInvalid(
                "NER_MAX_CONCURRENCY must be greater than zero".to_string(),
            ));
        }
        let _ = idle_unload_secs; // reserved; consumed by lazy-load task
        let loader = Arc::new(GlinerLoader {
            model_dir: model_dir.to_path_buf(),
            labels,
            threshold,
            batch_size,
            max_batch_tokens,
            max_concurrency,
            device_kind,
            logger: logger.clone(),
        });
        let loaded = Arc::new(loader.load()?);
        Ok(Self {
            loader,
            loaded,
            inference_gate: gate::InferenceGate::new(max_concurrency),
        })
    }
}
```

Update `new_with_logger` (`:589-605`) to pass `crate::config::DEFAULT_GLINER_IDLE_UNLOAD_SECS` as the new argument (position: between `device_kind` and `logger`).

- [x] **Step 4: Fix the trait impl and Debug impl**

`impl EntityExtractor for GlinerEntityExtractor` (`:1347-1366`): keep `provider_name` on the outer type; `acquire_inference_permit` stays on the outer type but must reference the loader's logger — replace `self.logger.log(` with `self.loader.logger.log(` and `self.inference_gate.available_permits()` stays as-is (body otherwise unchanged):

```rust
    async fn acquire_inference_permit(
        &self,
    ) -> Result<tokio::sync::OwnedSemaphorePermit, MemoryError> {
        let (permit, queue_wait) = self
            .inference_gate
            .acquire()
            .await
            .map_err(|_| MemoryError::Storage("GLiNER inference gate closed".to_string()))?;
        self.loader.logger.log(
            crate::service::log_event(
                "ner.gliner.queue.done",
                crate::service::log_args_with_duration(serde_json::json!({}), queue_wait),
                serde_json::json!({
                    "available_permits": self.inference_gate.available_permits()
                }),
                None,
                None,
                None,
            ),
            crate::logging::LogLevel::Debug,
        );
        Ok(permit)
    }
```

`extract_candidates` / `extract_candidates_with_labels` become:

```rust
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        self.loaded.extract_inner(content)
    }

    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        self.loaded.extract_inner_with_labels(content, zero_shot_labels)
    }
```

Debug impl (`:1338-1345`): reference loader fields:

```rust
impl std::fmt::Debug for GlinerEntityExtractor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlinerEntityExtractor")
            .field("labels", &self.loader.labels)
            .field("threshold", &self.loader.threshold)
            .finish()
    }
}
```

- [x] **Step 5: Update the factory call site**

In `crates/memory-mcp/src/service/entity_extraction.rs:126-135`, add the new argument between `config.device` and `logger.clone()`:

```rust
                config.device,
                config.gliner_idle_unload_secs,
                logger.clone(),
```

- [x] **Step 6: Verify — behavior must be identical**

```bash
cargo test -p memory_mcp ner::tests::gliner_idle_unload
cargo test -p memory_mcp --test local_model_integration local_gliner
cargo check --workspace --all-targets
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
```
Expected: PASS; the model-backed extraction tests still find the same entities (candidate assertions unchanged). `cargo check --workspace --all-targets` catches the eval-harness bench. Clippy zero warnings.

- [x] **Step 7: Fix the batching.rs diagnostic test**

`crates/memory-mcp/src/service/entity_extraction/gliner/batching.rs:65-158` (`batched_forward_matches_unbatched_hidden_states_with_padding`, `#[ignore]`d) constructs `GlinerEntityExtractor::new(...)` and then calls `build_prompt_words_for_labels`, `encode_window`, `run_forward`, `run_forward_batch` on the result. After the split those methods live on `LoadedGliner` (batching.rs is a child module of gliner.rs, so it can reach the private `GlinerLoader` via an absolute `crate::` path — Rust re-entry rule lets a descendant module access a private ancestor item). Replace the setup (`:83-88`) — use the absolute path to match the test's existing style:

```rust
        let loader = crate::service::entity_extraction::gliner::GlinerLoader {
            model_dir: model_dir.to_path_buf(),
            labels: labels.clone(),
            threshold: crate::config::DEFAULT_NER_THRESHOLD,
            batch_size: 1,
            max_batch_tokens: crate::config::DEFAULT_NER_MAX_BATCH_TOKENS,
            max_concurrency: 1,
            device_kind: crate::config::NerDeviceKind::Cpu,
            logger: crate::logging::StdoutLogger::new("error"),
        };
        let extractor = loader.load().expect("load local GLiNER model");
```

In the closure body, replace the static call at `:92-95`:

```rust
            let text_words =
                crate::service::entity_extraction::gliner::split_text_words(text);
```

(it resolves to the plain `fn` free function from Step 1 — no visibility annotation needed; the rest of the test calls instance methods on `extractor`, which is a `LoadedGliner`, so `build_prompt_words_for_labels`, `encode_window`, `run_forward`, `run_forward_batch` keep working unchanged).

- [x] **Step 8: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction/gliner.rs crates/memory-mcp/src/service/entity_extraction.rs crates/memory-mcp/src/service/entity_extraction/gliner/batching.rs
git commit -m "refactor: split LoadedGliner/GlinerLoader from extractor (no behavior change)"
```

---

## Task 8: Generic `LazyModel<T>` + Lazy Wiring (TDD)

**Why:** The core new logic — load-on-demand, exactly-once under concurrency, idle unload with race safety. Built generic over `T` so it is unit-testable without the 1.1 GB model.

**Files:**
- Create: `crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs`
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs` (add `mod lazy;`, swap eager field for `LazyModel`, add `ensure_loaded`)
- Test: `crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs` (`mod tests`)

**Interfaces:**
- Consumes: `crate::service::MemoryError`; `GlinerLoader::load()` + `LoadedGliner` (Task 7); `gate::InferenceGate` (Task 7).
- Produces:
  - `pub(super) struct LazyModel<T> { state: Arc<tokio::sync::Mutex<LazyModelState<T>>>, idle_unload: Option<std::time::Duration> }`
  - `pub(super) struct LazyModelState<T> { loaded: Option<Arc<T>>, last_used: std::time::Instant, unload_handle: Option<tokio::task::JoinHandle<()>> }`
  - `impl<T: Send + Sync + 'static> LazyModel<T>`: `new(idle_unload)`, `get_or_load<F>(load) -> Result<Arc<T>, MemoryError>` (F: `FnOnce() -> Result<Arc<T>, MemoryError> + Send + 'static`), `arm_unload(&self)`, `spawn_unload_task(state, timeout)`
  - **Unload semantics**: `get_or_load` never schedules an unload; the caller calls `arm_unload()` AFTER each use. The idle clock measures time since the last use COMPLETED — a long extract can never be interrupted by an unload (and the Arc clone keeps the model alive even if an unload fired).

- [x] **Step 1: Write the failing tests**

Create `crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs` with the module scaffolding and the full test module, but **without** `get_or_load` / `arm_unload` / `spawn_unload_task` (they are added in Step 3):

```rust
//! Lazy load-on-demand with idle-unload for heavyweight model instances.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::Mutex;

use crate::service::MemoryError;

/// State of a lazily loaded model instance.
pub(super) struct LazyModelState<T> {
    loaded: Option<Arc<T>>,
    last_used: Instant,
    unload_handle: Option<tokio::task::JoinHandle<()>>,
}

/// A model that is constructed on first use and dropped after `idle_unload`
/// of inactivity. `None` disables unloading (model stays loaded forever).
pub(super) struct LazyModel<T> {
    state: Arc<Mutex<LazyModelState<T>>>,
    idle_unload: Option<Duration>,
}

impl<T: Send + Sync + 'static> LazyModel<T> {
    pub(super) fn new(idle_unload: Option<Duration>) -> Self {
        Self {
            state: Arc::new(Mutex::new(LazyModelState {
                loaded: None,
                last_used: Instant::now(),
                unload_handle: None,
            })),
            idle_unload,
        }
    }

    // get_or_load + arm_unload + spawn_unload_task are implemented in Step 3.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn make_counter() -> Arc<AtomicUsize> {
        Arc::new(AtomicUsize::new(0))
    }

    fn fake_load(calls: &Arc<AtomicUsize>) -> impl FnOnce() -> Result<Arc<String>, MemoryError> + Send + 'static {
        let calls = Arc::clone(calls);
        move || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new("model".to_string()))
        }
    }

    #[tokio::test]
    async fn constructs_on_first_call() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_secs(60)));
        let value = model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(*value, "model");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn caches_within_idle_timeout() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_secs(60)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(30)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn unloads_after_idle_timeout() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(60)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The idle clock starts at the end of use.
        model.arm_unload().await;
        await_unloaded(&model).await;
        // A subsequent call must rebuild.
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn arm_after_use_resets_the_idle_timer() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(500)));
        // t=0: load + first use completes -> arm (task A fires at t=500).
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await;
        // t=100ms: a new use starts and completes -> get + arm (cancels A,
        // task B fires at t=600).
        tokio::time::sleep(Duration::from_millis(100)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await;
        // t=200ms: inside the fresh 500ms window since the last arm.
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!({ model.state.lock().await.loaded.is_some() }, true);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        await_unloaded(&model).await;
    }

    #[tokio::test]
    async fn no_unload_before_first_arm() {
        // The idle clock only starts after the first completed use; a freshly
        // loaded model with no arm yet must stay loaded past the timeout.
        let calls = make_counter();
        let model = LazyModel::<String>::new(Some(Duration::from_millis(40)));
        model.get_or_load(fake_load(&calls)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(120)).await;
        assert_eq!({ model.state.lock().await.loaded.is_some() }, true);
        // First arm schedules the unload.
        model.arm_unload().await;
        await_unloaded(&model).await;
    }

    #[tokio::test]
    async fn concurrent_loads_construct_exactly_once() {
        let calls = make_counter();
        let model = Arc::new(LazyModel::<String>::new(None));
        let mut handles = Vec::new();
        for _ in 0..8 {
            let m = Arc::clone(&model);
            let c = Arc::clone(&calls);
            handles.push(tokio::spawn(async move {
                m.get_or_load(fake_load(&c)).await.unwrap();
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn disabled_unload_keeps_model_loaded() {
        let calls = make_counter();
        let model = LazyModel::<String>::new(None);
        model.get_or_load(fake_load(&calls)).await.unwrap();
        model.arm_unload().await; // idle_unload=None -> no task scheduled
        tokio::time::sleep(Duration::from_millis(30)).await;
        model.get_or_load(fake_load(&calls)).await.unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    async fn await_unloaded(model: &LazyModel<String>) {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let unloaded = { model.state.lock().await.loaded.is_none() };
            if unloaded {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "model was not unloaded within the deadline"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}
```

Declare the module in `crates/memory-mcp/src/service/entity_extraction/gliner.rs`, at the module header (next to `mod batching; mod gate; mod scoring;`):

```rust
mod lazy;
```

- [x] **Step 2: Run to verify they fail**

```bash
cargo test -p memory_mcp lazy::tests
```
Expected: FAIL to compile with `no method named 'get_or_load' found` — the tests reference an API that does not exist yet.

- [x] **Step 3: Implement `get_or_load` + `arm_unload` + `spawn_unload_task`**

Fill the impl block in `lazy.rs`:

```rust
    /// Returns the cached model, or constructs it exactly once under the
    /// state lock. The `load` closure runs on the blocking pool.
    /// Does NOT schedule an unload — call `arm_unload` after use.
    pub(super) async fn get_or_load<F>(&self, load: F) -> Result<Arc<T>, MemoryError>
    where
        F: FnOnce() -> Result<Arc<T>, MemoryError> + Send + 'static,
    {
        let mut guard = self.state.lock().await;
        if let Some(loaded) = guard.loaded.as_ref() {
            guard.last_used = Instant::now();
            if let Some(handle) = guard.unload_handle.take() {
                handle.abort();
            }
            return Ok(Arc::clone(loaded));
        }
        let loaded = tokio::task::spawn_blocking(load)
            .await
            .map_err(|err| {
                MemoryError::Storage(format!("model load task panicked: {err}"))
            })??;
        guard.last_used = Instant::now();
        guard.loaded = Some(Arc::clone(&loaded));
        Ok(loaded)
    }

    /// Records that the model was used and (re)arms the idle-unload timer.
    /// The idle clock starts at USE COMPLETION, so an unload can never fire
    /// while an extract is still running.
    pub(super) async fn arm_unload(&self) {
        let mut guard = self.state.lock().await;
        guard.last_used = Instant::now();
        if let Some(handle) = guard.unload_handle.take() {
            handle.abort();
        }
        guard.unload_handle = self.idle_unload.map(|timeout| {
            Self::spawn_unload_task(Arc::clone(&self.state), timeout)
        });
    }

    fn spawn_unload_task(
        state: Arc<Mutex<LazyModelState<T>>>,
        timeout: Duration,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            let mut guard = state.lock().await;
            if guard.last_used.elapsed() >= timeout {
                guard.loaded = None;
                guard.unload_handle = None;
            }
        })
    }
}
```

- [x] **Step 4: Run the new unit tests**

```bash
cargo test -p memory_mcp lazy::tests
```
Expected: PASS (all 7 tests: construct, cache, unload-after-timeout, arm-resets-timer, no-unload-before-first-arm, concurrent-once, disabled).

- [x] **Step 5: Wire the extractor**

In `crates/memory-mcp/src/service/entity_extraction/gliner.rs` add `use std::time::Duration;` (the `mod lazy;` declaration was added in Step 1; `LazyModel` is referenced by its module path `lazy::LazyModel`).

In the outer struct, replace the eager field:

```rust
pub struct GlinerEntityExtractor {
    loader: Arc<GlinerLoader>,
    model: lazy::LazyModel<LoadedGliner>,
    inference_gate: gate::InferenceGate,
}
```

In `new_with_runtime`, replace the eager load with lazy construction:

```rust
        let idle_unload = (idle_unload_secs > 0).then(|| Duration::from_secs(idle_unload_secs));
        Ok(Self {
            loader,
            model: lazy::LazyModel::new(idle_unload),
            inference_gate: gate::InferenceGate::new(max_concurrency),
        })
```

(remove `let loaded = Arc::new(loader.load()?);`)

Add `ensure_loaded` to the outer impl:

```rust
    async fn ensure_loaded(&self) -> Result<Arc<LoadedGliner>, MemoryError> {
        let loader = Arc::clone(&self.loader);
        self.model
            .get_or_load(move || Ok(Arc::new(loader.load()?)))
            .await
    }
```

Rewrite the trait methods to route through `ensure_loaded`:

```rust
    async fn extract_candidates(&self, content: &str) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        let loaded = self.ensure_loaded().await?;
        let result = loaded.extract_inner(content);
        // Arm the idle-unload timer at USE COMPLETION (also fires when
        // extract_inner returned Err — the model was still "used").
        self.model.arm_unload().await;
        result
    }

    async fn extract_candidates_with_labels(
        &self,
        content: &str,
        zero_shot_labels: &[String],
    ) -> Result<Vec<EntityCandidate>, MemoryError> {
        let _permit = self.acquire_inference_permit().await?;
        let loaded = self.ensure_loaded().await?;
        let result = loaded.extract_inner_with_labels(content, zero_shot_labels);
        self.model.arm_unload().await;
        result
    }
```

- [x] **Step 6: Run the model-backed regression gate**

```bash
cargo test -p memory_mcp --test local_model_integration local_gliner
```
Expected: PASS — extractor now loads lazily on first extract but produces identical candidates. With default `GLINER_IDLE_UNLOAD_SECS=0` there is no unload, so repeated extracts hit the cache.

**No code changes are required at the three `GlinerEntityExtractor::new(...)` call sites in `#[ignore]`d tests** (`crates/memory-mcp/tests/local_model_integration.rs:334, :357, :527`). They continue to compile and run because:
1. The 3-arg public `new` still exists on the outer type and still returns an eager-load extractor (Task 7 Step 3 builds a `LoadedGliner` eagerly inside `new_with_runtime`).
2. After this task, `new_with_runtime` swaps the eager field for `lazy::LazyModel::new(None)` when `idle_unload_secs == 0`, so the test's first `extract_candidates` triggers `ensure_loaded()` and reloads transparently — the model is still resident for the test's lifetime because `arm_unload` with `idle_unload=None` is a no-op. Candidate assertions are unchanged.

- [x] **Step 7: Lint gate**

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
```
Expected: clean.

- [x] **Step 8: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction/gliner/lazy.rs crates/memory-mcp/src/service/entity_extraction/gliner.rs
git commit -m "feat: lazy-load GLiNER model with idle unload (LazyModel)"
```

---

## Task 9: Heap-Backed Safetensors Loading

**Why (verified):** `vmmap` on the live process shows the weights already live in the heap (`MALLOC_LARGE` 1.5 GB) with NO file-backed model region — candle copies mmap'd data into heap tensors during model build, so `from_mmaped_safetensors` is not holding file pages long-term. The memory win comes from the OTHER tasks (unload + allocator). This task is a **cleanliness + determinism** change: `from_buffered_safetensors` makes the single-owner `Vec<u8>` buffer the source of truth, removes the `unsafe` mmap call and the transient file-mapping lifetime, and guarantees that dropping `LoadedGliner` frees the weight bytes to the allocator (which mimalloc then returns to the OS). It is done here because Task 8 already re-plumbs this load path. **Cost to verify:** the read buffer adds a transient ~1.1 GB during cold load; confirm the load peak stays acceptable in Step 2.

**Files:**
- Modify: `crates/memory-mcp/src/service/entity_extraction/gliner.rs` (safetensors branch in `GlinerLoader::load`, currently `:693-695`)
- Test (gate, unchanged): `crates/memory-mcp/tests/local_model_integration.rs`

**Interfaces:**
- Consumes: `VarBuilder::from_buffered_safetensors(Vec<u8>, DType, &Device)` (candle).
- Produces: same `LoadedGliner`, now heap-backed.

- [x] **Step 1: Swap the loading strategy**

Replace the safetensors branch of `GlinerLoader::load`:

```rust
        let vb = if safetensors_path.is_file() {
            let buffer = std::fs::read(&safetensors_path).map_err(|err| {
                MemoryError::Storage(format!("failed to read safetensors: {err}"))
            })?;
            VarBuilder::from_buffered_safetensors(buffer, DTYPE, &device).map_err(|err| {
                MemoryError::Storage(format!("failed to load safetensors: {err}"))
            })?
        } else if pytorch_path.is_file() {
            VarBuilder::from_pth(pytorch_path.to_str().unwrap_or(""), DTYPE, &device).map_err(
                |err| MemoryError::Storage(format!("failed to load pytorch weights: {err}")),
            )?
        } else {
            return Err(MemoryError::Storage(
                "no model weights found (expected model.safetensors or pytorch_model.bin)"
                    .to_string(),
            ));
        };
```

- [x] **Step 2: Verify the model still loads and extracts identically**

```bash
cargo test -p memory_mcp --test local_model_integration local_gliner
```
Expected: PASS — same candidate assertions against the fixture.

- [x] **Step 3: Measure the cold-load peak**

Confirm the transient read buffer does not blow up the load peak beyond the expected band — run a single extract with idle-unload off (so the post-load RSS equals the loaded-model RSS) and sample at 0.5 s granularity:

```bash
cargo build -p memory_mcp --release
NER_PROVIDER=local-gliner NER_MODEL=urchade/gliner_multi-v2.1 \
NER_MODEL_DIR=crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1 \
GLINER_IDLE_UNLOAD_SECS=0 \
SURREALDB_DATA_DIR=/tmp/mm_peak_test ./target/release/memory_mcp serve &
pid=$!
# trigger one extract from zed, then sample for 120 s:
end=$(( $(date +%s) + 120 ))
: > /tmp/mm_peak.log
while (( $(date +%s) < end )); do
  ps -o rss= -p "$pid" | tr -d ' ' >> /tmp/mm_peak.log
  sleep 0.5
done
kill "$pid" 2>/dev/null
awk '{ if ($1 > m) m = $1 } END { print "PEAK_RSS_KB=" m }' /tmp/mm_peak.log
```
Expected: peak ≈ 2.2–2.8 GB. The cold-load transient is bounded by `(read buffer ≈ 1.1 GB) + (candle tensor copies ≈ 1.1–1.5 GB f32) + (support structures ≈ 0.1 GB) ≈ 2.3–2.7 GB`; this happens once, during the first extract's load phase. Steady-state after the load completes is ~1.8 GB (the same as today — the weight bytes already lived in `MALLOC_LARGE` before this task, see Evidence §1). **Standalone peak only**: measure after a fresh server start with no other heavy activity, so the number reflects this load path, not unrelated churn. If it exceeds **~3.0 GB**, the read buffer is being kept alive by candle after model build — fall back to mmap for the load path and rely on unload + mimalloc only (record in ADR-0030 as a consequence, not a regression of today's numbers).

- [x] **Step 4: Lint gate**

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
```
Expected: clean.

- [x] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/service/entity_extraction/gliner.rs
git commit -m "perf: load GLiNER weights heap-backed via buffered safetensors"
```

---

## Task 10: Optional `mimalloc` Allocator (requires user approval)

**Why:** The 5.2 GB of retained-empty malloc arenas is the dominant RSS term; mimalloc returns freed spans to the OS, bounding RSS to live allocations. Feature-gated, default off.

**Files:**
- Modify: `Cargo.toml` (workspace, `[workspace.dependencies]`)
- Modify: `crates/memory-mcp/Cargo.toml` (`[dependencies]` + `[features]`)
- Modify: `crates/memory-mcp/src/main.rs`

**Interfaces:**
- Consumes: `mimalloc = "0.1.43"` (workspace dep), `#[global_allocator]`.
- Produces: cargo feature `mimalloc` (`default = []` stays); global allocator active only in the binary built with `--features mimalloc`.

> **Approval gate:** AGENTS.md requires asking before changing `Cargo.toml`. The user pre-approved *planning* the allocator option; confirm explicitly before executing this task.

- [x] **Step 1: Add the workspace dependency**

In `Cargo.toml` `[workspace.dependencies]` (alphabetical, after `metrics-exporter-prometheus`):

```toml
mimalloc = "0.1"
```

> Pin to the latest 0.1.x at implementation time (`cargo add mimalloc@0.1 --dry-run` or the crates.io page). The 0.1 series is the stable line.

- [x] **Step 2: Add the optional dependency + feature to the crate**

In `crates/memory-mcp/Cargo.toml` `[dependencies]` (after `metrics-exporter-prometheus`):

```toml
mimalloc = { workspace = true, optional = true }
```

In `[features]` (additive, `default = []` unchanged):

```toml
mimalloc = ["dep:mimalloc"]
```

- [x] **Step 3: Install the global allocator**

In `crates/memory-mcp/src/main.rs`:

```rust
#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match memory_mcp::runner::run().await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(code) => code,
    }
}
```

- [x] **Step 4: Verify both build configurations**

```bash
cargo build -p memory_mcp                       # default: no mimalloc
cargo build -p memory_mcp --features mimalloc   # allocator active
cargo test -p memory_mcp --features mimalloc --test local_model_integration local_gliner
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps,mimalloc --locked -- -D warnings
```
Expected: both builds succeed; tests pass; clippy clean.

- [x] **Step 5: Commit**

```bash
git add Cargo.toml crates/memory-mcp/Cargo.toml crates/memory-mcp/src/main.rs
git commit -m "feat: optional mimalloc global allocator (feature-gated)"
```

---

## Task 11: Soak Verification (before/after)

**Why:** Prove the fix on the real binary with the user's actual workload pattern (background server + single-shot extracts from zed).

**Files:**
- Create: `scripts/memory_profile.sh`
- Uses: `docs/superpowers/plans/2026-08-03-gliner-memory-reduction.baseline.txt` (Task 1)

**Interfaces:**
- Consumes: running `memory_mcp` PID; env `GLINER_IDLE_UNLOAD_SECS`, feature `mimalloc`.
- Produces: `/tmp/memory_mcp_rss.log` with per-2s RSS samples + `PEAK_RSS_KB` line.

- [x] **Step 1: Create the sampler script**

`scripts/memory_profile.sh`:

```bash
#!/usr/bin/env bash
# Samples RSS (and footprint when available) of a running memory_mcp process.
# Usage: scripts/memory_profile.sh <pid> <duration_secs> [log_file]
set -euo pipefail

pid="${1:?usage: memory_profile.sh <pid> <duration_secs> [log_file]}"
duration="${2:?}"
log="${3:-/tmp/memory_mcp_rss.log}"

peak=0
start=$(date +%s)
: > "$log"
while (( $(date +%s) - start < duration )); do
  rss_kb=$(ps -o rss= -p "$pid" | tr -d ' ')
  fp=$(footprint "$pid" 2>/dev/null | awk '/Physical footprint/ {print $3}' | head -1)
  if (( rss_kb > peak )); then peak=$rss_kb; fi
  echo "$(date +%H:%M:%S) rss_kb=$rss_kb footprint=$fp" >> "$log"
  sleep 2
done
echo "PEAK_RSS_KB=$peak" | tee -a "$log"
```

`chmod +x scripts/memory_profile.sh`

- [x] **Step 2: Baseline re-run on the OLD binary (already measured: RSS 7.3 GB / footprint 1.8 GB)**

Restart the background server, then:

```bash
scripts/memory_profile.sh <pid> 120 /tmp/memory_mcp_rss_before.log
vmmap -summary <pid> 2>&1 | head -45   # record MALLOC_LARGE + MALLOC_SMALL (empty) rows
```
While it samples, trigger 3–5 extracts from zed (the user's normal flow). Expected: `rss_kb` ratchets up and stays high (matches Task 1).

- [x] **Step 3: NEW binary, idle unload only (default allocator)**

Build `--release` (no mimalloc), run with `GLINER_IDLE_UNLOAD_SECS=30`:

```bash
GLINER_IDLE_UNLOAD_SECS=30 ./target/release/memory_mcp serve   # or the user's launch env
```
Verify the server logs the lazy load on the first extract and an unload log line ~30 s after the last extract. Sample while triggering extracts:

```bash
scripts/memory_profile.sh <pid> 180 /tmp/memory_mcp_rss_unload_only.log
vmmap -summary <pid> 2>&1 | head -45
```
**Honest expectations (unload alone, default malloc):**
- **Footprint drops to ~0.3 GB** after unload (the model's `MALLOC_LARGE` unmaps on free) — this is the memory-pressure win.
- **RSS drops by ~1.5 GB but the empty arenas stay** (macOS malloc does not return them; they are clean and only reclaimed under kernel pressure) → RSS settles around **5–6 GB**, NOT at the floor. Do not mark this a failure — it is the documented behavior driving the mimalloc step.
- Peak during extract ≈ 1.6–2.2 GB; no unbounded ratchet beyond the retained arenas.

- [x] **Step 4: NEW binary, idle unload + mimalloc**

Build `--release --features mimalloc`, same env. Repeat Step 3. Expected:
- After the last extract, **RSS returns toward the floor (~50–300 MB) within ~30–35 s** (unload frees the model AND mimalloc returns the arenas to the OS).
- `PEAK_RSS_KB` across repeated extracts stays bounded; no ratchet.
- This is the config that meets the user's ~1 GB idle target.

- [x] **Step 5: mimalloc without idle unload**

Build `--release --features mimalloc`, `GLINER_IDLE_UNLOAD_SECS` unset. Expected: after an extract burst, RSS converges to ~1.5–2 GB (live model) instead of climbing to 4–7 GB. Documents the allocator-only configuration.

- [x] **Step 6: Record results + lint**

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo fmt --all --check
git add scripts/memory_profile.sh
git commit -m "test: add RSS/footprint sampler for memory soak verification"
```

---

## Task 12: Documentation

**Files:**
- Modify: `README.md` (config/env tables)
- Modify: `docs/agent/EVALUATION.md` only if it references NER defaults (verify; likely no change)

- [x] **Step 1: Document the new env var**

In `README.md`, add to the NER configuration section:

```markdown
| Variable | Description |
|----------|-------------|
| `GLINER_IDLE_UNLOAD_SECS` | Seconds of inactivity before the local GLiNER model is unloaded to free memory. `0` (default) keeps the model loaded for the process lifetime. Set e.g. `30` for Ollama `keep_alive`-style unloading. |
```

- [x] **Step 2: Document the feature flag**

In `README.md` build/features section:

```markdown
- `mimalloc` — use the mimalloc global allocator. Returns freed memory to
  the OS more aggressively than the macOS default malloc, which keeps
  released arenas resident (observed to inflate RSS of long-running servers).
  Build: `cargo build --release --features mimalloc`.
```

- [x] **Step 3: Note that NER defaults are unchanged**

`NER_MAX_BATCH_TOKENS` stays at its current default (per ADR-0031 — it does not drive activation memory at `batch_size=1`). If `README.md` currently documents a padding/waste rationale for it, correct that note instead of changing the default.

- [x] **Step 4: Commit**

```bash
git add README.md
git commit -m "docs: document GLINER_IDLE_UNLOAD_SECS and mimalloc feature"
```

---

## Final Verification (after Task 12)

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings
cargo test -p memory_mcp
cargo test -p memory_mcp --features mimalloc --test local_model_integration local_gliner
```

Then summarize for the user: idle RSS target (~50–300 MB with `GLINER_IDLE_UNLOAD_SECS` set AND the `mimalloc` feature — see the honest two-metric expectations in the Evidence section), active-extract peak (~1.6–2.2 GB, cannot go below ~1.1 GB model size), and the optional mimalloc build for bounded RSS without unload.
