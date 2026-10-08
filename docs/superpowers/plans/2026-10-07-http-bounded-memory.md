# HTTP Bounded Memory — Variant A Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Уменьшить startup/active/post-work память существующего HTTP SaaS, закрыв unbounded admission/retention и ненужные аллокации, с проверенным Linux evidence вместо предположений об allocator.

**Architecture:** Независимые byte/count budgets на ранний HTTP preflight, caches и detached embedding work. Context-local use cases получают narrow ports и shared deployment dependencies; storage возвращает необходимые projections/scalars/pages. Долгосрочные host/container данные приходят из существующих node_exporter/cAdvisor через VictoriaMetrics; приложение добавляет только low-cardinality HTTP/body и bounded-resource diagnostics. Сначала allocation baseline, затем независимо reviewable TDD slices; полный продукт и протокольные контракты сохраняются.

**Tech Stack:** Rust 1.99.0, Tokio, Axum, RMCP, SurrealDB SDK 3.2.4, remote SurrealDB 3.3.0, anno 0.13.0, regex, metrics-exporter-prometheus 0.18.3, Linux procfs/cgroup v2, node_exporter, cAdvisor, VictoriaMetrics, Grafana.

**Spec:** `docs/superpowers/specs/2026-10-07-http-bounded-memory-design.md` — проект для совместного review. Перед исполнением пользователь утверждает этот design и plan. Нынешнее согласие на вариант A не является разрешением на deploy или подтверждением каждого нового limit.

## Global Constraints

- Rust channel `1.99.0`; Docker/CI/toolchain pin contract неизменен.
- No dependencies, migrations, new MCP tools или generated-code changes без отдельного approval.
- No allocator change, malloc_trim timer, model removal, remote-only profile или worker process в этом плане.
- Сохранить полный streamable-http, dual-era MCP, bi-temporal validity, namespace isolation, canonical vector policies и audit trail.
- Business rules в owning context `api.rs`; transport/composition thin, не новый shared service container.
- No production `unwrap()`, no fact deletion, no request-level namespace selection.
- Не изменять исходные пользовательские незакоммиченные HTTP config/observability правки; integration через narrow edits с review diff.
- No credentials/content в logs, fixtures или evidence; использовать synthetic data и dedicated staging namespace. Не сохранять production hostnames/IPs, endpoints, credentials, tenant IDs или container IDs в code, tests, dashboards, logs или checked-in evidence.
- Production SSH сейчас только read-only; restart, config update, debugger injection, binary replacement и deployment требуют нового разрешения.
- Долгосрочные host/container metrics брать из уже работающих `node_exporter` и cAdvisor; приложение не дублирует их и не добавляет `/proc` sampler в request path. `xtask` sampler остаётся для контролируемых acceptance runs и process-level attribution.
- External dashboard selectors разрешены только после read-only подтверждения фактических metric families и стабильных labels в VictoriaMetrics. Неизвестная/недоступная серия блокирует соответствующую panel и dashboard acceptance; она не блокирует process-level memory acceptance, если controlled run и `xtask` sampler доступны. Не подставлять guessed selectors, нули или пустые panels.
- App telemetry ограничена HTTP body/preflight measurements и process-wide snapshots bounded runtime resources; никаких allocator introspection, response-stream wrappers, request payloads или high-cardinality labels. Не добавлять memory alerts до baseline.
- Deployment-specific runtime limits передавать через validated environment/config, без host-specific compile-time values. In-memory caches и очереди — disposable process-local accelerators; durable task/fact state остаётся в owning storage. Shutdown обязан отменять и boundedly join-ить background owners.
- Constrained profile budgets: body=1048576 B; preflight=2 slots/2097152 B process-wide; ordinary operations=8 (существующий VPS setting, не memory guarantee); pool=2; subscriptions=2; maintenance=1; TTL=120 s; anno input=65536 B; context cache=4194304 B per resident tenant runtime; query cache=2097152 B per resident tenant runtime; background admitted=8/running=1/retained=262144 B/deadline=60 s process-wide. Cache totals can therefore scale with resident pool capacity; no per-tenant budget is presented as a process-wide cap.
- Candidate targets: active process VmRSS+VmSwap <=200 MiB, post-work warm idle <=100 MiB, >=4x reduction versus controlled identical-workload baseline when baseline exceeds target; 10–20 MiB idle stretch only.
- Cache bytes are accounted retained estimates, not exact allocator accounting; preflight budget covers reserved raw body, not JSON expansion or transport buffers. `Content-Length` is declared size; observed app-body bytes are bytes exposed to the body collector, not TCP/TLS/chunk framing and not necessarily the complete wire body.
- **Strict vertical TDD for every uncompleted code task:** use the task's named scenarios in listed order, one at a time: add one test at the agreed public seam, run its exact filter and verify the intended RED (a behavioral assertion failure, or the exact missing interface for a genuinely new seam), make the smallest change for that scenario, rerun that filter GREEN, then start the next scenario. An unrelated compile/import/configuration failure is not RED. Do not write all tests before the first implementation. Run broader regressions only after the task's slices are green; refactoring follows GREEN and review, not part of RED→GREEN.
- Each implementation task ends with diff review and an isolated commit only when execution is approved. Existing commits/WIP are preserved; the current plan audit itself does not commit.

## Review Focus

1. Saturated slots, unknown/understated length, cancellation and malformed-but-fully-collected bodies: protocol precedence and reservation release stay correct; body-size telemetry records only complete collection and never claims a partial size — Tasks 3 and 16.
2. Invalidation during retrieval and cache-snapshot contention: an old generation cannot repopulate the cache; accounting stays consistent, and a diagnostic snapshot neither clones payloads nor holds a lock across `await` — Tasks 4, 5 and 16.
3. Oversized persisted Unicode episode: no truncation or partial extraction; refusal precedes extraction writes, and the labels path cannot bypass the guard — Task 6.
4. Provider outage, task cancellation, shutdown and runtime eviction: pending work and retained inputs remain bounded; original query identity is preserved; no hidden old runtime generation is retained — Tasks 12 and 14.
5. NONE/NULL/empty embeddings, orphan artifacts, concurrent inserts, and external telemetry with unknown/missing/identifying selectors: startup and pagination semantics remain intact; dashboard queries fail closed instead of showing false zero or exposing deployment identity — Tasks 8, 10, 15 and 17.

---

## Status, scope split и dependencies

План — umbrella для шести независимо reviewable slices: **evidence**, **admission/caches**, **extraction/maintenance/storage**, **background/lifetime**, **exporter inventory/dashboard contract**, **HTTP diagnostics**. Не объединять их в один огромный PR; каждый task остаётся отдельной поставкой, а внешний exporter inventory — отдельным блокирующим gate для dashboard. Текущая ветка: Task 1 и Task 3 уже committed; Task 2 safety guard/harness committed, но controlled baseline и workload остаются незавершёнными; Tasks 4–5 имеют незакоммиченный WIP. Эти изменения не перезаписывать. После Task 2 выполнять следующие robustness slices только с явной маркировкой, что это не root-cause fixes и не доказанное снижение RSS. Не угадывать correction для невыявленного owner.

```text
1 sampler → 2 controlled baseline / allocation attribution
2 → 3 early buffering
2 → 4 context cache → 5 query cache
2 → 6 anno input → 7 shared triples → 11 narrow HTTP maintenance
2 → 8 startup metadata
2 → 9 nonsemantic projections
2 → 10 task artifact pages
4 + 5 → 12 background admission/lifetime
2 + 12 → 13 metrics upkeep and unified background-task shutdown
12 → 14 subscription ownership (implementation only if regression confirms)
2 → 15 verified external metric inventory / dashboard contract
3 + 4 + 5 + 12 + 13 → 16 HTTP diagnostics and bounded runtime snapshots
15 + 16 → 17 technical dashboard panels and checker coverage
3..17 → 18 acceptance / controlled release proposal
```

Config/builder/shared composition files пересекаются: не разрешать одновременно писать туда нескольким agents. Parallel implementation возможна только с disjoint write sets; иначе sequential integration.

### Live baseline observation, not acceptance evidence

Read-only SSH 2026-10-07: image sha256 `0fdc287b8a8843007535b88197517d3ab0256beaa7fb8895fed3dbb0ebcc7247`, OCI revision absent; PID 3443295 started 11:04:33 UTC. At 11:38:12 VmRSS=371460 KiB, VmSwap=346908 KiB; at 11:47:20 793588/639252 KiB; at 11:47:53 751960/727256 KiB, HWM=1215228 KiB. Body/pool/request settings already reduced. Same PID grew; traffic was not isolated. Session MCP recall may or may not address this deployment. Do not call this cold idle, causal A/B or proven leak.

Exporter discovery to date: cAdvisor directly exposed candidate families `container_memory_rss`, `container_memory_working_set_bytes`, `container_memory_usage_bytes`, `container_memory_cache`, and `container_memory_swap`, and a Docker Compose service label was present. These facts do **not** prove those families/labels are stored in VictoriaMetrics. A direct node_exporter scrape returned 401; that does **not** prove its series are absent from VictoriaMetrics. Task 15 must verify the exact stored names and stable selector labels through the secured read-only VictoriaMetrics query path; no guessed external selector is accepted.

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
| Metrics upkeep | Modify `crates/memory-mcp/src/bin/memory_mcp_http.rs`, `http/metrics.rs`, `observability.rs`, `http/runtime/bootstrap.rs` | 5-second upkeep, 5 × 60-second window, explicit task owner and shutdown/join; later extended to refresh bounded resource snapshots |
| External metric contract | Create `observability/tests/test_external_metrics.py`; conditionally create `observability/external_metrics.yml` only after safe VictoriaMetrics inventory; modify `observability/check_dashboards.py`, `crates/xtask/src/observability.rs`; update `docs/performance/HTTP_MEMORY_BASELINE.md` | Pin only VictoriaMetrics-confirmed cAdvisor/node_exporter families and non-identifying stable matchers; no endpoint, credential, hostname, IP, instance/container ID or tenant ID |
| HTTP diagnostics | Modify `http/middleware/preflight.rs`, `http/middleware/preflight_budget.rs`, `http/logging.rs`, `http/metrics.rs`, `crates/memory-mcp/src/logging.rs`, `shared/observability.rs`, `observability.rs`, `http/runtime/{bootstrap,pool,storage}.rs`; add `http/runtime/memory_snapshot.rs`; add narrow snapshot accessors to cache/task owners | Exact collector-observed body bytes, closed-vocabulary refusals, rate-bounded DEBUG events, process-wide accounted runtime/cache/task gauges without payload copies or tenant labels |
| Technical dashboard | Modify `observability/build_dashboards.py`, `observability/check_dashboards.py`, `observability/README.md`; regenerate `observability/dashboards/technical.json` | Confirmed external host/container memory plus application-only memory diagnostics; no silent empty panels |
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
- `run` validates `duration_secs` in `1..=86400` and `interval_ms` in `1..=60000` before creating the output; the current CLI does not impose a separate sample-count cap. Acceptance protocol must use `interval_ms >=1000` and finite phase windows, so output volume is bounded for this workload. Use existing std only for JSON-lines writing with exact fixed field keys; no new serde/dependency required. One line/sample, valid JSON integer/null values. `run` owns one monotonic Instant origin and supplies its elapsed_ms to each sample; no per-sample origin reset. UTC phase timestamps in companion evidence.

