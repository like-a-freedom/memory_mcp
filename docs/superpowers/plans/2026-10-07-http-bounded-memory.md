# HTTP Bounded Memory — Variant A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Уменьшить startup/active/post-work память существующего HTTP SaaS, закрыв unbounded admission/retention и ненужные аллокации, с проверенным Linux evidence вместо предположений об allocator.

**Architecture:** Независимые byte/count budgets на ранний HTTP preflight, caches и detached embedding work. Context-local use cases получают narrow ports и shared deployment dependencies; storage возвращает необходимые projections/scalars/pages. Сначала allocation baseline, затем независимо reviewable TDD slices; полный продукт и протокольные контракты сохраняются.

**Tech Stack:** Rust 1.99.0, Tokio, Axum, RMCP, SurrealDB SDK 3.2.4, remote SurrealDB 3.3.0, anno 0.13.0, regex, metrics-exporter-prometheus 0.18.3, Linux procfs/cgroup v2.

**Spec:** `docs/superpowers/specs/2026-10-07-http-bounded-memory-design.md` — проект для совместного review. Перед исполнением пользователь утверждает этот design и plan. Нынешнее согласие на вариант A не является разрешением на deploy или подтверждением каждого нового limit.

## Global Constraints

- Rust channel `1.99.0`; Docker/CI/toolchain pin contract неизменен.
- No dependencies, migrations, new MCP tools или generated-code changes без отдельного approval.
- No allocator change, malloc_trim timer, model removal, remote-only profile или worker process в этом плане.
- Сохранить полный streamable-http, dual-era MCP, bi-temporal validity, namespace isolation, canonical vector policies и audit trail.
- Business rules в owning context `api.rs`; transport/composition thin, не новый shared service container.
- No production `unwrap()`, no fact deletion, no request-level namespace selection.
- Не изменять исходные пользовательские незакоммиченные HTTP config/observability правки; integration через narrow edits с review diff.
- No credentials/content в logs, fixtures или evidence; использовать synthetic data и dedicated staging namespace.
- Production SSH сейчас только read-only; restart, config update, debugger injection, binary replacement и deployment требуют нового разрешения.
- Constrained profile: body=1048576 B; preflight=2 slots/2097152 B; ordinary operations=8 (существующий VPS setting, не memory guarantee); pool=2; subscriptions=2; maintenance=1; TTL=120 s; anno input=65536 B; context cache=4194304 B; query cache=2097152 B; background admitted=8/running=1/retained=262144 B/deadline=60 s.
- Candidate targets: active VmRSS+VmSwap <=200 MiB, post-work warm idle <=100 MiB, >=4x reduction versus controlled identical-workload baseline when baseline exceeds target; 10–20 MiB idle stretch only.
- Кэши имеют accounted retained estimate, не exact allocator accounting; upload budget покрывает reserved raw body, не JSON expansion.
- Каждая задача: regression RED → minimal change → GREEN → diff review → isolated commit только при утверждённом execution; команды commit ниже не исполнялись на этапе планирования.

## Review Focus

1. Saturated slots, declared/unknown/understated-length and cancelled upload: slot refusals do not poll bodies, byte refusals do not copy the denied frame, all reservations release; existing cheap header validation remains ahead of overload refusal — Task 3.
2. Invalidation во время retrieval: результат старой generation не repopulate cache; byte accounting очищается на всех путях — Task 4.
3. Oversized persisted Unicode episode: no truncation/partial extraction; refusal предшествует projection writes, labels path не обходит guard — Task 6.
4. Remote provider outage, task cancellation и runtime eviction: bounded pending work, original query identity, capacity release, no hidden old service generations — Tasks 12 и 14.
5. NONE/NULL/empty embeddings, orphan artifacts и concurrent insertions: startup решения сохраняются; pagination продвигается без потери audit и tenant leakage — Tasks 8 и 10.

---

## Status, scope split и dependencies

План — umbrella для четырёх независимых subprojects: **evidence**, **admission/caches**, **extraction/maintenance/storage**, **background/lifetime/metrics**. Не объединять их в один огромный PR. Каждый task — отдельная reviewable поставка; после baseline выполнять только готовые узлы. Root-cause correction по невыявленному owner не угадывать.

```text
1 sampler → 2 controlled baseline / allocation attribution
2 → 3 early buffering
2 → 4 context cache → 5 query cache
2 → 6 anno input → 7 shared triples → 11 narrow HTTP maintenance
2 → 8 startup metadata
2 → 9 nonsemantic projections
2 → 10 task artifact pages
4 + 5 → 12 background admission/lifetime
2 → 13 metrics upkeep
12 → 14 subscription ownership (implementation only if regression confirms)
3..14 → 15 acceptance / controlled release proposal
```

Config/builder/shared composition files пересекаются: не разрешать одновременно писать туда нескольким agents. Parallel implementation возможна только с disjoint write sets; иначе sequential integration.

### Live baseline observation, not acceptance evidence

Read-only SSH 2026-10-07: image sha256 `0fdc287b8a8843007535b88197517d3ab0256beaa7fb8895fed3dbb0ebcc7247`, OCI revision absent; PID 3443295 started 11:04:33 UTC. At 11:38:12 VmRSS=371460 KiB, VmSwap=346908 KiB; at 11:47:20 793588/639252 KiB; at 11:47:53 751960/727256 KiB, HWM=1215228 KiB. Body/pool/request settings already reduced. Same PID grew; traffic was not isolated. Session MCP recall may or may not address this deployment. Do not call this cold idle, causal A/B or proven leak.

## File structure and responsibilities

Paths below relative to repository root, without `memory_mcp/` prefix. Existing locations inspected; new paths explicitly proposed.

