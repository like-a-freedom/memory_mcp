# HTTP-profile Memory: Observability & Footprint Reduction Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the HTTP profile's memory attributable (Phase 0: process/cgroup/allocator and startup-phase telemetry), so the ~1.25 GB footprint can be driven down by evidence rather than by guessing; then test and land the reduction hypotheses in Phase 1.

**Architecture:** Phase 0 adds a Linux `/proc` + glibc `mallinfo2` reader behind the existing five-second upkeep worker, exposing process RSS/swap/HWM, cgroup v2 counters, the `[heap]` extent and the glibc arena accounting as Prometheus gauges; an operator-gated on-demand `malloc_info()` dump gives per-arena detail without a rebuild. Phase 1 is a hypothesis backlog, promoted to tasks only after Phase 0 shows where the bytes are.

**Tech Stack:** Rust 2021, axum, `metrics`/`metrics-exporter-prometheus`, glibc (`mallinfo2`, `malloc_info`), cgroup v2, tokio.

**Spec:** this deployment's own evidence, not a design doc: `/opt/memory_mcp/memory-consumption.md` (operator's 2026-10-07 map of the footprint), `docs/performance/HTTP_MEMORY_BASELINE.md` (the measurement protocol and its BLOCKED gates), and `docs/performance/MEMORY_PROFILE.md` (allocator matrix). The task file itself carries the measured baseline in **Global Constraints**.

## Global Constraints

- Workspace `rust-version` 1.99.0; no toolchain, dependency, `Cargo.toml`, Dockerfile or CI-pin changes except where a task names them explicitly.
- No new MCP tools (8-tool surface frozen). New HTTP routes are allowed; a new route that leaks heap detail must be operator-gated and default-off.
- Telemetry reads only `/proc/self/*` and cgroup v2; it must never read another process's memory, request bodies, credentials, or log raw memory content.
- Reading `/proc` must degrade gracefully off Linux (return `None`, never panic, never record a wrong zero) — the local dev profile is macOS.
- The five-second upkeep worker already exists (`src/http/metrics.rs::spawn_upkeep`); do not add a second timer.
- Gauge names are prefixed `memory_`; a new constant goes in `src/shared/observability.rs` beside the existing `METRIC_HTTP_*` names.
- Tests: narrow first — `cargo test -p memory_mcp --lib <filter>`; the process-reading parsers are unit-tested against string fixtures, not against the live host.
- Before shipping: `cargo fmt --all --check` and `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` (zero warnings), plus `cargo run -p xtask -- check-observability`.
- Do NOT add a container `mem_limit` as a "fix" for the footprint: the operator has rejected a cap as the solution. A limit may only be revisited after Phase 1 lands.

### Measured baseline (2026-10-12, image `v1.33.0`, idle, ~13 min uptime)

| Signal | Value | Source |
|---|---|---|
| `VmRSS` | ~437-477 MB | `/proc/<pid>/status` |
| `VmSwap` | ~780-820 MB | `/proc/<pid>/status` |
| `VmData` | 1 221 MB (flat) | `/proc/<pid>/status` |
| `VmHWM` (peak RSS) | 1 216 MB | `/proc/<pid>/status` |
| `[heap]` extent | 1 144 MB VSZ, ~400-500 MB resident | `/proc/<pid>/maps` `[heap]` |
| cgroup `anon` / `current` / `swap` | 448 / 481 / 785 MB | cgroup v2 `memory.stat` / `memory.current` |
| App owner gauges (caches, tenant runtimes, reservations) | **all 0** | `/memory/metrics` |
| Threads / data size | 3 / ~1 MB of facts+episodes+entities | `/proc/<pid>` / prod dataset |

The whole footprint is anonymous heap over ~1 MB of data, with no local model (`NER_EXTRACTOR=anno` is rule-based; embeddings are remote). Static reading of the crate found no obviously large up-front allocation, which is why Phase 0 exists.

## Review Focus

Input classes and failure modes the spec implies but the task tests do not exercise — each is pinned by the named task test:

1. **Non-Linux hosts** — on macOS every `/proc` read returns `None` and no gauge is emitted; a `None` must never become `0` (a false "0 RSS" is worse than a missing series) — Task 1 unit `reader_returns_none_off_linux_and_never_zero`.
2. **Malformed `/proc` content** — a status file missing `VmRSS`, a `maps` line without `[heap]`, a cgroup path the process cannot read: each yields `None` for that field only, not an error for the whole snapshot and not a panic — Task 1 unit `partial_proc_content_yields_partial_snapshot`.
3. **Allocator without `mallinfo2`** — musl or a non-glibc target must skip the allocator gauges rather than fail to build or panic — Task 2 unit `malloc_info_is_absent_off_glibc`.
4. **Heap route safety** — the on-demand dump is refused (404/403) unless explicitly enabled, and never echoes request data — Task 2 test `heap_dump_route_is_disabled_by_default`.
5. **Peak vs steady** — the startup markers must show the peak transiently and the gauges must not reset `VmHWM`; an operator must be able to see that the 1.2 GB peak is a startup phenomenon, not steady state — Task 3 test `startup_markers_report_monotonic_peak`.