- [x] **Step 1: Write failing parser/argument cases.** `status_preserves_kib_units` asserts parsing `VmRSS: 100 kB`, `VmSwap: 20 kB`, `VmHWM: 140 kB` yields `(100,20,140)`; `missing_status_field_is_error`, `invalid_number_is_error`, and `zero_interval_is_error_before_output_is_created`. Linux integration samples a bounded child, checks fields exist, exits cleanly on child death; permissions errors are errors, not zero.
- [x] **Step 2: Run RED.** `cargo test -p xtask memory --locked`; observed unresolved `parse_status` (RED). Later GREEN on Rust 1.99 Linux: 4 unit tests + 2 Linux integration tests.
- [x] **Step 3: Implement sampler and command.** Discover cgroup v2 membership from process cgroup; if no visible cgroup mount, return nullable cgroup fields with explicit warning. No host-name/image/env/log dumps. Snapshot errors terminate with nonzero exit; flush output incrementally; duration and interval bounded.
- [x] **Step 4: Run GREEN.** Rust 1.99 Linux container ran `cargo test -p xtask --locked`; Linux integration tests sampled a bounded child and rejected a dead PID, with valid JSONL and finite exit. No staging PID was supplied, so this verifies sampler mechanics only, not the production/staging workload.
- [x] **Step 5: Review and commit.** Committed as `2d61dfcb` (`feat(xtask): add bounded Linux memory sampling`).

### Task 2: Controlled baseline and allocation attribution — mandatory decision gate

**Files:**
- Modify: `docs/performance/HTTP_MEMORY_BASELINE.md`.
- Create test: `crates/memory-mcp/tests/http_memory_workload.rs` (runnable local controlled-server contract tests plus a separately ignored external-staging workload entrypoint).
- Test: new workload, using existing HTTP fixtures/protocol envelopes as references; do not use embedded DB test process as production RSS baseline.

**Interfaces:**
- Workload env: `HTTP_MEMORY_TEST_BASE_URL`, `HTTP_MEMORY_TEST_API_KEY`, `HTTP_MEMORY_TEST_DESTRUCTIVE_STAGING=1` required, `HTTP_MEMORY_TEST_PHASE_FILE` optional artifact path. Refuse without explicit staging acknowledgement. Never print key.
- Workload scenarios named `small_mixed_memory_workload`, `unicode_extract_boundary_workload`, `provider_outage_workload`; all bounded finite requests. Fixture provider responses deterministic 2048 f64 dimensions, independent of NVIDIA.
- Evidence produces build commit/features/allocator, dataset sizes, limits, timestamps, success/refusal counts, cache/task/runtime counters when available, Linux sampler output, profiler peak/live stacks. No claimed root owner without call stacks.

- [x] **Step 1: Add failing workload-safety test.** `external_workload_refuses_missing_staging_acknowledgement` asserts refusal before HTTP client calls; test selection/config parsing via controlled inputs, no process-global env mutation in unit test. Add an integration subprocess test for env parsing.
- [x] **Step 2: Run RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_memory_workload --locked` first failed on missing guard, then passed 5 safety/config tests; no external HTTP client is constructed by those tests.
- [ ] **Step 3: TDD the finite workload against a local controlled HTTP server, then implement it.** Add `small_mixed_memory_workload_emits_bounded_request_sequence`; assert exact ordered counts and synthetic payload shapes: 5 warm-up cycles each perform 1 identical recall and 1 unique recall; 20 measured cycles each perform 10 identical recalls and 10 unique recalls. Keep the measured dataset fixed during this recall phase. Extraction mutates durable state, so run exactly one small extract in its own measured phase after the recall phase, record its added episode/fact counts separately, and do not repeat it in each cycle. This avoids a progressively growing dataset masquerading as retention. Run `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_memory_workload small_mixed_memory_workload_emits_bounded_request_sequence --locked`; expected RED is missing workload behavior, not a staging/network failure. Implement only that sequence and rerun GREEN. Then add `unicode_extract_workload_uses_exact_utf8_payload` and `provider_outage_workload_stops_after_fixed_attempts` as separate RED→minimum change→GREEN cycles using the same local server; no test may contact the external staging endpoint. For actual acceptance, provision an isolated staging DB/tenant per fresh-process run from the same deterministic snapshot; seed 24 episodes <=64 KiB, 129 facts <=4 KiB and 454 entities through schema-valid staging-only fixture adapters and current migrations; do not assume NER generates those counts. Resolve and verify the staging DB identity is separate from production before any seed write; verify initial counts before sampling. Use unique synthetic source IDs per run. Run one request at a time for the main gate and a separate 8-operation concurrency scenario; repeat each variant in three fresh processes. Count refusals separately.
- [ ] **Step 4: Execute staging-only protocol.** Exact-image baseline if source provenance obtainable; otherwise reproduce local HEAD and label deployed comparison noncausal. Measure startup-before-ready-tenant, health-only, first activation/recall/extract, repeated cycles, maintenance and quiescence. Sample each finite phase with `sample-memory --interval-ms 1000` (minimum acceptance interval; do not use the CLI's 100 ms lower bound for the full matrix); split any phase longer than the sampler's one-day duration limit. Quiescence uses controlled provider in-flight=0 and completed workload requests, plus existing lifecycle/task/pool observations where available, bounded by 10 minutes. Provider zero alone does not prove all background work drained. If a required owner has no observable drain signal, mark quiescence inconclusive and stop the idle acceptance gate; propose a narrow observer separately rather than inventing counters or silently adding public metrics. Time sampling may sleep, correctness ordering tests may not. Allocation profiler in separate diagnostic run with symbols; never preload/debug production. Include provider outage and no-scrape runs.
- [x] **Step 5: Gate before implementation.** Gate recorded unresolved: no isolated staging deployment/provider/database was supplied. Production was not used. Continue Tasks 3–14 only as robustness improvements, not root-cause fixes or measured RAM reduction; Tasks 15–17 are exporter inventory/diagnostics/dashboard work, not proof of a memory reduction. If dominant owner lies outside Tasks 3–14, seek review of owner-specific patch first. Robustness slices may proceed separately, labeled not root-cause fixes. This gate cannot be checked GREEN merely because HTTP returns 2xx.
- [ ] **Step 6: Commit evidence/protocol, not secrets/raw user data.** `GIT_EDITOR=true git commit -m "test(memory): establish controlled HTTP memory baseline"`; stage only this task's files and sanitized evidence references.

### Task 3: Bound preflight buffering before ordinary admission

**Files:**
- Create: `crates/memory-mcp/src/http/middleware/preflight_budget.rs`.
- Modify: `crates/memory-mcp/src/http/middleware.rs`, `http/middleware/preflight.rs:195–327`, `http.rs`, `http/config/parse.rs`, `http/config/types.rs`, `http/config/validate.rs`.
- Create test: `crates/memory-mcp/tests/http_preflight_buffering.rs`.
- Docs: `README.md` HTTP config table/overload precedence.

**Interfaces:**
- `PreflightBudget::new(request_limit: usize, byte_limit: usize) -> Result<Self, MemoryError>`.
- Implemented `PreflightBudget::try_reserve(self: &Arc<Self>, initial_bytes: usize) -> Result<PreflightReservation, PreflightRefusal>`; the middleware passes parsed `Content-Length` or 0 when absent/unparseable. `PreflightReservation::try_reserve_through(&mut self, total_bytes: usize) -> Result<(), PreflightRefusal>` (`pub(super)`) atomically reserves only the additional bytes needed to reach the cumulative observed total, before a frame is copied. Refusal enum is `Requests | Bytes`; reservation is non-Clone and Drop synchronously releases its slot and exact accounted reservation.
- `HttpState.preflight_budget: Arc<PreflightBudget>`; environment variables `MEMORY_MCP_HTTP_PREFLIGHT_REQUEST_LIMIT` and `MEMORY_MCP_HTTP_PREFLIGHT_BYTES`, defaults slots=20 and bytes=67108864. Constrained acceptance explicitly sets 2 and 2097152. Reserve a valid declared Content-Length before polling; unknown length starts at zero; `Limited` enforces per-request `body_limit`. Serde defaults; positive/checked validation, slots<=`Semaphore::MAX_PERMITS`, aggregate bytes>=per-request body limit.

- [x] **Step 1: Add resource regression cases.** `byte_refusal_returns_acquired_slot`: limit=(2 slots,10 B), first reserve(10), second reserve(1) is Bytes, dropping first allows reserve(10). `slot_refusal_does_not_charge_bytes`; `drop_restores_both_resources`; `reservation_growth_is_bounded_and_released`; checked overflow and Semaphore::MAX_PERMITS validation. Observe successful/failed reservation, not private atomic internals.
- [x] **Step 2: Add request regressions.** Controlled Body signals first poll, waits on permit. While first body blocks, slot-denied request returns 503 with zero body polls; declared-byte refusal also occurs before body polling. Separate cancellation-in-collection, cancellation-downstream, malformed JSON, oversized advertised length, understated Content-Length that exceeds aggregate byte budget, chunked exact/+1 byte, stream-error, and subscription-response cases. Cheap headers retain 415/406/413 even when saturated. Downstream receives original bytes and validation metadata; default 20-tenant small-body workload passes without raising the 64 MiB budget. Use existing `HttpStateTestBuilder` and channels, no sleeps.
- [x] **Step 3: Run RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_preflight_buffering --locked`.
- [x] **Step 4: Implement reservation with nonblocking semaphore and checked atomic byte ledger.** Reserve valid declared length before collect after cheap checks; for absent/invalid length start at zero, then atomically charge each cumulative observed body length before copying that frame into the preflight buffer. Keep the per-request `Limited` hard body cap. Keep guard through downstream await, explicitly drop parsed JSON before it; release on headers return, not SSE completion. No unbounded waiter queue or header-based early authentication. Response 503 text fixed by spec.
- [x] **Step 5: Run GREEN and compatibility.** New target, then `--test http_proto_conformance`, `--test http_proxy_streaming`, `--test http_load_concurrency` with same features. Existing 20-tenant success scenario must pass with defaults; do not lower limits to hide failures. Document raw-byte versus RSS distinction.
- [x] **Step 6: Review/commit task files only.** Committed as `28bc4601` (`fix(http): bound preflight buffering by observed body bytes`).