| Slice | Files | Responsibility |
|---|---|---|
| Evidence | Create `crates/xtask/src/memory.rs`; modify `crates/xtask/src/main.rs`; create `docs/performance/HTTP_MEMORY_BASELINE.md` | External Linux snapshot sampler and reproducible protocol; no product instrumentation |
| Admission | Create `crates/memory-mcp/src/http/middleware/preflight_budget.rs`; modify `http/middleware.rs`, `http/middleware/preflight.rs`, `http.rs`, `http/config/{parse,types,validate}.rs` | Early nonblocking slot/byte reservation |
| Context cache | Create `src/platform/context_cache.rs`; modify `platform/context_cache_key.rs`, `platform.rs`, `memory/context_cache.rs`, `memory/context_cache/invalidation.rs`, `memory/retrieval.rs`, `memory/retrieval/pipeline.rs`, `service/core/builder.rs` | Private weighted LRU and invalidation generation |
| Cache config | Create `src/config/memory.rs`; modify `config.rs`, HTTP deployment/runtime wiring, bootstrap stdio wiring | Parse limits once; retain constructor compatibility |
| Query cache | Create `src/embedding/query_cache.rs`; modify `embedding.rs`, `embedding/{runtime,service}.rs`, builder | Byte bound and whole-cache expiry |
| NER | Modify `config/ner.rs`, `knowledge/api.rs`, `knowledge/entity_extraction.rs`, `knowledge/entity_extraction/anno.rs`, `memory/episode/{fact_extraction,entity_extraction}.rs` | Explicit UTF-8 byte refusal before amplification/writes |
| Shared rules | Modify `shared/triple_extractor.rs`, builder | Process-shared compiled immutable regexes |
| Startup reads | Modify `knowledge/{api,knowledge_store}.rs`, `service/startup.rs` | Owner count/sample metadata |
| Projections | Modify `knowledge/{queries,knowledge_store,graph_store}.rs` | Retrieval-only payload reduction, no ranking rewrite |
| Artifact pages | Modify `http/tasks/worker.rs` | Bounded row pages without audit deletion |
| Maintenance | Create `src/bootstrap/http_maintenance.rs`; modify `bootstrap.rs`, `memory/api.rs`, `memory/lifecycle_workers.rs`, workers, `http/lifecycle.rs`, `http/tasks/scheduler.rs`, `http/embedding/backfill_scheduler.rs`, approved composition shims | Narrow wiring and owning use cases, no MemoryService construction for lifecycle |
| Background | Modify `embedding/{api,runtime,service}.rs`, `embedding/providers/task_runner.rs`, HTTP deployment wiring | One process-wide admitted/running/input budget and tracked lifetime |
| Metrics | Modify `http/metrics.rs`, `observability.rs`, `http/runtime/bootstrap.rs` | 5-second upkeep, 5 × 60-second window, owned shutdown |
| Subscription | Modify `mcp/handlers.rs`, `http/subscriptions.rs`, `http/transport.rs` only if reproduction proves ownership issue | Stream captures narrow ports/scalars rather than whole service |

Where table paths abbreviate `src/`, their full prefix is `crates/memory-mcp/src/`. Tests are itemized per task. Include module declarations in the owning parent, not new broad re-exports for tests. Do not move unrelated modules.

---

### Task 1: External Linux memory sampler

**Files:**
- Create: `crates/xtask/src/memory.rs`.
- Modify: `crates/xtask/src/main.rs` (Command and dispatch).
- Create: `docs/performance/HTTP_MEMORY_BASELINE.md` (protocol and field semantics).
- Test: pure cases inside new module; Linux subprocess/procfs integration tests labeled as integration.

**Interfaces:**
- Produces `SampleMemory` xtask command: `--pid <u32> --duration-secs <u64> --interval-ms <u64> --output <PathBuf>`.
- New `MemorySample { elapsed_ms: u64, rss_kib: u64, swap_kib: u64, hwm_kib: u64, cgroup_current_bytes: Option<u64>, cgroup_swap_bytes: Option<u64> }`.
- `parse_status(input: &str) -> Result<(u64, u64, u64), String>`; `sample_linux(pid: u32, elapsed_ms: u64) -> Result<MemorySample, String>`; `run(pid: u32, duration_secs: u64, interval_ms: u64, output: &Path) -> Result<(), String>`.
- Use existing std only for JSON-lines writing with exact fixed field keys; no new serde/dependency required. One line/sample, valid JSON integer/null values. `run` owns one monotonic Instant origin and supplies its elapsed_ms to each sample; no per-sample origin reset. UTC phase timestamps in companion evidence.

- [x] **Step 1: Write failing parser/argument cases.** `status_preserves_kib_units` asserts parsing `VmRSS: 100 kB`, `VmSwap: 20 kB`, `VmHWM: 140 kB` yields `(100,20,140)`; `missing_status_field_is_error`, `invalid_number_is_error`, `zero_interval_is_error`. Linux integration samples a bounded child, checks fields exist, exits cleanly on child death; permissions errors are errors, not zero.
- [x] **Step 2: Run RED.** `cargo test -p xtask memory --locked`; observed unresolved `parse_status` (RED). Later GREEN on Rust 1.99 Linux: 4 unit tests + 2 Linux integration tests.
- [x] **Step 3: Implement sampler and command.** Discover cgroup v2 membership from process cgroup; if no visible cgroup mount, return nullable cgroup fields with explicit warning. No host-name/image/env/log dumps. Snapshot errors terminate with nonzero exit; flush output incrementally; duration and interval bounded.
- [x] **Step 4: Run GREEN.** Rust 1.99 Linux container ran `cargo test -p xtask --locked`; Linux integration tests sampled a bounded child and rejected a dead PID, with valid JSONL and finite exit. No staging PID was supplied, so this verifies sampler mechanics only, not the production/staging workload.
- [ ] **Step 5: Review and commit.** `git add crates/xtask/src/memory.rs crates/xtask/src/main.rs docs/performance/HTTP_MEMORY_BASELINE.md`; `GIT_EDITOR=true git commit -m "feat(xtask): add bounded Linux memory sampling"`.

### Task 2: Controlled baseline and allocation attribution — mandatory decision gate

**Files:**
- Modify: `docs/performance/HTTP_MEMORY_BASELINE.md`.
- Create: `crates/memory-mcp/tests/http_memory_workload.rs` (ignored external-staging integration workload).
- Test: new workload, using existing HTTP fixtures/protocol envelopes as references; do not use embedded DB test process as production RSS baseline.