---

## Phase 0 — make the footprint attributable

### Task 1: Process, cgroup and allocator gauges in `/metrics`

**Files:**
- Create: `crates/memory-mcp/src/platform/process_memory.rs` (the reader + parser)
- Modify: `crates/memory-mcp/src/platform.rs` (register `pub mod process_memory;`)
- Modify: `crates/memory-mcp/src/shared/observability.rs` (metric-name constants)
- Modify: `crates/memory-mcp/src/http/runtime/memory_snapshot.rs` (`HttpMemorySnapshot` fields + `capture()`)
- Modify: `crates/memory-mcp/src/http/metrics.rs` (`record_memory_snapshot()`)
- Test: unit tests in `process_memory.rs`; one gauge-exposition test via the existing `spawn_upkeep_with_ticks` fixture.

**Interfaces:**
- Consumes: nothing from other tasks; the existing `HttpMemorySnapshotSource::capture()` call site in `spawn_upkeep`.
- Produces: `pub(crate) struct ProcessMemory { pub rss_bytes: Option<u64>, pub swap_bytes: Option<u64>, pub hwm_bytes: Option<u64>, pub vdata_bytes: Option<u64>, pub threads: Option<u64>, pub heap_bytes: Option<u64>, pub cgroup_current_bytes: Option<u64>, pub cgroup_swap_bytes: Option<u64>, pub cgroup_anon_bytes: Option<u64>, pub cgroup_file_bytes: Option<u64>, pub alloc_arena_bytes: Option<u64>, pub alloc_mmap_bytes: Option<u64>, pub alloc_in_use_bytes: Option<u64>, pub alloc_free_bytes: Option<u64>, pub alloc_keepcost_bytes: Option<u64> }` and `pub(crate) fn read() -> ProcessMemory`. Read-only; every field optional; never panics.

Metric names to add as constants: `memory_process_rss_bytes`, `memory_process_swap_bytes`, `memory_process_hwm_bytes`, `memory_process_vmdata_bytes`, `memory_process_threads`, `memory_process_heap_bytes`, `memory_cgroup_current_bytes`, `memory_cgroup_swap_bytes`, `memory_cgroup_anon_bytes`, `memory_cgroup_file_bytes`, `memory_alloc_arena_bytes`, `memory_alloc_mmap_bytes`, `memory_alloc_in_use_bytes`, `memory_alloc_free_bytes`, `memory_alloc_keepcost_bytes`.

- [ ] **Step 1: Write the failing parser unit tests**

In `process_memory.rs`, test against string fixtures (no live host):

```rust
#[test]
fn parses_status_fields() {
    let status = "VmRSS:\t  437000 kB\nVmSwap:\t  800000 kB\nVmData:\t 1221000 kB\nVmHWM:\t 1216000 kB\nThreads:\t3\n";
    let m = parse_status(status);
    assert_eq!(m.rss_bytes, Some(437_000 * 1024));
    assert_eq!(m.swap_bytes, Some(800_000 * 1024));
    assert_eq!(m.threads, Some(3));
}

#[test]
fn missing_status_field_is_none_not_zero() {
    let m = parse_status("VmSwap:\t 100 kB\n");
    assert_eq!(m.rss_bytes, None, "absent must be None, never Some(0)");
}

#[test]
fn finds_heap_extent_in_maps() {
    let maps = "5555 rw-p 0 00:00 0 [heap]\n";
    assert_eq!(parse_heap_bytes(maps, 0x100), Some(0x100 * 4096));
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p memory_mcp --lib platform::process_memory`
Expected: COMPILE ERROR — `parse_status` / `parse_heap_bytes` not found.

- [ ] **Step 3: Implement the reader**