### Task 4: Byte-bounded context cache and invalidation fence

**Files:**
- Create: `crates/memory-mcp/src/platform/context_cache.rs`, `crates/memory-mcp/src/config/memory.rs`, `crates/memory-mcp/tests/cache_limits_environment.rs`.
- Modify: `platform.rs`, `config.rs`, `platform/context_cache_key.rs`, `memory/context_cache.rs`, `memory/context_cache/invalidation.rs`, `memory/retrieval.rs`, `memory/retrieval/pipeline.rs:132–148`, `service/core/builder.rs`, composition cache-limit wiring.
- Update consumers' type imports in `memory/retrieval_deps.rs`, `memory/capabilities/deps.rs`, `embedding/service.rs` without changing their responsibilities.
- Test: module public-interface unit cases; `tests/embedded_context_cache.rs` integration invalidation.

**Interfaces:**
- `ContextCache = Arc<RwLock<ContextCacheState>>`; raw LRU private.
- `CacheGeneration(u64)`; `ContextCacheLookup::Hit(Vec<AssembledContextItem>) | Miss(CacheGeneration)`; `CacheInsertOutcome::Stored | Oversized | StaleGeneration`.
- `ContextCacheState::new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize) -> Self`; `lookup(&mut self, key: &CacheKey) -> ContextCacheLookup`; `insert(&mut self, generation: CacheGeneration, key: CacheKey, items: &[AssembledContextItem]) -> CacheInsertOutcome`; `clear(&mut self)`, `len(&self)->usize`, `accounted_bytes(&self)->usize`.
- New composition value `CacheLimits { context_bytes: NonZeroUsize, query_bytes: NonZeroUsize }`; `CacheLimits::from_env(default_context_bytes: usize) -> Result<Self, MemoryError>` is called once at each composition root and parses `MEMORY_CONTEXT_CACHE_BYTES` plus `MEMORY_QUERY_EMBEDDING_CACHE_BYTES`; both must be positive. Context entry cap remains 512 and query cap remains 128; HTTP defaults per resident runtime: context bytes=4194304, stdio context bytes=16777216, query bytes=2097152. Retain constructor default paths and inject resolved limits before serving. Config business validation stays config-owned; cache budgets are per runtime, not process-wide.
- Pipeline `check_cache` returns lookup enum; `store_cache` consumes captured generation. Both invalidation paths invoke `clear()`.

- [ ] **Step 1: Start the state-level vertical cycles in this order.** First `byte_cap_evicts_before_entry_cap`: use a minimal item and exact byte budget; assert least-recently-used eviction keeps both caps. Then individually cycle `oversized_entry_is_not_cloned_or_cached`, `oversized_replacement_removes_old_entry`, `replacement_updates_weight`, `clear_releases_accounted_bytes`, and `invalidation_rejects_old_generation` (miss→clear→insert old generation yields `StaleGeneration` and len=0). For every case, add only that test, run its exact name filter and observe a behavioral RED, implement only enough for that case, rerun GREEN, then add the next. Fixtures include nested provenance/reconciliation where the case needs it; document node allowance rather than asserting serialization size.
- [ ] **Step 2: Implement the weighted-LRU behavior required by the next red case only.** Account owned capacities recursively, including keys and provenance; overflow is `Oversized`, never wrapped. Reject before cloning candidate payload; remove an oversized previous value; evict least-recently-used entries until both caps fit. Store weight and expose no mutable entry bypass. Generation must not wrap into a valid old token; on exhaustion disable new inserts until recreated. Do not claim exact heap bytes.
- [ ] **Step 3: Add the retrieval integration regression as a separate cycle.** `assemble_after_invalidation_observes_fresh_context` asserts an assemble→invalidate→assemble sequence cannot return the pre-invalidation context. Run `cargo test -p memory_mcp --test embedded_context_cache assemble_after_invalidation_observes_fresh_context --locked`; implement the narrow retrieval generation wiring and rerun GREEN.
- [ ] **Step 4: Add config cases as separate subprocess cycles.** `cache_limits_use_http_and_stdio_defaults` checks HTTP context=4194304, stdio context=16777216 and query=2097152; `query_cache_budget_override_is_applied` verifies `MEMORY_QUERY_EMBEDDING_CACHE_BYTES`; `zero_context_cache_budget_fails_with_variable_name`, `zero_query_cache_budget_fails_with_variable_name`, `invalid_context_cache_budget_fails_with_variable_name` and `invalid_query_cache_budget_fails_with_variable_name` assert clear config errors. For each case, add only that subprocess test, run its listed exact filter, verify the behavioral RED, implement only that case, then rerun GREEN before proceeding. Use subprocesses, never process-global environment mutation.

```bash
cargo test -p memory_mcp --test cache_limits_environment cache_limits_use_http_and_stdio_defaults --locked
cargo test -p memory_mcp --test cache_limits_environment query_cache_budget_override_is_applied --locked
cargo test -p memory_mcp --test cache_limits_environment zero_context_cache_budget_fails_with_variable_name --locked
cargo test -p memory_mcp --test cache_limits_environment zero_query_cache_budget_fails_with_variable_name --locked
cargo test -p memory_mcp --test cache_limits_environment invalid_context_cache_budget_fails_with_variable_name --locked
cargo test -p memory_mcp --test cache_limits_environment invalid_query_cache_budget_fails_with_variable_name --locked
```
- [ ] **Step 5: Integrate limits/generation with retrieval.** Keep existing result `Vec` contract and ACL/temporal cache keys. No shared Arc-result rewrite or new response fields. Document changed reuse, not changed query output.
- [ ] **Step 6: Run GREEN regressions.** New state cases plus `cargo test -p memory_mcp --test embedded_context_cache --locked`, `--test memory_invalidation`, and `--test assemble_context_reconciliation`; confirm every invalidation adapter resets bytes/generation.
- [ ] **Step 7: Review/commit.** `GIT_EDITOR=true git commit -m "fix(memory): cap context cache bytes and fence invalidation"`.

### Task 5: Bound query embeddings and purge all expired entries

**Files:**
- Create: `crates/memory-mcp/src/embedding/query_cache.rs`.
- Modify: `embedding.rs`, `embedding/runtime.rs`, `embedding/service.rs`, `service/core/builder.rs`; consume Task 4 CacheLimits.
- Test: new state unit cases; existing `embedding::service` ScriptedProvider scenarios.

**Interfaces:**
- `QueryEmbeddingCacheState::new(max_entries: NonZeroUsize, max_bytes: NonZeroUsize, ttl: Duration) -> Self`.
- `get(&mut self, key: &str, now: std::time::Instant) -> Option<Vec<f64>>`; `insert(&mut self, key: String, embedding: Vec<f64>, now: Instant) -> QueryCacheInsertOutcome`; outcome `Stored | Oversized | InvalidExpiry`.
- `purge_expired(&mut self, now: Instant) -> usize`, `accounted_bytes(&self)->usize`; TTL 300 s, entries 128, bytes from Task 4.