**Interfaces:**
- Workload env: `HTTP_MEMORY_TEST_BASE_URL`, `HTTP_MEMORY_TEST_API_KEY`, `HTTP_MEMORY_TEST_DESTRUCTIVE_STAGING=1` required, `HTTP_MEMORY_TEST_PHASE_FILE` optional artifact path. Refuse without explicit staging acknowledgement. Never print key.
- Workload scenarios named `small_mixed_memory_workload`, `unicode_extract_boundary_workload`, `provider_outage_workload`; all bounded finite requests. Fixture provider responses deterministic 2048 f64 dimensions, independent of NVIDIA.
- Evidence produces build commit/features/allocator, dataset sizes, limits, timestamps, success/refusal counts, cache/task/runtime counters when available, Linux sampler output, profiler peak/live stacks. No claimed root owner without call stacks.

- [x] **Step 1: Add failing workload-safety test.** `external_workload_refuses_missing_staging_acknowledgement` asserts refusal before HTTP client calls; test selection/config parsing via controlled inputs, no process-global env mutation in unit test. Add an integration subprocess test for env parsing.
- [x] **Step 2: Run RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_memory_workload --locked` first failed on missing guard, then passed 5 safety/config tests; no external HTTP client is constructed by those tests.
- [ ] **Step 3: Implement finite synthetic workload.** Dedicated tenant: 24 episodes <=64 KiB, 129 facts <=4 KiB, 454 entities. Seed deterministic schema-valid records through staging-only fixture adapters using existing migrations; do not assume NER generates those exact counts. Resolve the staging DB connection explicitly, validate it is distinct from production before any seed write, and verify seeded counts before sampling. 5 warmups + 20 cycles, each 10 identical recalls, 10 unique recalls, 1 small extract; synthetic source IDs unique per run. Run one operation at a time for main gate; separate 8-operation concurrency scenario. Three fresh-process repeats per variant. Count refusals separately; no assertion loops mixed with scenario branching in unit tests.
- [ ] **Step 4: Execute staging-only protocol.** Exact-image baseline if source provenance obtainable; otherwise reproduce local HEAD and label deployed comparison noncausal. Measure startup-before-ready-tenant, health-only, first activation/recall/extract, repeated cycles, maintenance and quiescence. Quiescence uses controlled provider in-flight=0 and completed workload requests, plus existing lifecycle/task/pool observations where available, bounded by 10 minutes. Provider zero alone does not prove all background work drained. If a required owner has no observable drain signal, mark quiescence inconclusive and stop the idle acceptance gate; propose a narrow observer separately rather than inventing counters or silently adding public metrics. Time sampling may sleep, correctness ordering tests may not. Allocation profiler in separate diagnostic run with symbols; never preload/debug production. Include provider outage and no-scrape runs.
- [x] **Step 5: Gate before implementation.** Gate recorded unresolved: no isolated staging deployment/provider/database was supplied. Production was not used. Continue Tasks 3–14 only as robustness improvements, not root-cause fixes or measured RAM reduction. If dominant owner lies outside tasks 3–14, seek review of owner-specific patch first. Robustness slices may proceed separately, labeled not root-cause fixes. This gate cannot be checked GREEN merely because HTTP returns 2xx.
- [ ] **Step 6: Commit evidence/protocol, not secrets/raw user data.** `GIT_EDITOR=true git commit -m "test(memory): establish controlled HTTP memory baseline"`; stage only this task's files and sanitized evidence references.

### Task 3: Bound preflight buffering before ordinary admission

**Files:**
- Create: `crates/memory-mcp/src/http/middleware/preflight_budget.rs`.
- Modify: `crates/memory-mcp/src/http/middleware.rs`, `http/middleware/preflight.rs:195–327`, `http.rs`, `http/config/parse.rs`, `http/config/types.rs`, `http/config/validate.rs`.
- Create test: `crates/memory-mcp/tests/http_preflight_buffering.rs`.
- Docs: `README.md` HTTP config table/overload precedence.

**Interfaces:**
- `PreflightBudget::new(request_limit: usize, byte_limit: usize) -> Result<Self, MemoryError>`.
- `PreflightBudget::try_reserve(self: &Arc<Self>, body_limit_bytes: usize) -> Result<PreflightReservation, PreflightRefusal>`; refusal enum `Requests | Bytes`; reservation non-Clone and Drop releases all resources synchronously.
- `HttpState.preflight_budget: Arc<PreflightBudget>`; default new knobs: slots=20, bytes=67108864. Reserve declared Content-Length before polling; incrementally charge observed bytes before copying each data frame. Serde defaults; positive/checked validation and bytes>=body_limit.

- [ ] **Step 1: Write RED resource cases.** `byte_refusal_returns_acquired_slot`: limit=(2 slots,10 B), first reserve(10), second reserve(1) is Bytes, dropping first allows reserve(10). `slot_refusal_does_not_charge_bytes`; `drop_restores_both_resources`; `reservation_growth_is_bounded_and_released`; checked overflow and Semaphore::MAX_PERMITS validation. Observe successful/failed reservation, not private atomic internals.
- [ ] **Step 2: Add request regressions.** Controlled Body signals first poll, waits on permit. While first body blocks, slot-denied request returns 503 with zero body polls; declared-byte refusal also occurs before body polling. Separate cancellation-in-collection, cancellation-downstream, malformed JSON, oversized advertised length, understated Content-Length that exceeds aggregate byte budget, chunked exact/+1 byte, stream-error, and subscription-response cases. Cheap headers retain 415/406/413 even when saturated. Downstream receives original bytes and validation metadata; default 20-tenant small-body workload passes without raising the 64 MiB budget. Use existing `HttpStateTestBuilder` and channels, no sleeps.
- [ ] **Step 3: Run RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_preflight_buffering --locked`.
- [ ] **Step 4: Implement reservation with nonblocking semaphore and checked atomic byte ledger.** Reserve valid declared length before collect after cheap checks; for absent/invalid length start at zero, then atomically charge each cumulative observed body length before copying that frame into the preflight buffer. Keep the per-request `Limited` hard body cap. Keep guard through downstream await, explicitly drop parsed JSON before it; release on headers return, not SSE completion. No unbounded waiter queue or header-based early authentication. Response 503 text fixed by spec.
- [ ] **Step 5: Run GREEN and compatibility.** New target, then `--test http_proto_conformance`, `--test http_proxy_streaming`, `--test http_load_concurrency` with same features. Existing 20-tenant success scenario must pass with defaults; do not lower limits to hide failures. Document raw-byte versus RSS distinction.
- [ ] **Step 6: Review/commit task files only.** `GIT_EDITOR=true git commit -m "fix(http): bound preflight buffering before body collection"`.