`read()` composes: `parse_status(read("/proc/self/status"))`, `parse_heap_bytes(read("/proc/self/maps"))`, cgroup v2 via `/proc/self/cgroup` -> resolve the `memory.current` / `memory.swap.current` / `memory.stat` under `/sys/fs/cgroup/<path>/`, and glibc `mallinfo2()` wrapped in one `unsafe` block with a `// SAFETY:` note that it is a value-returning libc call with no preconditions. Every failure path returns `None` for that field. Gate the `mallinfo2` call behind `#[cfg(all(target_os = "linux", target_env = "gnu"))]`.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p memory_mcp --lib platform::process_memory`
Expected: PASS.

- [ ] **Step 5: Add the fields to the snapshot and set the gauges**

Add one `pub(crate) memory: crate::platform::process_memory::ProcessMemory` field to `HttpMemorySnapshot` (keep `Copy`), populate it in `capture()`, and in `record_memory_snapshot()` set each gauge only when its field is `Some`.

- [ ] **Step 6: Exposition test**

In `src/http/metrics.rs` tests, use `spawn_upkeep_with_ticks` to run one upkeep pass and assert the rendered text contains `memory_process_rss_bytes` on Linux. Gate the assertion `#[cfg(target_os = "linux")]`.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p memory_mcp --lib http::metrics`
Expected: PASS (Linux); the macOS build compiles and skips the assertion.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/platform/process_memory.rs crates/memory-mcp/src/platform.rs \
  crates/memory-mcp/src/shared/observability.rs \
  crates/memory-mcp/src/http/runtime/memory_snapshot.rs crates/memory-mcp/src/http/metrics.rs
git commit -m "feat(observability): expose process, cgroup and allocator memory gauges"
```

---

### Task 2: On-demand heap dump and glibc tunable logging

**Files:**
- Create: `crates/memory-mcp/src/control/local_admin/heap.rs` (handler + `malloc_info` wrapper)
- Modify: `crates/memory-mcp/src/control/local_admin.rs` (register `pub mod heap;`)
- Modify: `crates/memory-mcp/src/http/router.rs` (mount `GET /api/v1/admin/heap` in the existing `#[cfg(feature = "control-plane")]` admin block, beside `/api/v1/admin/session`)
- Modify: `crates/memory-mcp/src/http/config/parse.rs` (new flag `MEMORY_MCP_HTTP_HEAP_DUMP_ENABLED`, default `false`) and `types.rs`
- Test: handler tests in `heap.rs`.

**Interfaces:**
- Consumes: the admin auth extractor the other `/api/v1/admin/*` handlers use (`crate::control::local_admin::handlers`).
- Produces: `pub(crate) async fn heap_dump() -> impl IntoResponse` mounted at `GET /api/v1/admin/heap` returning `text/plain` with `malloc_info(0, stream)` output; `pub(crate) fn malloc_info_text() -> Option<String>` (glibc-only, `None` off glibc); `pub(crate) fn effective_glibc_tunables() -> Vec<(&'static str, String)>` covering `MALLOC_ARENA_MAX`, `MALLOC_MMAP_THRESHOLD_`, `MALLOC_TRIM_THRESHOLD_`, plus the process's own `mallinfo2` summary.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test]
async fn heap_dump_route_is_disabled_by_default() {
    // Router built with the flag unset must not mount the route.
    // GET /api/v1/admin/heap -> 404.
}