- [ ] **Step 1: Start vertical cache-state cycles in this order.** `expired_entry_is_a_miss_at_exact_deadline`, `lookup_purges_other_expired_entries`, `insert_purges_expired_before_live_eviction`, `cache_hit_does_not_renew_ttl`, `replacement_releases_old_bytes`, `oversized_vector_bypasses_cache`, and `vector_spare_capacity_is_accounted`. For each named case, write only that test, run its exact name filter and verify the intended RED, make the minimum state change, and rerun GREEN before proceeding. Use fixed `std::time::Instant` values; no sleeping or assumed Tokio time pause.
- [ ] **Step 2: Implement the current failing case only.** Expire before cloning the vector; preserve the full provider-signature key; compute TTL with `checked_add` and return `InvalidExpiry` on overflow. Cache bypass must not change the returned embedding result. Idle entries remain byte-bounded; no cleanup task is added.
- [ ] **Step 3: Add service behavior cases one at a time.** `same_query_signature_uses_cached_embedding_once`, `different_provider_signature_never_reuses_embedding`, and `cache_invalidation_keeps_existing_behavior`; each gets its own exact-filter RED, minimum integration, and GREEN before the next case.
- [ ] **Step 4: Run GREEN regressions.** New `embedding::query_cache` cases and `cargo test -p memory_mcp --lib embedding::service --locked`; no performance claim without the controlled allocation comparison.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "fix(embedding): bound query cache and purge expired entries"`.

### Task 6: Explicit whole-input anno limit, no silent quality change

**Files:**
- Modify: `crates/memory-mcp/src/config/ner.rs`, `knowledge/api.rs`, `knowledge/entity_extraction.rs`, `knowledge/entity_extraction/anno.rs`, factory/build-context code in `knowledge/entity_extraction.rs`, `memory/episode/fact_extraction.rs`, `memory/episode/entity_extraction.rs`.
- Test: adapter unit tests with explicit constructor limit; `tests/memory_extraction.rs` controlled no-write path; create `tests/anno_input_environment.rs` subprocess config target.
- Docs: `README.md` limit/refusal/large persisted episode behavior.

**Interfaces:**
- Reuse existing `EntityExtractor::provider_name()`; add only `EntityExtractor::max_input_bytes(&self) -> Option<usize> { None }`. The shared owner validator is `knowledge::api::validate_entity_extraction_input(extractor: &(impl EntityExtractor + ?Sized), content: &str) -> Result<(), MemoryError>`; it reads provider and limit from the same extractor, preventing mismatched arguments. The Anno adapter calls this validator immediately before its empty-input shortcut/backend path; the episode owner calls it on the original persisted content before sanitization or writes. No duplicate byte-count logic.
- Change the typed config variant to `NerExtractorConfig::Anno { max_input_bytes: usize }`; parse `ANNO_MAX_INPUT_BYTES` only for `anno`, with default 1048576 and allowed range 1..=1048576. An explicit override for another selector is rejected. Carry the value through the existing backend factory dispatch; do not add Anno-specific state to shared `NerBuildContext`.
- `AnnoEntityExtractor::with_max_input_bytes(max_input_bytes: usize) -> Result<Self, MemoryError>` validates the allowed range; `new()` uses 1048576. The anno factory constructs from the typed variant. The constrained acceptance run explicitly sets `ANNO_MAX_INPUT_BYTES=65536`; do not add an implicit compiled profile switch.
- Stable error text is exactly `entity extraction input too large: provider={provider} actual_bytes={actual} max_bytes={max}`; never include input content.

- [ ] **Step 1: Write the first RED adapter test.** `anno_adapter_rejects_input_over_explicit_byte_limit` constructs the extractor with limit=8, passes a synthetic 9-byte input, and asserts the exact `MemoryError::Validation` message is `entity extraction input too large: provider=anno actual_bytes=9 max_bytes=8`; assert the input is absent from the error.
- [ ] **Step 2: Run only that test.** `cargo test -p memory_mcp --lib anno_adapter_rejects_input_over_explicit_byte_limit --locked`; expected RED is the missing rejection/contract, not a compile or fixture error.
- [ ] **Step 3: Implement the smallest adapter slice and run GREEN.** Add the default trait limit method, shared validator, Anno configured limit, and call the validator before the empty-input shortcut/backend. Re-run the exact test; it must pass before another test is written.
- [ ] **Step 4: Add adapter edge cases as separate vertical cycles, in order.** For each case below, add only that test, run its exact command and verify a behavioral RED, make the minimum change, then rerun the same command GREEN before continuing. `anno_accepts_exact_byte_limit` asserts 8 bytes are accepted; `anno_counts_utf8_bytes_not_characters` asserts five `é` characters are 10 bytes and rejected at limit 8; `anno_rejects_oversized_whitespace_before_empty_shortcut` asserts nine whitespace bytes reject; `anno_labels_path_obeys_same_limit` asserts the labels API returns the same refusal.

```bash
cargo test -p memory_mcp --lib anno_accepts_exact_byte_limit --locked
cargo test -p memory_mcp --lib anno_counts_utf8_bytes_not_characters --locked
cargo test -p memory_mcp --lib anno_rejects_oversized_whitespace_before_empty_shortcut --locked
cargo test -p memory_mcp --lib anno_labels_path_obeys_same_limit --locked
```

Keep below-limit extraction output identical to existing person/sorted/dedup/common-noun fixtures.
- [ ] **Step 5: Add original-episode owner regression as its own RED/GREEN cycle.** `oversized_persisted_episode_is_rejected_before_extraction_writes` uses a recording extractor with limit=8; assert backend calls=0 and fact/entity/edge/extraction-projection writes=0. Inline source ingestion may already have committed; do not assert rollback. Assert sanitization cannot make oversized original text admissible. Run `cargo test -p memory_mcp --test memory_extraction oversized_persisted_episode_is_rejected_before_extraction_writes --locked` before implementing the owner guard, then rerun it GREEN.
- [ ] **Step 6: Add configuration cases one at a time in `tests/anno_input_environment.rs`.** Each case is a separate subprocess RED→minimum config/factory change→GREEN cycle. Assert defaults=1048576, valid override is passed into the Anno extractor, zero and >1048576 reject with the variable name, and the override with a non-Anno selector is rejected as irrelevant. Run each exact filter before and after its change:

```bash
cargo test -p memory_mcp --test anno_input_environment anno_input_limit_defaults_to_one_mib --locked
cargo test -p memory_mcp --test anno_input_environment anno_input_limit_accepts_explicit_value --locked
cargo test -p memory_mcp --test anno_input_environment anno_input_limit_rejects_zero --locked
cargo test -p memory_mcp --test anno_input_environment anno_input_limit_rejects_above_one_mib --locked
cargo test -p memory_mcp --test anno_input_environment anno_input_limit_rejects_non_anno_selector --locked
```

Typed `NerExtractorConfig::Anno { max_input_bytes }` carries the value; do not widen shared `NerBuildContext`.
- [ ] **Step 7: Run broader GREEN regressions and document compatibility.** Run `cargo test -p memory_mcp --lib anno --locked`, `cargo test -p memory_mcp --test memory_extraction --locked`, `cargo test -p memory_mcp --test tools_stored_extract_reporting --locked`, and `cargo test -p memory_mcp --test ner_progress_channels --locked`. Document that rejecting persisted episodes above 1 MiB is a compatibility boundary, without truncation/chunk fallback. No model-download tests are needed for this lightweight path.
- [ ] **Step 8: Review/commit.** `GIT_EDITOR=true git commit -m "fix(knowledge): reject oversized anno inputs before extraction"`.

### Task 7: Share built-in triple regexes

**Files:**
- Modify: `crates/memory-mcp/src/shared/triple_extractor.rs:79–136`, `service/core/builder.rs:108–135`.
- Test: shared extractor behavioral/unit cases; retain existing English/Russian extraction cases.

**Interfaces:** `RuleBasedTripleExtractor::shared() -> Arc<Self>` backed by `LazyLock<Arc<Self>>`. Existing new/Default and injected `Arc<dyn TripleExtractor>` remain valid.

- [ ] **Step 1: Add and run RED for `built_in_shared_extractor_is_reused`.** The public `shared()` contract returns pointer-identical `Arc`s across repeated calls; the expected RED is failure of that contract, not a constructor error.
- [ ] **Step 2: Implement only process-shared built-in construction and rerun GREEN.** Back `RuleBasedTripleExtractor::shared()` with `LazyLock<Arc<Self>>`; leave `new`/`Default` available.
- [ ] **Step 3: Add and run RED for `concurrent_shared_extractions_preserve_fact_identity_and_language`.** Assert caller `source_fact_id` and language normalization are isolated across concurrent calls; implement only if needed, then rerun GREEN. Do not change regex syntax, pattern order, extraction quality, or custom fixtures.
- [ ] **Step 4: Run extraction regression and allocation comparison.** Run the triple extractor filter and affected `--test memory_extraction` scenario. Confirm fewer construction stacks; do not claim an unmeasured number of MiB. Static compiled memory is intentionally process-lifetime.
- [ ] **Step 5: Review/commit.** `GIT_EDITOR=true git commit -m "perf(extraction): share built-in triple regexes"`.

### Task 8: Owner-scoped startup metadata queries

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/api.rs`, `knowledge/knowledge_store.rs`, `service/startup.rs:62–85`.
- Create test: `crates/memory-mcp/tests/startup_embedding_metadata.rs`.

**Interfaces:**
- `FactEmbeddingMetadataReadPort: Send + Sync` with async `count_facts(&self)->Result<usize,MemoryError>` and `sample_stored_embedding_dimensions(&self,sample_size:usize)->Result<Vec<usize>,MemoryError>`.
- Adapter existing KnowledgeStoreClient. Existing startup helper signatures unchanged; delegate to owner. Sample default remains 16.

- [ ] **Step 1: Add and run real-engine RED/GREEN cycles one at a time in the listed order.** Assertions: `empty_namespace_returns_zero_and_no_dimensions` → count=0 and empty sample; `fact_count_includes_rows_without_embeddings` → unembedded fact contributes to count; `dimension_sample_is_limited_to_sixteen_scalar_rows` → at most 16 dimensions from rows in stable id order; `empty_embedding_reports_dimension_zero` → present empty array yields 0; `none_and_null_embeddings_follow_schema_contract` → absent/null embeddings are skipped; `mixed_dimensions_preserve_startup_refusal_or_recovery` → existing startup decision is unchanged; `malformed_count_or_dimension_response_is_an_error` → explicit error, never a coerced zero. For each case, add only that test, run the exact command below with that literal case filter, verify the behavioral RED, implement the minimum adapter change, and rerun GREEN before the next case. Use the real SurrealDB test engine and current schema/migrations.

```bash
cargo test -p memory_mcp --test startup_embedding_metadata empty_namespace_returns_zero_and_no_dimensions --locked
cargo test -p memory_mcp --test startup_embedding_metadata fact_count_includes_rows_without_embeddings --locked
cargo test -p memory_mcp --test startup_embedding_metadata dimension_sample_is_limited_to_sixteen_scalar_rows --locked
cargo test -p memory_mcp --test startup_embedding_metadata empty_embedding_reports_dimension_zero --locked
cargo test -p memory_mcp --test startup_embedding_metadata none_and_null_embeddings_follow_schema_contract --locked
cargo test -p memory_mcp --test startup_embedding_metadata mixed_dimensions_preserve_startup_refusal_or_recovery --locked
cargo test -p memory_mcp --test startup_embedding_metadata malformed_count_or_dimension_response_is_an_error --locked
```
- Query/decoder implementation for each current RED only: use one statement per operation, `SELECT count() AS count FROM fact GROUP ALL` and a scalar dimension projection ordered by `id` with `LIMIT $limit`; exclude both NONE and NULL embeddings while preserving an empty vector as dimension 0. Validate actual syntax/decoding against the locked SDK and real engine; decode counts with checked integer conversion and dimensions only from schema-conforming float arrays. Nonconforming legacy rows return an explicit error, not a silently valid dimension. Consume statement 0; do not assume a `LET; SELECT` response shape or alter the startup decision tree.
- [ ] **Step 2: Run GREEN and startup compatibility.** `cargo test -p memory_mcp --test startup_embedding_metadata --locked`; `cargo test -p memory_mcp --lib startup --locked`; and `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_embedding_reembed --locked` for relevant scenarios.
- [ ] **Step 3: Review/commit.** `GIT_EDITOR=true git commit -m "perf(knowledge): use bounded startup embedding metadata reads"`.

### Task 9: Remove unused payload from nonsemantic retrieval

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/queries.rs`, `knowledge/knowledge_store.rs`, scoped retrieval reads in `knowledge/graph_store.rs`.
- Tests: existing `tests/embedded_fts_search.rs`, `memory_recall.rs`, `explain_provenance.rs`, `assemble_context_reconciliation.rs`.

**Interfaces:** Preserve current select_facts_filtered/select_facts_by_entity_links signatures and existing result parsers. Projection for nonsemantic fact rows: `fact_id,fact_type,content,quote,source_episode,t_valid,t_ingested,t_invalid,t_invalid_ingested,confidence,index_keys,access_count,last_accessed,entity_links,scope,policy_tags,provenance` and `ft_score` where computed. Community matching: `community_id,summary,member_entities,updated_at,ft_score`, limit25 unchanged.

- [ ] **Step 1: Add and run RED for `nonsemantic_fact_read_omits_embedding_and_preserves_recall`.** Fixture has a large embedding and ACL/temporal/provenance fields; assert the returned projection omits the vector while final recall, provenance and visibility remain identical. Assert decoded data, not SQL text.
- [ ] **Step 2: Implement the minimum fact projection and rerun GREEN.** Keep ANN embeddings available to semantic reads and sem-score-missing fallback; preserve FTS escaping and temporal predicates.
- [ ] **Step 3: Add and run RED for `graph_edge_projection_preserves_parser_fallback_identity`.** Assert the edge projection retains the identity fields consumed by the parser; update only that projection and rerun GREEN.
- [ ] **Step 4: Add any further nonsemantic projection scenario only as a separate vertical cycle.** Each added case must verify output/ranking behavior through the adapter result and be green before the next; no lexical scan truncation, hub degree rewrite, or ordering change.
- [ ] **Step 5: Run GREEN.** Run the four listed targets, scoped to changed scenarios: `cargo test -p memory_mcp --test embedded_fts_search --locked`, `cargo test -p memory_mcp --test memory_recall --locked`, `cargo test -p memory_mcp --test explain_provenance --locked`, and `cargo test -p memory_mcp --test assemble_context_reconciliation --locked`. Compare controlled-workload decoded-response sizes and peak allocations.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "perf(retrieval): project nonsemantic payloads without changing ranking"`.