### Task 4: Byte-bounded context cache and invalidation fence

**Files:**
- Create: `crates/memory-mcp/src/platform/context_cache.rs`, `src/config/memory.rs`.
- Modify: `platform.rs`, `config.rs`, `platform/context_cache_key.rs`, `memory/context_cache.rs`, `memory/context_cache/invalidation.rs`, `memory/retrieval.rs`, `memory/retrieval/pipeline.rs:132–148`, `service/core/builder.rs`, composition cache-limit wiring.
- Update consumers' type imports in `memory/retrieval_deps.rs`, `memory/capabilities/deps.rs`, `embedding/service.rs` without changing their responsibilities.
- Test: module public-interface unit cases; `tests/embedded_context_cache.rs` integration invalidation.

**Interfaces:**
- `ContextCache = Arc<RwLock<ContextCacheState>>`; raw LRU private.
- `CacheGeneration(u64)`; `ContextCacheLookup::Hit(Vec<AssembledContextItem>) | Miss(CacheGeneration)`; `CacheInsertOutcome::Stored | Oversized | StaleGeneration`.
- `ContextCacheState::new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize) -> Self`; `lookup(&mut self, key: &CacheKey) -> ContextCacheLookup`; `insert(&mut self, generation: CacheGeneration, key: CacheKey, items: &[AssembledContextItem]) -> CacheInsertOutcome`; `clear(&mut self)`, `len(&self)->usize`, `accounted_bytes(&self)->usize`.
- New composition value `CacheLimits { context_bytes: NonZeroUsize, query_bytes: NonZeroUsize }`, parsed once with `from_env(default_context_bytes: usize) -> Result<Self, MemoryError>`; HTTP context default=4194304, local=16777216, query=2097152; retain constructor default paths, inject resolved limits before serving. Config business validation stays config-owned.
- Pipeline `check_cache` returns lookup enum; `store_cache` consumes captured generation. Both invalidation paths invoke `clear()`.

- [ ] **Step 1: Write RED tests via state interface.** Fixtures construct small fully specified items with nested provenance/reconciliation. `byte_cap_evicts_before_entry_cap`, `oversized_entry_is_not_cloned_or_cached`, `oversized_replacement_removes_old_entry`, `replacement_updates_weight`, `clear_releases_accounted_bytes`, `invalidation_rejects_old_generation`. For fence: miss→clear→insert(old generation) must StaleGeneration and len=0. Use numeric exact bound tests with empty/minimal fixtures; document node allowance rather than asserting serialization size.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --lib context_cache --locked`; integration `cargo test -p memory_mcp --test embedded_context_cache --locked` after adding assemble→invalidate→assemble visibility test.
- [ ] **Step 3: Implement weighted LRU.** Account owned capacities recursively, including keys and provenance; use checked/saturating-safe overflow as Oversized, not wrapped weights. Reject before cloning candidate payload; remove oversized previous value; evict least recent until both caps fit. Store weight, never expose mutable entry bypass. Generation never silently wraps into a previously valid token: on exhaustion disable new inserts until recreated. Do not claim exact heap bytes.
- [ ] **Step 4: Integrate cache limits and generation with retrieval.** Keep existing result Vec contract and ACL/temporal cache keys. No shared Arc-result rewrite or new response fields. Document changed reuse, not changed query output. Config subprocess tests reject zero/invalid values without global-env races.
- [ ] **Step 5: Run GREEN.** Above commands plus `--test memory_invalidation`, `--test assemble_context_reconciliation`. Confirm all invalidation adapters reset bytes/generation.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "fix(memory): cap context cache bytes and fence invalidation"`.

### Task 5: Bound query embeddings and purge all expired entries

**Files:**
- Create: `crates/memory-mcp/src/embedding/query_cache.rs`.
- Modify: `embedding.rs`, `embedding/runtime.rs`, `embedding/service.rs`, `service/core/builder.rs`; consume Task 4 CacheLimits.
- Test: new state unit cases; existing `embedding::service` ScriptedProvider scenarios.

**Interfaces:**
- `QueryEmbeddingCacheState::new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize, ttl: Duration) -> Self`.
- `get(&mut self, key: &str, now: std::time::Instant) -> Option<Vec<f64>>`; `insert(&mut self, key: String, embedding: Vec<f64>, now: Instant) -> QueryCacheInsertOutcome`; outcome `Stored | Oversized | InvalidExpiry`.
- `purge_expired(&mut self, now: Instant) -> usize`, `accounted_bytes(&self)->usize`; TTL 300 s, entries 128, bytes from Task 4.

- [ ] **Step 1: Write RED scenarios.** Exact expiry miss (`expires_at<=now`), other-key lookup purges expired, insertion purges before evicting live, hit does not renew TTL, replacement releases bytes, oversized vector bypass, vector spare capacity counted. Fixed std::Instant values, no sleeping/time pause assumption.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --lib embedding::query_cache --locked`.
- [ ] **Step 3: Encapsulate LRU and migrate one store path for foreground/background.** Expiry before vector clone. Preserve full provider-signature key; TTL computed with checked_add and InvalidExpiry on overflow. Default returned result unaffected by cache bypass. Idle retained entries remain byte-bounded; no cleanup task needed here.
- [ ] **Step 4: Run GREEN and provider behavior.** Above plus `cargo test -p memory_mcp --lib embedding::service --locked`; prove same query hits once, different signature cannot reuse vector, invalidate behavior unchanged.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "fix(embedding): bound query cache and purge expired entries"`.

### Task 6: Explicit whole-input anno limit, no silent quality change