#[cfg(all(target_os = "linux", target_env = "gnu"))]
#[test]
fn malloc_info_text_is_present_on_glibc() {
    let text = malloc_info_text().expect("glibc has malloc_info");
    assert!(text.contains("<malloc"), "{text}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p memory_mcp --lib control::local_admin::heap`
Expected: COMPILE ERROR — module and function not found.

- [ ] **Step 3: Implement**

Implement `malloc_info_text()` with `libc::open_memstream` + `libc::malloc_info` + `fclose`, inside a single `unsafe` block documented as: `open_memstream` yields a stream we own; `malloc_info` writes to it; `fclose` flushes and frees the buffer. Gate the whole body `#[cfg(all(target_os = "linux", target_env = "gnu"))]` and return `None` otherwise. Mount the route only when the flag is set, behind the admin extractor. Log `effective_glibc_tunables()` once at startup from the HTTP binary.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p memory_mcp --lib control::local_admin::heap`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/memory-mcp/src/control/local_admin/heap.rs crates/memory-mcp/src/control/local_admin.rs \
  crates/memory-mcp/src/http/router.rs \
  crates/memory-mcp/src/http/config/parse.rs crates/memory-mcp/src/http/config/types.rs \
  crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(observability): operator-gated heap dump and glibc tunable logging"
```

---

### Task 3: Startup-phase memory markers

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/bootstrap.rs` (`build_state`)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (one marker after the router is built)
- Test: unit test for the marker helper.

**Interfaces:**
- Consumes: `crate::platform::process_memory::read()` from Task 1.
- Produces: `fn log_memory_marker(logger: &StdoutLogger, phase: &'static str)` emitting an INFO event `op=http.startup_memory` with fields `phase`, `rss_bytes`, `hwm_bytes`, `heap_bytes`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn marker_records_monotonic_peak_fields() {
    // A recording logger captures one event with phase="build_state" and the
    // same rss/hwm/heap field names the gauges use.
    let events = run_with_recording_logger(|| log_memory_marker(&logger(), "build_state"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].get("op").unwrap(), "http.startup_memory");
    assert!(events[0].contains_key("rss_bytes") && events[0].contains_key("hwm_bytes"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p memory_mcp --lib http::runtime::bootstrap::tests::marker`
Expected: COMPILE ERROR — `log_memory_marker` not found.

- [ ] **Step 3: Implement and place the markers**

Call `log_memory_marker` after each named phase of `build_state` (config resolved, registry/store connected, migrations applied, pool built, scheduler started) and once in the binary after the router is built. The markers are INFO and must not carry request data.

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test -p memory_mcp --lib http::runtime::bootstrap::tests`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/http/runtime/bootstrap.rs crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(observability): log process memory after each startup phase"
```

---

### Task 4: Surface the new gauges in the shipped dashboards

**Files:**
- Modify: the observability dashboards/recording rules the `xtask -- check-observability` command regenerates (follow that command's source of truth; do not hand-edit generated output).

**Interfaces:**
- Consumes: the gauge names from Task 1.
- Produces: a "process memory" panel row (RSS, swap, HWM, `[heap]`, cgroup anon vs file, allocator arena vs in-use vs free) and an alert on `memory_process_hwm_bytes` approaching the host RAM.

- [ ] **Step 1: Add the panels/rules to the generator input**

- [ ] **Step 2: Run the checker**

Run: `cargo run -p xtask -- check-observability`
Expected: exit 0; the regenerated dashboards contain the new series and the check passes.

- [ ] **Step 3: Commit**

```bash
git add observability/
git commit -m "feat(observability): dashboard and alerts for process memory"
```

---

## Phase 1 — hypothesis backlog (promote to tasks after Task 1-4 data)

Do not implement these before Task 1 exposes the allocator gauges: each states the one measurement that decides it. Order is by expected value per unit effort.

| # | Hypothesis | The measurement that decides it | Candidate fix |
|---|---|---|---|
| H1 | glibc **arena retention**: the 1.14 GB `[heap]` is mostly free chunks the allocator keeps, not live objects | `memory_alloc_free_bytes` (mallinfo2 `fordblks`) >> `memory_alloc_in_use_bytes` (`uordblks`), and `keepcost` is large | periodic `malloc_trim(0)` on a timer, and/or `MALLOC_TRIM_THRESHOLD_` / a fixed `MALLOC_MMAP_THRESHOLD_`; A/B against no-op |
| H2 | **Startup peak** (~1.2 GB `VmHWM`) is an eager-init artifact, not steady state | the Task 3 markers show which phase owns the peak | make the owning subsystem lazy/bounded |
| H3 | A **transient large buffer** (a query result, an embedding batch, a body) drives the heap up once and the allocator keeps it | `malloc_info` before/after a single workload request | bound the buffer; chunk or stream it |
| H4 | **Allocator choice** | A/B `--features mimalloc` on this exact dataset/config under the sampler | adopt the winner for Linux only if the A/B wins |
| H5 | Footprint scales with **concurrency/buffer limits** (`POOL_CAP`, `GLOBAL_REQUEST_LIMIT`, `SUBSCRIPTION_LIMIT`, `MAINTENANCE_PARALLELISM`, `PREFLIGHT_BYTES`) | ramp each limit and watch `memory_alloc_*` | lazy per-request buffers instead of pre-sized |
| H6 | **Idle tenants retain runtimes** | `tenant_runtime_count` vs `RUNTIME_IDLE_TTL_SECS` under a real request then idle | free caches/pools on eviction |

Only H1-H6 evidence, gathered from staging or a disposable host via `cargo run -p xtask -- sample-memory`, can support a RAM-reduction claim; today's idle numbers cannot (see `HTTP_MEMORY_BASELINE.md`).

## Operational hygiene (done in this session)

- **Removed the published SurrealDB port.** `docker-compose.yml` no longer publishes `8000:8000`; `memory_mcp` reaches SurrealDB over the compose network. Applied to the running host (`/opt/memory_mcp/docker-compose.yaml`, backup kept) and to this repo.
- **Still open, not a memory fix:** `/opt/memory_mcp/.env` is mode `0644` (world-readable) and holds DB passwords, session/CSRF keys, the API-key pepper and the embeddings API key. Tighten to `0600` and rotate the exposed secrets. (No value is reproduced here.)