### Task 10: Keyset-page artifact reconciliation without audit deletion

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs:416–478`.
- Tests: `crates/memory-mcp/tests/http_durable_tasks.rs`, `http_crash_recovery.rs`.

**Interfaces:** Existing async `reconcile_artifacts(&self)->Result<u64,MemoryError>` unchanged; page=64 projected `id,task_id,episode_id,fact_ids`; private adapter cursor type `ArtifactCursor { after_key: String, upper_key: String }`. Use current record-ID normalization, no caller-table interface.

- [ ] **Step 1: Add and run the first real-engine RED case `reconcile_pending_artifacts_across_pages_without_cross_tenant_changes`.** Use 129 artifacts split across tenant A/B with orphan/terminal rows before pending rows; assert every target pending task reconciles once and tenant B remains unchanged. Fixture applies migration 044.
- [ ] **Step 2: Implement one projected page and rerun GREEN.** Capture the tenant's maximum record id once; query pages of 64 ordered by id, with `id > type::record('task_artifact', $after_key)` and `id <= type::record('task_artifact', $upper_key)`. Both cursor fields store normalized record keys, not prefixed full ids; drop each page before fetching the next.
- [ ] **Step 3: Add the remaining real-engine cases one at a time.** `reconcile_exact_page_boundary_once`, `repeated_pass_does_not_increment_task_versions`, `expired_task_artifacts_remain_audit_records`, `malformed_or_nonadvancing_cursor_returns_error`, `later_pass_after_page_failure_reconciles_all_pending_rows`, and `insert_after_upper_key_is_seen_on_next_pass`. For each, write only that test, run its exact filter and observe the intended RED, implement only the failing behavior, then rerun GREEN. The cursor is in-memory for one reconciliation pass only: a failed invocation returns an error, and the next invocation safely rescans from the beginning; already completed work remains idempotent. Within a pass, cursor advances by the last scanned row even when no task changed; inserts beyond the fixed upper ceiling wait for the next pass.
- [ ] **Step 4: Preserve existing policy and stop on query-design failure.** Keep task-completion and missing-table handling; no fingerprint/cancel/fencing rewrite, artifact deletion, new index, or migration. If engine ordering/cursor fails, stop for a concrete query redesign—never fall back to full scan.
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

- [ ] **Step 1: Add/run RED for `http_decay_retracts_claims_atomically_without_extractor_construction`.** Assert the same atomic fact+claim retraction and zero calls to the recording extractor/provider; implement the narrow claim-store dependency and rerun GREEN.
- [ ] **Step 2: Add/run RED for `http_archival_remains_namespace_scoped_without_ner_dependency`.** Assert tenant isolation and zero extractor construction; make only the archival wiring change and rerun GREEN.
- [ ] **Step 3: Add/run RED for `http_community_rebuild_preserves_result_identity_and_summary`.** Implement only the narrow community wiring if needed, then rerun GREEN. These tests assert effects, never source-string absence.
- [ ] **Step 4: Add remaining backfill/task behavior cases individually.** Use recording providers/extractors and preserve canonical vector writes, dimension signature, cancellation and joins; run one named regression RED→minimal change→GREEN at a time.
- [ ] **Step 5: Run GREEN and allocation comparison.** Run the listed lifecycle and HTTP backfill/task targets; profile maintenance regex/service construction stacks before/after. Community rebuild still retains O(vertices)/all edges in its current algorithm, so this task does not claim bounded graph memory. Business behavior stays in owner APIs; do not change CLI bootstrap incidentally.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "refactor(http): construct maintenance from narrow owner dependencies"`.

### Task 12: Bound detached embedding work and own its lifetime

**Files:**
- Modify: `crates/memory-mcp/src/embedding/api.rs`, `embedding/runtime.rs`, `embedding/providers/task_runner.rs`, `embedding/service.rs`, `service/core/builder.rs`, `http/runtime/bootstrap.rs`, `http/runtime/storage.rs` shared deployment injection.
- Test: runner/service controlled adapter scenarios; HTTP shutdown integration target `tests/http_server_startup.rs` or create `tests/http_background_embedding.rs` for independent resource tests.

**Interfaces:**
- `BackgroundEmbeddingLimits { max_admitted_tasks: usize, max_running_tasks: usize, max_retained_bytes: usize, total_timeout: Duration }` = 8/1/262144/60s.
- `BackgroundAdmissionError::Duplicate | TaskCapacity | ByteCapacity | Oversized | ShuttingDown`.
- `try_admit(&self, task_key:&str, retained_bytes:usize)->Result<BackgroundTaskReservation,BackgroundAdmissionError>`; RAII non-Clone guard, short synchronous registry lock; poison error handled descriptively.
- Construct one `Arc<BackgroundTaskRunner>` in the HTTP composition root; pass clones through `RuntimeOptions` into every tenant `EmbeddingService` and retain the same Arc in `HttpRuntime` for shutdown and Task 16 snapshots. Never create one runner per tenant. The key includes namespace plus fact identity or provider signature plus normalized original query identity. Stdio constructs its own runner with the same limits.
- `BackgroundTaskRunner::resource_snapshot(&self) -> BackgroundTaskSnapshot` returns admitted/running counts and accounted retained bytes synchronously without keys or inputs. Query enqueue borrows `input:&str`; `store_query_embedding_by_key(&self, cache_key: String, embedding: Vec<f64>)` preserves the original normalized identity. The coordinator exposes `shutdown(&self)` and async `join_until(&self, deadline: tokio::time::Instant) -> Result<(), MemoryError>` through owned job tracking; reject new work after shutdown. `HttpRuntime` owns the coordinator but does not create a separate runner shutdown path; Task 13's single `HttpRuntime::join_background_tasks(deadline)` invokes this coordinator and joins the upkeep task. Jobs must not retain `HttpState`.
- Fact retry durable write remains canonical `ReplaceStale`; queries >65536 input bytes bypass background retry with bounded outcome, foreground unchanged. Retain effective provider prefix <=8000 chars plus bounded keys, not original giant text.

- [ ] **Step 1: Add/run RED for runner admission invariants one at a time.** Cases: `duplicate_key_is_refused`, `task_capacity_is_nonblocking`, `retained_byte_capacity_is_nonblocking`, `one_running_job_keeps_pending_admitted_work_bounded`, `refused_request_spawns_no_waiter`, `reservation_drop_releases_key_and_bytes`, and `abort_panic_or_failure_releases_all_admission`. Each case uses explicit permits/barriers; complete RED→minimum runner change→GREEN before writing the next.
- [ ] **Step 2: Add/run RED for service retention/identity cases one at a time.** `multibyte_input_is_accounted_by_utf8_bytes`, `long_query_uses_original_normalized_identity_not_provider_prefix`, and `same_fact_id_in_distinct_namespaces_is_not_deduplicated`; assert exact retained-byte/identity outcomes, implement narrowly, rerun GREEN per case.
- [ ] **Step 3: Add/run RED for lifecycle cases one at a time.** `deadline_includes_queue_provider_backoff_and_persistence`, `provider_outage_does_not_grow_admitted_resources`, `shutdown_rejects_admission_and_joins_or_aborts_before_deadline`, and `late_database_commit_remains_idempotent`. Deadline tests use explicit `Instant`s and manually controlled futures; do not claim timeout cancels remote server work or add Tokio test-util without approval.
- [ ] **Step 4: Implement bounded admission/execution for the current red case, not a broad channel rewrite.** At most 8 admitted futures/handles, at most 1 provider/persistence operation. RAII releases all accounting. Reap completed handles before further admission; no append-only historical `JoinHandle` vector and no slot release that allows unreaped-handle growth. One deadline starts at admission; abort tracked jobs at shutdown. Refusal uses closed labels and existing recovery guidance, never source content. Avoid owning work before successful admission except bounded identity computation. Foreground provider timeout/Retry-After behavior stays unchanged.
- [ ] **Step 5: Run GREEN.** Runner/service tests plus canonical vector and HTTP backfill/reembed integration targets. Check Task4 invalidation and Task5 query-cache store reused.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "fix(embedding): bound detached retries and release resources on cancellation"`.

### Task 13: Prometheus upkeep independent of scrapes

**Files:**
- Modify: `crates/memory-mcp/src/http/metrics.rs`, `observability.rs:390–403`, `http/runtime/bootstrap.rs`, and `crates/memory-mcp/src/bin/memory_mcp_http.rs` for explicit shutdown/join wiring.
- Test: preserve the existing single-test `tests/recorder_installation.rs`; create `tests/http_metrics_upkeep.rs` as an isolated one-test process for controlled upkeep; create `tests/http_background_shutdown.rs` for the unified upkeep/embedding-coordinator lifetime. Each test binary installs/uses at most one process-global recorder.
- Docs: `observability/README.md` only narrow additions without overwriting user work.

**Interfaces:**
- `SUMMARY_BUCKET_DURATION: Duration = Duration::from_secs(60)` and the existing bucket count=5; verify the full rolling window is 300 seconds, not `300 × 5`.
- Task 13 introduces `spawn_upkeep(handle: PrometheusHandle, cancel: CancellationToken) -> JoinHandle<()>`; Task 16 extends this existing single task with the narrow snapshot source—never start a second timer. Interval=5s with `MissedTickBehavior::Skip`. `HttpRuntime` owns the upkeep handle and the Task 12 coordinator; it exposes `join_background_tasks(&mut self, deadline: tokio::time::Instant) -> Result<(), MemoryError>`. It signals upkeep cancellation before awaiting either owner, invokes coordinator `shutdown`, then attempts both coordinator and upkeep joins under the same absolute deadline; it must not use `?` after the first join and thereby skip the second. Return the first cleanup failure as a descriptive `MemoryError` and report any secondary cleanup failure with a fixed owner label. The binary preserves the serve result as primary: if serving failed, cleanup must not replace that error; if serving succeeded, return cleanup failure if any. It keeps `HttpRuntime` alive, clones `runtime.state` for serving rather than moving it, closes admission and calls `state.shutdown.begin()` as today, joins the scheduler, then calls `runtime.join_background_tasks(tokio::time::Instant::now() + cfg.shutdown_grace).await`; this is the bounded post-serve background-cleanup allowance, not a claim that total shutdown is limited to one grace interval. Task captures only metrics handle, cancellation token and optional narrow snapshot source—not `Arc<HttpState>` or `HttpRuntime`.

- [ ] **Step 1: Add one failing upkeep behavior test.** In the new isolated `tests/http_metrics_upkeep.rs` process, record a controlled summary sample without rendering, trigger one injected upkeep tick, then assert the sample/quantile remains exposed and the verified window is 5 × 60 seconds. Keep `recorder_installation.rs`'s existing one-test, one-install contract unchanged.
- [ ] **Step 2: Run the focused test and verify RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_metrics_upkeep upkeep_refreshes_summary_without_scrape --locked`; failure must demonstrate missing upkeep behavior, not a recorder installation/discovery error. `test-fixtures` provides only the injected tick seam and is not enabled in production.
- [ ] **Step 3: Implement the minimal periodic upkeep driver.** Call the locked dependency's `run_upkeep()` from one safe Tokio task; use an injected tick source for unit tests and the five-second timer in production. Preserve existing metric names/types; no listener, second recorder, or migration of existing distributions.
- [ ] **Step 4: Test then implement unified shutdown ownership.** First add and run RED for `runtime_shutdown_cancels_upkeep_and_attempts_embedding_join` in `tests/http_background_shutdown.rs`: use controlled task completion signals; assert upkeep cancellation is signaled before awaiting the coordinator and both joins are attempted even when either returns an error. Run `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_background_shutdown runtime_shutdown_cancels_upkeep_and_attempts_embedding_join --locked`; RED must be the missing cleanup behavior. Then start exactly one task from `build_state`, store its handle in `HttpRuntime`, and give it only the state shutdown token and metrics handle. In the binary retain runtime ownership (`let state = runtime.state.clone()`), preserve `serve_result` while running cleanup, and make a serve error primary if both serving and cleanup fail; when serving succeeds, surface cleanup failure. Close admission and signal shutdown as today, join the scheduler, then call `runtime.join_background_tasks(tokio::time::Instant::now() + cfg.shutdown_grace).await`; rerun the exact test GREEN. That one method also shuts down/joins Task 12's coordinator.
- [ ] **Step 5: Run GREEN and no-scrape verification.** Run `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_metrics_upkeep --locked` and `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_background_shutdown --locked`, preserve `cargo test -p memory_mcp --features streamable-http --test recorder_installation --locked`, and run `cargo run -p xtask -- check-observability`; verify observations drain without scraping while allocator RSS may lag.
- [ ] **Step 6: Review/commit.** `GIT_EDITOR=true git commit -m "fix(metrics): upkeep HTTP summaries without scrape dependency"`.