**Files:**
- Modify: `crates/memory-mcp/src/config/ner.rs`, `knowledge/api.rs`, `knowledge/entity_extraction.rs`, `knowledge/entity_extraction/anno.rs`, factory/build-context code in `knowledge/entity_extraction.rs`, `memory/episode/fact_extraction.rs`, `memory/episode/entity_extraction.rs`.
- Test: adapter unit tests with explicit constructor limit; `tests/memory_extraction.rs` controlled no-write path; config environment subprocess target if existing NER target unsuitable, create `tests/anno_input_environment.rs`.
- Docs: `README.md` limit/refusal/large persisted episode behavior.

**Interfaces:**
- Add default trait method `EntityExtractor::max_input_bytes(&self) -> Option<usize> { None }`.
- Owner validation `knowledge::api::validate_entity_extraction_input(extractor: &(impl EntityExtractor + ?Sized), content: &str) -> Result<(), MemoryError>`.
- `AnnoEntityExtractor::with_max_input_bytes(max_input_bytes: usize) -> Result<Self, MemoryError>`; new() uses 1048576; `NerConfig.anno_max_input_bytes` carried through build context, explicit other-selector override irrelevant.
- Stable Validation message from spec, no input text. Configuration allowed 1..=1048576; constrained profile 65536.

- [ ] **Step 1: Write RED cases.** With limit=8: 8 bytes accepted, 9 rejected; `"ééééé"` rejected as 10 bytes; 9 whitespace bytes rejected before empty shortcut; labels path rejects too. Below-limit existing person/sorted/dedup/common-noun results unchanged.
- [ ] **Step 2: Add pipeline regression.** Recording episode source supplies oversized original text; assert backend calls=0, facts/entities/edges/projection writes=0. Inline ingestion may preexist; test does not require source episode rollback. Sanitization must not turn oversized original into accepted input.
- [ ] **Step 3: Run RED.** `cargo test -p memory_mcp --lib anno --locked`; `cargo test -p memory_mcp --test memory_extraction --locked` with relevant name filter.
- [ ] **Step 4: Implement guard before sanitization/copy/backend and wire policy.** Whole-input accepted semantics; no fallback/truncate/chunks. Default changed rejection of >1 MiB persisted episodes documented as explicit compatibility boundary requiring review. Do not weaken model selectors or accept irrelevant env silently.
- [ ] **Step 5: Run GREEN and labels contract.** Above plus existing `--test tools_stored_extract_reporting`, `--test ner_progress_channels`; no model-download tests needed for this lightweight change.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "fix(knowledge): reject oversized anno inputs before extraction"`.

### Task 7: Share built-in triple regexes

**Files:**
- Modify: `crates/memory-mcp/src/shared/triple_extractor.rs:79–136`, `service/core/builder.rs:108–135`.
- Test: shared extractor behavioral/unit cases; retain existing English/Russian extraction cases.

**Interfaces:** `RuleBasedTripleExtractor::shared() -> Arc<Self>` backed by `LazyLock<Arc<Self>>`. Existing new/Default and injected `Arc<dyn TripleExtractor>` remain valid.

- [ ] **Step 1: Write RED scenarios.** Independent repeated shared calls are pointer-identical; concurrent extractions on same shared extractor preserve caller source_fact_id and language normalization with no cross-call contamination. This is resource-sharing/isolation behavior, not a bare constructor test.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --lib triple_extractor --locked`.
- [ ] **Step 3: Replace only built-in constructor paths with shared Arc.** Do not change regex syntax, pattern order, extractor quality, or custom fixtures. Static retained compiled memory is intentionally process-lifetime and measured separately.
- [ ] **Step 4: Run GREEN.** Same filter plus `--test memory_extraction` affected scenario. Allocation profile confirms fewer construction stacks; no unmeasured MB claim.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "perf(extraction): share built-in triple regexes"`.

### Task 8: Owner-scoped startup metadata queries

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/api.rs`, `knowledge/knowledge_store.rs`, `service/startup.rs:62–85`.
- Create test: `crates/memory-mcp/tests/startup_embedding_metadata.rs`.

**Interfaces:**
- `FactEmbeddingMetadataReadPort: Send + Sync` with async `count_facts(&self)->Result<usize,MemoryError>` and `sample_stored_embedding_dimensions(&self,sample_size:usize)->Result<Vec<usize>,MemoryError>`.
- Adapter existing KnowledgeStoreClient. Existing startup helper signatures unchanged; delegate to owner. Sample default remains 16.

- [ ] **Step 1: Write RED real-engine contracts.** Empty namespace→0/empty; count with and without embeddings; >16 records→<=16 scalar dimensions; empty vector→dimension0; mixed dimensions preserve startup refusal/recovery. NONE/NULL behavior tested under actual schema. Controlled malformed count/dimension response returns error rather than zero.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --test startup_embedding_metadata --locked`.
- [ ] **Step 3: Implement single-statement aggregates/projections.** `SELECT count() AS count FROM fact GROUP ALL`; `SELECT array::len(embedding) AS dimension FROM fact WHERE embedding IS NOT NONE ORDER BY id ASC LIMIT $limit`. Validate against SDK/engine, checked integer decoding and schema conforming float arrays; nonconforming legacy rows must produce explicit error, not a silently valid dimension. No LET;SELECT assuming last response: adapter returns statement0. Do not alter startup decision tree.
- [ ] **Step 4: Run GREEN and startup compatibility.** New target; `cargo test -p memory_mcp --lib startup --locked`; `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_embedding_reembed --locked` relevant scenarios.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "perf(knowledge): use bounded startup embedding metadata reads"`.