### Task 14: Verify subscription/service ownership; fix only confirmed retention

**Files:**
- Test: extend `crates/memory-mcp/tests/http_subscription_replica.rs` and `tenancy_runtime.rs` with controlled ownership probes; fixture only test-fixtures.
- Conditional Modify: `crates/memory-mcp/src/mcp/handlers.rs:680–785`, `http/subscriptions.rs`, `http/transport.rs:139–172`.

**Interfaces:**
- Preserve subscription transport/public messages and separate subscription admission.
- Existing port owners provide stream dependencies; proposed private `SubscriptionStreamDeps` holds only queue/registry port clones, authenticated identity, shutdown token, `queue_capacity:usize`, `auth_recheck:Duration`. Do not add MemoryService field.
- Observable release via test adapter Drop notification/Weak lifetime probe; no production constructor-only tests or source-code inventory checks.

- [ ] **Step 1: Add the ownership regression.** `subscription_eviction_releases_unrelated_tenant_runtime_dependencies` opens a controlled stream, releases the operation pin, forces pool idle eviction with supplied time/explicit scheduler action, activates the same tenant again, and asserts the previous service generation is released while legitimate subscription state remains owned. Use a Drop notification/Weak probe, bounded waits and no TTL sleeps; do not couple the assertion to private fields.
- [ ] **Step 2: Run the exact test and inspect RED.** `cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --test http_subscription_replica subscription_eviction_releases_unrelated_tenant_runtime_dependencies --locked`. `mcp-apps` is required because RMCP only advertises resource-subscription capability in that feature profile. If the existing code already passes the release contract, record no defect and skip production edits; this task completes as a verified exclusion.
- [ ] **Step 3: If RED, narrow captures.** Construct stream deps before future, avoid self/handler capture, preserve revocation recheck, queue bounds, reconnect, and shutdown. Subscription remains functional under existing eviction policy; do not make eviction kill streams incidentally.
- [ ] **Step 4: Run GREEN.** Subscription target plus runtime/isolation tests. Confirm no duplicate retained service generations after repeated eviction/reactivation.
- [ ] **Step 5: Review/commit only if changes.** `GIT_EDITOR=true git commit -m "fix(subscriptions): avoid retaining unrelated tenant service state"`; otherwise sanitized evidence-only commit.

### Task 15: Verify external exporter series and define the dashboard contract

**Files:**
- Conditional Create: `observability/external_metrics.yml` only if VictoriaMetrics inventory confirms safe selectors; entries contain metric-family names, exporter owner (`cadvisor` or `node_exporter`), allowed label names/values and required non-identifying matchers only.
- Modify: `observability/check_dashboards.py`, `crates/xtask/src/observability.rs`, `observability/README.md`, `docs/performance/HTTP_MEMORY_BASELINE.md`.
- Create: `observability/tests/test_external_metrics.py` with standard-library `unittest` cases (`ExternalMetricContractTests` and `DashboardMetricContractTests`) and an explicit `unittest.main()` entry point; no new Python dependency.

**Interfaces:**
- YAML schema: `metrics: [{name, exporter, required_matchers, allowed_matchers}]`; each matcher value is an explicit set/list of approved literals. Python model: `MetricSpec(exporter: str, required_matchers: dict[str, str], allowed_matchers: dict[str, frozenset[str]])` and `ExternalMetricContract(metrics: dict[str, MetricSpec])`.
- `validate_external_expression(expr: str, contract: ExternalMetricContract) -> list[str]` in `observability/check_dashboards.py`. `validate_dashboard_external_metrics(document: dict, contract: ExternalMetricContract) -> list[str]` walks the existing nested panel/row structure and applies it to every target expression. For external families (`container_memory_*`, `node_memory_*`), accept only explicit selectors with exact `=` matchers; reject unknown families, absent required matchers, unknown matcher names/values and regex/negative/dynamic matchers. Keep this deliberately small grammar so the checker does not pretend to implement all PromQL. A missing contract is acceptable only when the dashboard contains no external exporter expression; an external expression without a verified contract fails closed.
- Contract selectors must use verified, non-identifying generic labels/values (for example, a stable service label); never store endpoint, credential, hostname, IP, `instance`, tenant ID, container ID, arbitrary label or sample value. Do not expose identifying exporter labels via saved legend text. A host-wide/fleet query must be labeled as fleet context, never as this VPS; if no safe and semantically useful selector/query exists, block that panel pending operator input rather than guessing.
- Discovery uses a read-only VictoriaMetrics query through the secured operator path. Do not write the query URL, credentials, raw labels containing identity, or live sample values to repo, logs, tests or checked-in evidence.

- [x] **Step 1: Add one failing validator test.** `ExternalMetricContractTests.test_unverified_external_family_is_rejected` calls `validate_external_expression` with a synthetic family and sanitized fixture; expect one validation error. Before the new seam exists, the intended RED is that exact missing function.
- [x] **Step 2: Run only that test and verify RED.** From the repository root, run `python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_unverified_external_family_is_rejected`; the intended RED is the missing validator/assertion, not an import or discovery error.
- [x] **Step 3: Implement the smallest external-family validator and rerun the same test GREEN.** Validate metric name and exact selector grammar against `ExternalMetricContract`.
- [x] **Step 4: Add and run the next RED/GREEN cases one at a time.** Use these exact focused invocations before and after each minimal change:

```bash
python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_unverified_selector_label_is_rejected
python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_unverified_selector_value_is_rejected
python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_required_matcher_is_enforced
python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_regex_and_negative_matchers_are_rejected
python3 -m unittest observability.tests.test_external_metrics.ExternalMetricContractTests.test_verified_family_and_selector_are_accepted
python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_dashboard_document_walks_nested_rows
```
- [ ] **Step 5: Perform the read-only VM inventory — BLOCKED.** No secured, read-only VictoriaMetrics query path or operator-confirmed safe selectors were available in this execution. Do not infer that candidate families are absent, create `external_metrics.yml`, or add exporter panels; request a sanitized selector through the secured operator path.
- [x] **Step 6: Integrate the checker and only a verified contract.** Add `observability/tests/test_external_metrics.py` after `observability/check_dashboards.py` in the `xtask` script list so dashboard generation/checking runs first and the test suite is covered by `cargo run -p xtask -- check-observability`. If inventory is GREEN, write only the verified sanitized `observability/external_metrics.yml` entries. If inventory is BLOCKED, do not create an empty or guessed contract; dashboards with no external expressions may still pass local checks, while any external expression without a contract must fail closed. Update docs with cAdvisor=container-level and node_exporter=host-level semantics; neither is process-specific Rust heap attribution.
- [x] **Step 7: Run GREEN and review.** `python3 observability/tests/test_external_metrics.py` and `cargo run -p xtask -- check-observability`; all synthetic invalid cases fail closed, approved sanitized fixtures pass. If inventory remains BLOCKED, record that external panels/dashboard acceptance remain blocked even if the local app-only dashboard check passes. No live exporter access is needed for these local checks.
- [ ] **Step 8: Commit only sanitized inventory and checker changes.** `GIT_EDITOR=true git commit -m "test(observability): validate external memory series contract"`.

### Task 16: Add low-overhead HTTP and retained-resource diagnostics

**Files:**
- Modify: `crates/memory-mcp/src/http/middleware/preflight.rs`, `http/middleware/preflight_budget.rs`, `http/logging.rs`, `crates/memory-mcp/src/logging.rs`, `http/metrics.rs`, `shared/observability.rs`, `observability.rs`, `http/runtime/{bootstrap,pool,storage}.rs`, and bounded cache/task owners from Tasks 4, 5 and 12.
- Create: `crates/memory-mcp/src/http/runtime/memory_snapshot.rs` as the small HTTP telemetry adapter; do not add a shared service container or tenant-labeled metric.
- Tests: `tests/http_preflight_buffering.rs` for body/reservation behavior; create `tests/http_preflight_metrics.rs` as an isolated one-test process for recorder-backed exposition; existing HTTP logging tests and focused owner-interface tests. Do not install a second global recorder in either existing test binary.
- Docs: `observability/README.md` and `docs/performance/HTTP_MEMORY_BASELINE.md`.

**Interfaces:**
- `PreflightObservation { declared_content_length_bytes: Option<u64>, observed_body_bytes: u64, outcome: PreflightOutcome }`; closed `PreflightOutcome`: `FullyCollected | RequestCapacityRefused | AggregateByteCapacityRefused | BodyLimitRefused | BodyReadError`. Cancellation drops the future and emits no completed-body observation. A body-stream error is classified internally as `BodyReadError` but keeps the existing HTTP response behavior; it is not counted as a capacity refusal or full body.
- Count `observed_body_bytes` exactly once in the existing frame loop before charging/copying a data frame. It means bytes exposed to the app body collector, not TCP/TLS/chunk framing; it is a prefix count on failure. A declared Content-Length is never substituted for observed bytes.
- `memory_http_preflight_body_bytes` is recorded through the existing `metrics::histogram!` facade, with no labels, exactly once after body collection completes and before JSON decoding. This includes fully collected malformed JSON because those bytes were actually buffered. Partial, refused, stream-error and cancelled bodies do not enter the distribution.
- `memory_http_preflight_refusals_total{reason}` uses only `request_capacity | aggregate_byte_capacity | body_limit`; `BodyReadError` is excluded. `memory_http_preflight_reserved_requests` and `memory_http_preflight_reserved_bytes` are gauges sampled by upkeep from the live `PreflightBudget`; `reserved_requests = request_limit - semaphore.available_permits()`, `reserved_bytes` is the existing atomic ledger. Do not update metrics on every acquire/growth/drop. The byte gauge is reserved budget (initially declared length, then collector-observed bytes), not exact resident memory.
- Process-wide no-label gauges: `memory_http_tenant_runtime_count` (ready or draining runtimes resident in `Pool`), `memory_http_context_cache_accounted_bytes` and `memory_http_query_cache_accounted_bytes` (sum of estimates in those resident runtimes), plus `memory_http_background_embedding_admitted_tasks`, `memory_http_background_embedding_running_tasks` and `memory_http_background_embedding_retained_bytes` (read once from the single process-wide coordinator, not summed once per tenant). These are accounted/owner gauges, not process RSS or complete heap accounting; resources held outside the pool-owned runtime set are not represented.
- `Pool::resident_runtime_handles(&self) -> Vec<Arc<TenantRuntime>>` clones only bounded runtime `Arc`s under the short pool lock and releases that lock before returning; it includes `Ready` and `Draining`, not `Loading`/failed slots. `TenantRuntime::retained_resource_snapshot(&self) -> impl Future<Output = TenantRetainedResources>` copies only the two cache counters under their owner locks. `HttpMemorySnapshotSource::capture(&self) -> impl Future<Output = HttpMemorySnapshot>` awaits those scalar reads sequentially, drops each lock before the next await, sums without copying cache payloads or tenant identifiers, and holds at most `pool_cap` temporary runtime `Arc`s until capture returns.
- The snapshot source contains only `Arc<Pool>`, `Arc<PreflightBudget>` and the one shared background coordinator from Task 12. Task 16 extends the existing upkeep task's signature to receive this source and set all eight gauges once per tick; it captures neither `Arc<HttpState>` nor `HttpRuntime`, and creates no second timer.
- DEBUG operations are `http.preflight.refused` and `http.preflight.large_body`, added to `logging::HTTP_OPERATIONS`; operators enable only these with `RUST_LOG=http.preflight=debug`. Logs use the validated existing `RequestId` when present, parsed numeric declared length, observed prefix bytes and closed outcome only—no request method/path, tenant fingerprint, arbitrary header or content. Emit large-body events only for fully collected bodies >=65536 B. Rate-limit each fixed event/reason to at most one log per second; refusal counters remain exact. Apply the rate limit before allocating/serializing the event.

- [ ] **Step 1: Add one failing recorder-backed measurement test.** `preflight_histogram_records_completed_body_bytes_once` is the sole test in the new `tests/http_preflight_metrics.rs` integration binary; use one fresh shared recorder, send the controlled 7-byte malformed-JSON body `{"a":]}` through preflight, assert the existing bad-request response and exactly one 7-byte observation before JSON decoding.
- [ ] **Step 2: Run the focused case and verify behavioral RED.** `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_preflight_metrics preflight_histogram_records_completed_body_bytes_once --locked`; it must fail because the observation is absent or incorrect, not because of a second-recorder/test-seam error.
- [ ] **Step 3: Implement one observation at the existing collection seam and rerun GREEN.** Do not add another body pass, payload clone or response-body wrapper.
- [ ] **Step 4: Add one failure-path test per cycle, then implement and rerun it GREEN.** `partial_refusal_is_not_recorded_as_full_body_size`, `body_read_error_is_not_a_full_size_or_capacity_refusal`, `cancelled_collection_releases_reservation_without_observation`, and `refusal_reason_uses_closed_vocabulary`. Preserve existing HTTP status semantics; only separate internal read-error classification where required to avoid falsely counting it as `body_limit`.
- [ ] **Step 5: Add snapshot behavior tests as separate RED/GREEN cycles.** In order: `snapshot_sums_two_resident_runtime_cache_estimates`, `snapshot_counts_shared_coordinator_once`, `snapshot_exposes_no_tenant_identity_or_cache_payload`, `snapshot_reflects_runtime_eviction_on_next_capture`, and `pool_eviction_proceeds_while_snapshot_waits_for_cache_lock`. Each test goes through the snapshot-source seam and controlled owner state; run one exact filter, observe behavioral RED, implement the minimum, then rerun GREEN before the next. Use explicit lock/barrier ordering and no sleeps.
- [ ] **Step 6: Add log-safety and bounded-volume tests as separate RED/GREEN cycles.** `preflight_refusal_log_contains_only_allowlisted_fields`, `large_body_log_requires_fully_collected_threshold`, `preflight_log_rate_limit_is_fixed_and_bounded`, and `preflight_debug_directive_selects_only_preflight_operations`. Assert no body/token/header/URI/tenant fingerprint, threshold=65536 B, and at most one event per fixed event/reason per second. Use a controlled monotonic clock and fixed four-slot limiter state (three refusal reasons plus large-body event), not a dynamic per-request map. Verify `RUST_LOG=http.preflight=debug` selects only those operation prefixes while the default level for `http.request` remains unchanged.
- [ ] **Step 7: Run focused RED/GREEN cycles, then aggregate suites.** Run each current Rust case with its exact filter before and after the change. Once all are green, run `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_preflight_buffering --locked`, `cargo test -p memory_mcp --features streamable-http,test-fixtures --test http_preflight_metrics --locked`, focused logging tests, `cargo test -p memory_mcp --features streamable-http --test recorder_installation --locked`, `cargo test -p memory_mcp --features streamable-http --test http_metrics_upkeep --locked`, and `cargo run -p xtask -- check-observability`.
- [ ] **Step 8: Verify exposition contract.** Extend the isolated `http_preflight_metrics` test's single end-to-end assertion sequence (not a second recorder test) to inspect HELP/TYPE/sample output for all new metric families. Assert the `memory_http_preflight_body_bytes` representation and dashboard query match the locked exporter, rather than assuming histogram `_bucket` output.
- [ ] **Step 9: Review/commit.** `GIT_EDITOR=true git commit -m "feat(http): expose bounded memory diagnostics"`.

### Task 17: Add exporter-backed memory and app diagnostics to the technical dashboard

**Files:**
- Modify: `observability/build_dashboards.py`, `observability/check_dashboards.py`, `observability/README.md`.
- Regenerate: `observability/dashboards/technical.json` through `python3 observability/build_dashboards.py`; do not hand-edit generated JSON.
- Tests: `observability/tests/test_external_metrics.py` and the `cargo run -p xtask -- check-observability` gate.

**Interfaces:**
- Add collapsed technical-dashboard rows `Host and container resources` and `HTTP memory diagnostics`.
- External PromQL may name only Task 15 contract entries. Show cAdvisor container RSS, working set and swap as distinct series; show node_exporter host available memory and host swap separately. Add optional usage/cache or CPU-context panels only for series and selectors confirmed in Task 15. Do not sum host and container values or call cAdvisor RSS process `VmRSS`.
- App panels show fully collected preflight body-size distribution (including fully collected malformed bodies), refusal rate by closed reason, current reserved request/budget-byte gauges, resident runtime count, cache estimates and process-wide background-task gauges. Keep process-level `VmRSS`/`VmSwap` sourced from controlled `xtask` evidence, not fabricated as application Prometheus series; annotate that owner gauges are not a complete heap/RSS accounting.
- Dashboard tests fail on unknown external family/label/value, reject identifying labels such as `instance`/container IDs, and distinguish missing series from zero (no `or vector(0)` fallback). No memory alert thresholds in this task.

- [ ] **Step 1: Add and run RED for the app-diagnostics dashboard contract.** `DashboardMetricContractTests.test_technical_dashboard_contains_http_memory_diagnostics` reads the generated technical dashboard and asserts the `HTTP memory diagnostics` row has the preflight distribution/refusal and bounded-resource panels. Run `python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_technical_dashboard_contains_http_memory_diagnostics`; the intended RED is the missing app row/panels, not test discovery/import.
- [ ] **Step 2: Implement the minimum app-panel generator change and rerun GREEN.** Reuse Task 15's `validate_dashboard_external_metrics` only for external expressions; do not duplicate selector parsing.
- [ ] **Step 3: Add the app-panel case as a separate RED/GREEN cycle.** `DashboardMetricContractTests.test_app_body_distribution_uses_exported_metric_type` asserts the panel query matches the locked exporter's verified representation. Run `python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_app_body_distribution_uses_exported_metric_type`, verify the named assertion fails, implement only the panel change, and rerun GREEN. Mutate synthetic in-memory dashboard documents in tests; do not write live identities or samples.
- [ ] **Step 4: Add external panels only if Task 15 inventory is GREEN.** Add and run the named dashboard test, then add each panel case separately with RED→minimum generator/checker change→GREEN. Use these exact focused invocations before and after each change:

```bash
python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_technical_dashboard_contains_separate_host_and_container_memory_queries
python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_container_memory_panels_keep_rss_working_set_and_swap_distinct
python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_host_memory_panel_does_not_sum_container_memory
python3 -m unittest observability.tests.test_external_metrics.DashboardMetricContractTests.test_dashboard_does_not_replace_missing_external_series_with_zero
```