### Task 9: Remove unused payload from nonsemantic retrieval

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/queries.rs`, `knowledge/knowledge_store.rs`, scoped retrieval reads in `knowledge/graph_store.rs`.
- Tests: existing `tests/embedded_fts_search.rs`, `memory_recall.rs`, `explain_provenance.rs`, `assemble_context_reconciliation.rs`.

**Interfaces:** Preserve current select_facts_filtered/select_facts_by_entity_links signatures and existing result parsers. Projection for nonsemantic fact rows: `fact_id,fact_type,content,quote,source_episode,t_valid,t_ingested,t_invalid,t_invalid_ingested,confidence,index_keys,access_count,last_accessed,entity_links,scope,policy_tags,provenance` and `ft_score` where computed. Community matching: `community_id,summary,member_entities,updated_at,ft_score`, limit25 unchanged.

- [ ] **Step 1: Write RED adapter/result tests.** Fact carries large embedding plus ACL/temporal/provenance fields; nonsemantic read does not return vector, final recall/provenance/visibility remains identical. Assert actual projected data, not SQL-text inventory. Edge projection preserves fallback identity fields used by parser.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --test embedded_fts_search --locked` relevant added cases.
- [ ] **Step 3: Apply projections to identified nonsemantic paths only.** Keep ANN embeddings for sem_score-missing fallback; preserve Surreal FTS literal escape behavior and temporal predicates. No lexical scan truncation, hub degree rewrite or ordering change hidden in this task.
- [ ] **Step 4: Run GREEN.** Four listed targets scoped by changed scenarios; compare controlled workload decoded-response sizes and peak allocations.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "perf(retrieval): project nonsemantic payloads without changing ranking"`.

### Task 10: Keyset-page artifact reconciliation without audit deletion

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs:416–478`.
- Tests: `crates/memory-mcp/tests/http_durable_tasks.rs`, `http_crash_recovery.rs`.

**Interfaces:** Existing async `reconcile_artifacts(&self)->Result<u64,MemoryError>` unchanged; page=64 projected `id,task_id,episode_id,fact_ids`; private adapter cursor type `ArtifactCursor { after_key: String, upper_key: String }`. Use current record-ID normalization, no caller-table interface.

- [ ] **Step 1: Write RED >page cases.** 129 artifacts across tenant A/B, some orphan/terminal before pending: all target pending tasks reconcile once, other tenant unchanged. Exact64 boundary, repeated pass no additional version increments, expired task artifacts preserved, malformed/nonadvancing cursor errors. Fixture must apply migration044.
- [ ] **Step 2: Add interruption/order regression.** Controlled DB fault after first page then retry; cursor advances by last scanned row even when update changed no task. Concurrent insertion outside fixed upper-id ceiling deferred to next pass; assert next pass sees it.
- [ ] **Step 3: Run RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_durable_tasks --locked` added names.
- [ ] **Step 4: Implement single-statement page queries.** Capture max id for tenant at start; pages `ORDER BY id ASC LIMIT $limit`, `id>type::record('task_artifact',$after_key)` and `id<=upper`. Cursor argument is record key, not prefixed full ID. Drop page before next; preserve existing task completion policy and missing-table handling. No fingerprint/cancel/fencing correctness rewrite or artifact deletion. No new index/migration.
- [ ] **Step 5: Run GREEN.** Durable and crash-recovery targets; document row cap≠byte cap, no pagination index, giant fact_ids residual. If engine record ordering/cursor fails test, stop for concrete query redesign, never fallback to full scan.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "perf(tasks): reconcile artifact history in bounded pages"`.

### Task 11: Narrow HTTP maintenance dependencies

**Files:**
- Create: `crates/memory-mcp/src/bootstrap/http_maintenance.rs`.
- Modify: `bootstrap.rs`, `memory/api.rs`, `memory/lifecycle_workers.rs:23–58`, `memory/lifecycle_workers/decay.rs`, `archival.rs`, `communities.rs`, `http/lifecycle.rs:157–195`, `http/tasks/scheduler.rs`, `http/embedding/backfill_scheduler.rs`, composition conversions in `platform/lifecycle_runtime.rs` / existing approved service shims as required.
- Tests: `tests/lifecycle_decay.rs`, `lifecycle_archival.rs`, `lifecycle_communities.rs`, `http_embedding_backfill.rs`, `http_durable_tasks.rs`.

**Interfaces:**
- `LifecycleHandles<'a>.claim_store: Arc<dyn knowledge::claims::ClaimStore>` replaces retained ClaimService reference; decay calls existing atomic retract operation. Other workers do not capture claim store if unused.
- New bootstrap constructor `lifecycle_handles<'a>(db: Arc<dyn DbClient>, namespace: &'a str, logger: &'a StdoutLogger, config: &LifecycleConfig, claim_store: Arc<dyn ClaimStore>) -> LifecycleHandles<'a>`; this performs wiring only.
- Owner use-case wrappers in `memory/api.rs`: `run_decay(handles:&LifecycleHandles<'_>,threshold:f64,half_life_days:f64)->Result<usize,MemoryError>`, `run_archival(handles:&LifecycleHandles<'_>,age_days:u32)->Result<usize,MemoryError>`, `rebuild_communities(handles:&LifecycleHandles<'_>)->Result<usize,MemoryError>`, all async.
- Extraction/backfill adapters use current capability dependency shapes and deployment shared providers/extractors. Do not introduce new container or widen a context to accept MemoryService. Lifecycle no longer constructs it. CLI lifecycle broader bootstrap redesign deferred.