If Task 15 is BLOCKED, leave external panels and their tests unimplemented, do not create empty rows/panels or guessed selectors, and mark this external-dashboard deliverable BLOCKED pending a safe operator-provided selector.
- [ ] **Step 5: Generate the technical dashboard.** Always add the app-only `HTTP memory diagnostics` row below service saturation/collection health. Add `Host and container resources` only when Task 15 has verified safe selectors. Keep host, container and process gauges distinct; use the exporter-confirmed metric representation for app distributions and never save hostname/instance/container ID values.
- [ ] **Step 6: Run local validation.** `python3 observability/build_dashboards.py`, `python3 observability/tests/test_external_metrics.py`, then `cargo run -p xtask -- check-observability`. Assert no duplicate RefIds, unknown family/labels/values, missing-selector fallback or `or vector(0)` in the new memory panels.
- [ ] **Step 7: Verify live external panel data only when Task 15 is GREEN.** Confirm verified queries return series through the secured VictoriaMetrics path without writing the query endpoint, credentials or identity-bearing labels to repository artifacts. If Task 15 is BLOCKED, keep external panels absent and dashboard acceptance BLOCKED; this does not invalidate process-level acceptance from the `xtask` sampler.
- [ ] **Step 8: Review/commit.** `GIT_EDITOR=true git commit -m "feat(observability): chart host and HTTP memory diagnostics"`.

### Task 18: Linux memory acceptance, residual-risk decision and release proposal

**Files:**
- Modify: `docs/performance/HTTP_MEMORY_BASELINE.md` with controlled A/B and attribution.
- Modify: `README.md` constrained config contract and new refusals.
- No production VPS files are modified by this task without separate release approval.

**Interfaces:** Consumes Task 1 sampler, Task 2 identical workload, Task 16 diagnostics, any verified Task 15–17 exporter panels, resolved spec limits, and independent task commits. Produces acceptance table, profiler before/after, exporter/app telemetry comparison where available, test outcomes, and deploy/rollback proposal pinned by immutable image digest. A blocked external panel is reported as unavailable context, not substituted with zero and not treated as a failed process-memory measurement.

**Execution status (2026-10-08):** Steps 1–3 remain **BLOCKED / NOT MEASURED** because no isolated staging deployment, database, mock provider, profiler, or verified VictoriaMetrics query path was supplied. Step 4 remains **OPEN** for the user's release choice. Step 5 passed the final local shipping checks, including strict Clippy after the scheduler argument refactor. Step 6 is **NOT PREPARED** without staging acceptance and immutable release image digests. Step 7 local evidence/docs and the integrated review are complete; the local commit records robustness work only and makes no production acceptance or release claim.

- [ ] **Step 1: Execute identical fresh-process Linux release matrix.** Three runs baseline/candidate, same 2-CPU constraint, controlled external DB/mock embeddings, fixed dataset/requests. Separate provider outage, concurrent uploads, maintenance, unique queries, no-scrape, subscription eviction. Track successes/refusals and p95; no network provider latency confounding. Sample the already-running cAdvisor and node_exporter series through VictoriaMetrics, and run the `xtask` sampler for process `VmRSS`/`VmSwap` plus cgroup current/swap. Keep host, container, process and swap denominators in separate columns.
- [ ] **Step 2: Isolate diagnostics overhead.** Compare three paired fresh-process runs with identical memory-optimization code, workload, exporter scrape cadence and 2-CPU constraint: build without Task 16 instrumentation vs instrumentation enabled at default INFO. Measure per-pair active and post-work process `VmRSS+VmSwap`, `VmHWM`, CPU-seconds per accepted operation, accepted-request p95 and app metric-series count. Obtain CPU-seconds only from a staging-only process tool that isolates the service; if it is unavailable, mark the CPU overhead gate inconclusive rather than using host-wide CPU or an unverified cAdvisor series. Separately compare three paired INFO vs `RUST_LOG=http.preflight=debug` runs on the instrumented build to measure optional DEBUG overhead and log volume; DEBUG output is rate-limited and limited to refusals or fully collected bodies >=65536 B. Require instrumentation-on median active and post-work footprint deltas each <=5 MiB, CPU-seconds/accepted-operation delta <=5%, accepted-request p95 <=1.15× telemetry-off baseline, and app metric-series count constant as requests/tenants vary. Separately require DEBUG-on vs INFO median footprint delta <=5 MiB, CPU-seconds/accepted-operation delta <=5%, and p95 <=1.15× INFO baseline. Apply the relevant paired gate to each comparison. If any criterion fails, revise telemetry and repeat the paired comparison before acceptance.
- [ ] **Step 3: Check targets and resource plateaus.** Active process footprint (`VmRSS+VmSwap`) <=200 MiB; post-work warm idle (`VmRSS+VmSwap`) <=100 MiB; >=4x reduction versus controlled identical-workload baseline when that baseline exceeds the applicable target; p95 <=1.15× baseline for accepted operations. Record `VmHWM` separately as the required process RSS peak; do not infer it from sampled cgroup or exporter values. Compare cAdvisor container RSS/working-set/swap to process sampler values but do not treat them as interchangeable; compare node_exporter host memory/swap only as pressure context. TTL quiescence waits for actual owners/jobs, not a fixed sleep alone. Verify 20 cycles do not monotonically add live retained resources; raw samples retained outside repo with sanitized hashes/references.
- [ ] **Step 4: Resolve unmet targets honestly.** If live/peak owner remains giant rows, lexical rescue/full graph/community rebuild, or SDK internals, stop release claims and create a narrowly scoped follow-up spec from allocation stacks. Do not silently cap recall candidates, change ranking, delete audit, switch allocator or add malloc_trim. Projection/pagination row caps are not full byte guarantees. If process RSS remains far above app-accounted caches/tasks/buffers, treat the gap as allocator/SDK/runtime ownership still unresolved; exporter dashboards do not prove live Rust allocations. Explicit decision: accept robustness-only release with failed RAM target or approve next owner fix; user must choose.
- [x] **Step 5: Run shipping validation.** `MEMORY_LOG_COLOR=never cargo test -p memory_mcp --locked` and `MEMORY_LOG_COLOR=never cargo test -p memory_mcp --features streamable-http,mcp-apps,test-fixtures --locked` passed on the final implementation. The first HTTP-profile run exposed a shared log-capture test selecting a concurrent event by operation name; the test now selects its unique request ID, and the repeated full run passed. `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`, `cargo run -p xtask -- check-toolchain-pin`, and `cargo run -p xtask -- check-observability` passed. Strict Clippy emitted no warnings, including no dead-code or too-many-arguments warnings. The color override makes captured log assertions deterministic and does not modify application configuration. These checks do not substitute for Steps 1–3.
- [ ] **Step 6: Prepare release/rollback proposal.** Exact immutable old/new image digests, retained sanitized effective config, no migration, staged config acceptance of ANNO limit, restore old image/config if target/functionality fails. Production stop/start consumes maintenance window and needs new user approval. Do not run `upgrade.sh` or deploy latest automatically.
- [x] **Step 7: Review evidence and commit docs.** Local evidence/docs and the independent whole-branch review/fix pass are recorded. Per the ledger ruling, task-level commits were consolidated into one integrated local commit after green gates; unrelated logging-plan documents and `observability/.tmp-trackly/` are excluded. No deployment or publish is part of this step.

---

## Explicitly deferred designs, not hidden implementation discretion

1. **NER chunking:** requires quality/span/context equivalence design; oversized input refusal is first version.
2. **Lexical/graph/community hard byte bounds:** candidate ranking, degree and summary semantics need separate specs. If acceptance stress reaches these owners, Task 18 stops for focused design. Streaming union-find removes all-edge Vec but still O(vertices), not universal constant memory.
3. **SDK response frame caps / huge persisted rows:** inspect locked SDK and profile before choosing transport caps; post-decode rejection does not protect allocation peak. No invented SDK buffer sizes.
4. **Artifact indexes/cross-tick fairness:** migration permission and recovery/continuation design required. Row paging retains audit and existing semantics only.
5. **Allocator/system changes:** live-vs-retained evidence first; no kernel/cache/swappiness tuning counted as memory fix.
6. **10–20MiB idle:** separate slim-profile/worker design only if user makes this a hard requirement after variant A results.
7. **CLI lifecycle bootstrap redesign:** not implied by HTTP optimisation; narrow HTTP maintenance first.

## Self-review performed at planning stage

- Spec coverage: core policies and exporter/app-observability additions map to Tasks 1–18; deferred graph/SDK/slim goals remain explicitly excluded from a universal guarantee.
- Steps/TDD: every unfinished code slice names its observable scenario and test seam, requires behavioral RED before the minimum change and GREEN before the next scenario; evidence-only gates do not pretend to be code tests. No product code bodies are prewritten.
- Type consistency: CacheGeneration flows lookup→store; CacheLimits consumed by query/context; BackgroundTaskReservation cleanup owns admission/key registration; the Task 16 snapshot is process-wide and consumed by the existing Task 13 upkeep owner; existing lifecycle return type usize is preserved. Preflight initial reservation and cumulative observed-byte growth match the committed Task 3 API; artifact cursors normalize both lower and upper record IDs.
- SOLID/DDD/12-factor: use cases stay with owning contexts and narrow ports; HTTP/composition remains adapters; no new service container. Runtime knobs are validated deployment config, services stay externally supplied, ephemeral caches are not durable truth, background owners have bounded shutdown, and production-specific selectors/config are not compiled or checked in.
- Review Focus: the five original behavior classes remain mapped to named regression cases; external-series and telemetry safety cases are covered by Tasks 15–17.
- Scope/proportion: independent slices, no broad architecture rewrite, per-request RSS sampling, response-stream wrapping, or speculative allocator fix; long-term exporter metrics avoid duplicate app instrumentation.
- Residual gates: Task 2's controlled workload/baseline is not complete, so the root cause and any reduction claim remain unproven. Task 15 may be blocked if no safe VictoriaMetrics selector is available; in that case no external panel or dashboard acceptance is claimed. The 10–20 MiB idle figure remains stretch, not a promise.
- Plan-state note: Task 1 and Task 3 are committed; Task 2's safety harness is committed but its controlled workload/baseline remain blocked; Tasks 4–5 contain existing uncommitted WIP and remain unchecked. This audit changed only this plan; no product code, observability artifact, companion spec, or deployment was modified.

## Execution handoff

Review spec + plan together, especially input/cache/background limits, compatibility refusals, the exporter inventory gate, DEBUG log privacy contract, and the distinction between process, container and host memory. Recommended **Subagent-driven** execution: resource lifetime, protocol order, external metric verification and multiple context interfaces warrant independent per-task review. Native execution is possible and cheaper but has a single whole-branch independent review at the end. Production modifications require a separate release approval even after plan implementation is approved.