- [ ] **Step 1: Write RED behavioral tests.** Decay performs same atomic fact+claim retraction; archival namespace isolation; community result identity/summary unchanged. HTTP maintenance uses injected narrow dependencies with recording extractors/providers whose call counters remain0 for decay/archival; no construction of NER dependency required. Test effects, not source string absence.
- [ ] **Step 2: Run RED.** Targeted lifecycle tests and HTTP backfill/task scenarios with streamable-http,test-fixtures.
- [ ] **Step 3: Implement narrow construction.** Direct claim-store adapter, deployment policy carried intact; workers cancellation/join preserved. Task extraction still receives anno limit/shared triples; backfill canonical vector writes and dimension signature preserve. All business behavior remains owner api. Avoid changing CLI boot paths incidentally.
- [ ] **Step 4: Run GREEN and allocation comparison.** Listed tests; profile maintenance regex/service construction stacks before/after. Community rebuild still retains O(vertices)/all edges in current algorithm: this task does not claim bounded graph memory.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "refactor(http): construct maintenance from narrow owner dependencies"`.

### Task 12: Bound detached embedding work and own its lifetime

**Files:**
- Modify: `crates/memory-mcp/src/embedding/api.rs`, `embedding/runtime.rs`, `embedding/providers/task_runner.rs`, `embedding/service.rs`, `service/core/builder.rs`, `http/runtime/bootstrap.rs`, `http/runtime/storage.rs` shared deployment injection.
- Test: runner/service controlled adapter scenarios; HTTP shutdown integration target `tests/http_server_startup.rs` or create `tests/http_background_embedding.rs` for independent resource tests.

**Interfaces:**
- `BackgroundEmbeddingLimits { max_admitted_tasks: usize, max_running_tasks: usize, max_retained_bytes: usize, total_timeout: Duration }` = 8/1/262144/60s.
- `BackgroundAdmissionError::Duplicate | TaskCapacity | ByteCapacity | Oversized | ShuttingDown`.
- `try_admit(&self, task_key:&str, retained_bytes:usize)->Result<BackgroundTaskReservation,BackgroundAdmissionError>`; RAII non-Clone guard, short synchronous registry lock; poison error handled descriptively.
- Runner shared per HTTP deployment (namespace/provider signature included in key); process-lifetime cancellation and bounded joins, no strong HttpState cycle. Stdio own runner same defaults.
- Query enqueue borrows `input:&str`; `store_query_embedding_by_key(&self,cache_key:String,embedding:Vec<f64>)` uses original identity. Task runner exposes `shutdown(&self)` and async `join_until(&self,deadline:tokio::time::Instant)->Result<(),MemoryError>` through owned job tracking; reject new tasks after shutdown.
- Fact retry durable write remains canonical `ReplaceStale`; queries >65536 input bytes bypass background retry with bounded outcome, foreground unchanged. Retain effective provider prefix <=8000 chars plus bounded keys, not original giant text.

- [ ] **Step 1: Write RED runner scenarios.** Duplicate key, full task budget, full byte budget, one running job with pending admitted bounded; refused request spawns no waiter; guard drop releases keys/bytes; abort/panic/failure free all admission. Explicit permits/barriers establish ordering.
- [ ] **Step 2: Write service regressions.** Multibyte input retained bounded; long accepted query keyed by original normalized signature, not provider prefix. Same fact ID in two namespaces not deduped together. Timeout starts at admission, includes queue/provider/backoff/persistence; provider outage stops growing admitted resources. Shutdown rejects new job and joins/aborts remaining under existing grace. Late DB commit remains idempotent, do not claim timeout cancels remote server.
- [ ] **Step 3: Run RED.** `cargo test -p memory_mcp --lib embedding --locked`; new HTTP target with streamable-http,test-fixtures. Deadline adapter tests use supplied explicit expired Instant and manually controlled futures; no new Tokio test-util feature without approval.
- [ ] **Step 4: Implement bounded admission+execution, no broad channel rewrite.** At most8 futures alive, at most1 doing provider/persistence. All cleanup RAII. Completed handles are reaped before further admission; tracking capacity is included in the same eight-job bound, never an append-only Vec of historical JoinHandles. Release of an admission slot must not permit an unbounded accumulation of unreaped handles. One total deadline; abort tracked jobs during shutdown. Refusal recorded with closed labels and existing guidance/recovery route; no source contents. Avoid work computed/owned before successful admission, except bounded identity computation. Foreground provider timeout/Retry-After semantics unchanged.
- [ ] **Step 5: Run GREEN.** Runner/service tests plus canonical vector and HTTP backfill/reembed integration targets. Check Task4 invalidation and Task5 query-cache store reused.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "fix(embedding): bound detached retries and release resources on cancellation"`.

### Task 13: Prometheus upkeep independent of scrapes

**Files:**
- Modify: `crates/memory-mcp/src/http/metrics.rs`, `observability.rs:390–403`, `http/runtime/bootstrap.rs` owner of task handles/shutdown.
- Test: `tests/recorder_installation.rs` and local recorder tests; avoid global-recorder conflicts.
- Docs: `observability/README.md` only narrow additions without overwriting user work.

**Interfaces:**
- `SUMMARY_BUCKET_DURATION: Duration = Duration::from_secs(60)` and existing bucket count5; keep documented full window300s, not passing it as per-bucket duration.
- `spawn_upkeep(handle: PrometheusHandle, cancel: CancellationToken) -> JoinHandle<()>`; interval5s, Skip missed ticks; handle task owned by HttpRuntime and joined at shutdown. This task must not hold Arc<HttpState>.

- [ ] **Step 1: Write RED recorder behavior.** Local recorder with unique bounded metric keys; no render between observations; explicit upkeep drain then verify samples/quantiles remain available. Five60-second buckets configuration validated against dependency behavior; do not only assert comments/constants. Pure upkeep driver receives a controlled tick signal in tests so no wall-clock sleeps required.
- [ ] **Step 2: Run RED.** `cargo test -p memory_mcp --features streamable-http --test recorder_installation --locked` and relevant metrics module tests.
- [ ] **Step 3: Implement upkeep and shutdown ownership.** Use dependency `run_upkeep()` from safe Tokio task; separate test driver from production timer without public test-only widening. No listener, no global second recorder, no histograms migration. Correct all builder call sites, preserve named summary families.
- [ ] **Step 4: Run GREEN and observability lint.** Above; `cargo run -p xtask -- check-observability`. Regenerated user assets only reviewed incremental changes; do not discard their WIP. Stress no-scrape samples with profiler: raw retention drains, allocator residency may lag.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "fix(metrics): upkeep HTTP summaries without scrape dependency"`.

### Task 14: Verify subscription/service ownership; fix only confirmed retention

**Files:**
- Test: extend `crates/memory-mcp/tests/http_subscription_replica.rs` and `tenancy_runtime.rs` with controlled ownership probes; fixture only test-fixtures.
- Conditional Modify: `crates/memory-mcp/src/mcp/handlers.rs:680–785`, `http/subscriptions.rs`, `http/transport.rs:139–172`.

**Interfaces:**
- Preserve subscription transport/public messages and separate subscription admission.
- Existing port owners provide stream dependencies; proposed private `SubscriptionStreamDeps` holds only queue/registry port clones, authenticated identity, shutdown token, `queue_capacity:usize`, `auth_recheck:Duration`. Do not add MemoryService field.
- Observable release via test adapter Drop notification/Weak lifetime probe; no production constructor-only tests or source-code inventory checks.

- [ ] **Step 1: Reproduce.** Open subscription with controlled stream, release operation pin, force pool idle eviction using supplied time/explicit scheduler action, activate same tenant again. Prove whether old service dependency lifetime is retained; separate legitimate subscription-state owner from unrelated context-cache/provider owner. Bounded test waits, no TTL sleeps.
- [ ] **Step 2: Run regression.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_subscription_replica --locked` specific case. If current code passes release contract, record no defect and skip production edits; this task completes as verified exclusion.
- [ ] **Step 3: If RED, narrow captures.** Construct stream deps before future, avoid self/handler capture, preserve revocation recheck, queue bounds, reconnect, and shutdown. Subscription remains functional under existing eviction policy; do not make eviction kill streams incidentally.
- [ ] **Step 4: Run GREEN.** Subscription target plus runtime/isolation tests. Confirm no duplicate retained service generations after repeated eviction/reactivation.
- [ ] **Step 5: Review/commit only if changes.** `GIT_EDITOR=true git commit -m "fix(subscriptions): avoid retaining unrelated tenant service state"`; otherwise sanitized evidence-only commit.

### Task 15: Linux memory acceptance, residual-risk decision and release proposal

**Files:**
- Modify: `docs/performance/HTTP_MEMORY_BASELINE.md` with controlled A/B and attribution.
- Modify: `README.md` constrained config contract and new refusals.
- No production VPS files are modified by this task without separate release approval.

**Interfaces:** Consumes Task1 sampler, Task2 identical workload, resolved spec limits, independent task commits. Produces acceptance table, profiler before/after, test outcomes, deploy/rollback proposal pinned by immutable image digest.

- [ ] **Step 1: Execute identical fresh-process Linux release matrix.** Three runs baseline/candidate, same2CPU constraint, controlled external DB/mock embeddings, fixed dataset/requests. Separate provider outage, concurrent uploads, maintenance, unique queries, no-scrape, subscription eviction. Track successes/refusals and p95; no network provider latency confounding.
- [ ] **Step 2: Check targets and resource plateaus.** Active footprint<=200MiB; post-work<=100MiB; >=4x reduction condition from spec; p95<=1.15 baseline for accepted operations. Collect cgroup current+swap separately, not mix it with process denominator. TTL quiescence waits for actual owners/jobs, not a fixed sleep alone. Verify 20 cycles do not monotonically add live retained resources; raw samples retained outside repo with sanitized hashes/references.
- [ ] **Step 3: Resolve unmet targets honestly.** If live/peak owner remains giant rows, lexical rescue/full graph/community rebuild, or SDK internals, stop release claims and create a narrowly scoped follow-up spec from allocation stacks. Do not silently cap recall candidates, change ranking, delete audit, switch allocator or add malloc_trim. Projection/pagination row caps are not full byte guarantees. Explicit decision: accept robustness-only release with failed RAM target or approve next owner fix; user must choose.
- [ ] **Step 4: Run shipping validation.** `cargo test -p memory_mcp --locked`; HTTP targets with `--features streamable-http,mcp-apps,test-fixtures`; `cargo fmt --all --check`; `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`; `cargo run -p xtask -- check-toolchain-pin`; observability check. Bound each invocation and report timeout/failure; unrelated failures not fixed silently.
- [ ] **Step 5: Prepare release/rollback proposal.** Exact immutable old/new image digests, retained sanitized effective config, no migration, staged config acceptance of ANNO limit, restore old image/config if target/functionality fails. Production stop/start consumes maintenance window and needs new user approval. Do not run `upgrade.sh` or deploy latest automatically.
- [ ] **Step 6: Review evidence and commit docs.** `GIT_EDITOR=true git commit -m "docs(memory): record bounded HTTP acceptance evidence"`; no claim validation passed unless executed.

---

## Explicitly deferred designs, not hidden implementation discretion

1. **NER chunking:** requires quality/span/context equivalence design; oversized input refusal is first version.
2. **Lexical/graph/community hard byte bounds:** candidate ranking, degree and summary semantics need separate specs. If acceptance stress reaches these owners, Task15 stops for focused design. Streaming union-find removes all-edge Vec but still O(vertices), not universal constant memory.
3. **SDK response frame caps / huge persisted rows:** inspect locked SDK and profile before choosing transport caps; post-decode rejection does not protect allocation peak. No invented SDK buffer sizes.
4. **Artifact indexes/cross-tick fairness:** migration permission and recovery/continuation design required. Row paging retains audit and existing semantics only.
5. **Allocator/system changes:** live-vs-retained evidence first; no kernel/cache/swappiness tuning counted as memory fix.
6. **10–20MiB idle:** separate slim-profile/worker design only if user makes this a hard requirement after variant A results.
7. **CLI lifecycle bootstrap redesign:** not implied by HTTP optimisation; narrow HTTP maintenance first.

## Self-review performed at planning stage

- Spec coverage: each core policy mapped to Tasks1–15; deferred graph/SDK/slim goals explicitly excluded from universal guarantee.
- Steps: every task has RED/GREEN or an evidence decision gate, exact paths and interface contracts; no product code bodies prewritten.
- Type consistency: CacheGeneration flows lookup→store; CacheLimits consumed by query/context; BackgroundTaskReservation cleanup owns both admission and key registration; existing lifecycle return type usize preserved.
- Review Focus: five classes mapped to named regression cases in owning tasks.
- Scope/proportion: independent slices, no broad architecture rewrite or speculative allocator fix; plan intentionally includes observational gates where root cause is not yet proved.
- Current state: **no tasks executed; no tests/builds/profiling/deploy claimed**. Only read-only SSH and source review completed; this document and companion spec are the only intended writes.

## Execution handoff

Review spec + plan together, especially new input/cache/background limits and compatibility refusals. Recommended **Subagent-driven** execution: resource lifetime, protocol order and multiple context interfaces warrant independent per-task review. Native execution is possible and cheaper but has a single whole-branch independent review at the end. Execution method and implementation approval are not yet supplied. Production modifications require a separate release approval even after plan implementation is approved.
