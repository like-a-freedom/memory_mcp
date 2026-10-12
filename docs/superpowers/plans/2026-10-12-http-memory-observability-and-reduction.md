# HTTP-profile Memory: Observability & Footprint Reduction Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the HTTP profile's ~1.25 GB footprint attributable by exporting the one class of signal no collector can see — the glibc allocator's own accounting and the `[heap]` extent — and by logging process memory after each startup phase; then test and land the reduction hypotheses in Phase 1 from evidence rather than guesses.

**Architecture:** A new `src/platform/process_memory.rs` reads glibc `mallinfo2()` and `/proc/self/maps` (Linux/glibc only, every other field `None`). The six allocator gauges ride the existing five-second upkeep tick through the same `HttpMemorySnapshotSource` seam that already carries the HTTP owner gauges, and are declared in the `shared/observability.rs` registry so `describe_metrics()` describes them. Startup-phase memory markers are **structured logs**, not gauges. Phase 1 is a hypothesis backlog, promoted to tasks only after Phase 0 shows where the bytes are.

**Tech Stack:** Rust 2021, `metrics`/`metrics-exporter-prometheus`, glibc (`mallinfo2`, `malloc_info`), Linux `/proc`, cgroup v2, tokio.

**Spec:** this deployment's own evidence, not a design doc: `/opt/memory_mcp/memory-consumption.md` (operator's 2026-10-07 map of the footprint), `docs/performance/HTTP_MEMORY_BASELINE.md` (measurement protocol and its BLOCKED gates), `docs/performance/MEMORY_PROFILE.md` (allocator matrix), and — binding — `observability/README.md` section "Deliberate gaps" (the "No process metrics" rule this plan amends, deliberately and in writing).

## Global Constraints

- Workspace `rust-version` 1.99.0; no toolchain, dependency, `Cargo.toml`, Dockerfile or CI-pin changes.
- No new MCP tools (8-tool surface frozen). New HTTP routes are allowed only when operator-gated and default-off.
- **The "No process metrics" rule stands for RSS/CPU/uptime and for cgroup counters.** This plan does NOT re-export them: cAdvisor already reports `container_memory_rss`, `container_memory_swap`, `container_memory_working_set_bytes` and `container_memory_max_usage_bytes` for `memory_mcp` into the same VictoriaMetrics store (`job="ionos_eu_vps_cadvisor"`), and duplicating them forks the contract. The **only** new families are the allocator's own accounting and the `[heap]` extent, which no collector can see. `observability/README.md` must gain this carve-out explicitly, with the reason.
- New families are declared in `src/shared/observability.rs` as a `pub const METRIC_*` **and** a `MetricDescription { name, kind: MetricKind::Gauge, unit: None, help }` entry appended to the `DESCRIPTIONS` registry array; `describe_metrics()` in `src/observability.rs` consumes that array and needs no edit. A family with no `DESCRIPTIONS` entry is a defect.
- Telemetry reads only `/proc/self/*` and glibc; it must never read another process's memory, request bodies, credentials, or log raw memory content. Labels stay bounded (ADR-0005, ADR-0048); anything per-item belongs in the log.
- Off Linux/glibc every read returns `None`; a `None` is never recorded as `0` (a false "0 bytes" is worse than a missing series). The local dev profile is macOS.
- The five-second upkeep worker already exists (`src/http/metrics.rs::spawn_upkeep`); do not add a second timer.
- Diagnostics overhead is gated: `HTTP_MEMORY_BASELINE.md` allows **<=5 MiB footprint delta and <=5% CPU delta, constant series count per added family**. Task 5 measures it; the sampler `cargo run -p xtask -- sample-memory` already exists.
- Tests: narrow first — `cargo test -p memory_mcp --lib <filter>`; the parsers are unit-tested against string fixtures, never against the live host.
- Before shipping: `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` (zero warnings), and `cargo run -p xtask -- check-observability` (which regenerates the dashboards and runs the `observability/*.py` contract checks).
- Do NOT add a container `mem_limit` as a fix for the footprint; the operator has rejected a cap as the solution. Revisit only after Phase 1 lands.

### Measured baseline (2026-10-12, image `v1.33.0`, idle)

| Signal | Value | Source |
|---|---|---|
| `[heap]` extent | 1 144 MB VSZ, ~400-500 MB resident | `/proc/<pid>/maps` `[heap]` |
| `VmData` / `VmHWM` | 1 221 MB / 1 216 MB | `/proc/<pid>/status` |
| `VmRSS` / `VmSwap` | ~437-477 MB / ~780-820 MB | `/proc/<pid>/status` |
| cgroup `anon` / `current` | 448 MB / 481 MB | cgroup v2 |
| container RSS / swap / max | ~755-780 MB (old build) -> ~477-509 MB (new) | cAdvisor, already scraped |
| App owner gauges (caches, runtimes, reservations) | **all 0** | `/memory/metrics` |
| Data size / threads / NER | ~1 MB / 3 / `anno` (no model, embeddings remote) | prod / code |

The whole footprint is anonymous glibc heap over ~1 MB of data. Static reading of the crate found no large up-front allocation, and the app's own accounting reports zero — which is exactly the gap Phase 0 closes.

## Review Focus

Input classes and failure modes the spec implies but the task tests do not exercise — each is pinned by the named task test:

1. **Non-Linux / non-glibc** — on macOS the allocator read yields all `None` and no gauge is emitted; a `None` must never become `0` — Task 1 unit `maps_without_heap_is_none_not_zero`.
2. **Malformed `/proc`** — a `maps` file where the `[heap]` line is absent yields `None` for `heap_bytes` only, never a panic, never a `0` and never a whole-snapshot error — Task 1 unit `maps_without_heap_is_none_not_zero`. (No cgroup is read: the binding carve-out keeps cgroup counters with cAdvisor.)
3. **Exposition contract unchanged for process metrics** — the rendered `/metrics` must still contain no `memory_process_rss`/`memory_process_cpu`/`memory_process_uptime` family, and `observability/README.md` must document the allocator carve-out — Task 4 test `check-observability` plus the amended README.
4. **Heap route safety** — the on-demand `malloc_info` dump is refused (404) unless explicitly enabled, and never echoes request data — Task 2 test `heap_dump_route_is_disabled_by_default`.
5. **Scrape path under a base path** — a deployment with `base_path=/memory` serves the exposition at `/memory/metrics`; the shipped scrape config must match or every new panel is empty — Task 5 test/check `scrape_path_matches_base_path`.
6. **Peak vs steady** — the startup markers must show the 1.2 GB peak as a boot-time event, not steady state — Task 3 test `startup_markers_report_phase_and_peak`.

---

## Phase 0 — make the footprint attributable

### Task 1: Allocator-attribution gauges in `/metrics`

**Files:**
- Create: `crates/memory-mcp/src/platform/process_memory.rs` (the allocator / `[heap]` reader + parsers)
- Modify: `crates/memory-mcp/src/platform.rs` (register `pub mod process_memory;`)
- Modify: `crates/memory-mcp/src/shared/observability.rs` (6 consts + 6 `MetricDescription` entries)
- Modify: `crates/memory-mcp/src/observability.rs` (add the six names to the `HTTP_OWNER_SNAPSHOT` allow-list inside `every_declared_family_reaches_the_exposition`; the family is written only by upkeep, exactly like the eight already listed there, so without the entry that test fails on a declared-but-unrecorded family)
- Modify: `crates/memory-mcp/src/http/runtime/memory_snapshot.rs` (`HttpMemorySnapshot` gains an `alloc` field + `capture()` populates it)
- Modify: `crates/memory-mcp/src/http/metrics.rs` (`record_memory_snapshot()` sets the gauges)
- Test: unit tests in `process_memory.rs`; one exposition test via the existing `spawn_upkeep_with_ticks` fixture.

**Interfaces:**
- Consumes: nothing from other tasks; the existing `HttpMemorySnapshotSource::capture()` call site in `spawn_upkeep`.
- Produces: `pub(crate) struct AllocSnapshot { pub(crate) arena_bytes: Option<u64>, pub(crate) mmap_bytes: Option<u64>, pub(crate) in_use_bytes: Option<u64>, pub(crate) free_bytes: Option<u64>, pub(crate) keepcost_bytes: Option<u64>, pub(crate) heap_bytes: Option<u64> }` and `pub(crate) fn read_alloc() -> AllocSnapshot`. Read-only; every field optional; never panics. `ProcessTotals` / `read_process_totals()` are **not** produced here: their only consumer is Task 3's logs, and a `pub(crate)` item with no caller in this commit is dead code under the zero-warning gate (`-D warnings`). Task 3 produces them in the same file.

New constants + registry entries (`kind: MetricKind::Gauge`, `unit: None`): `memory_alloc_arena_bytes` (`mallinfo2.arena`), `memory_alloc_mmap_bytes` (`hblkhd`), `memory_alloc_in_use_bytes` (`uordblks`), `memory_alloc_free_bytes` (`fordblks`), `memory_alloc_keepcost_bytes` (`keepcost`), `memory_alloc_heap_bytes` (the `[heap]` mapping size from `/proc/self/maps`).

- [ ] **Step 1: Write the failing parser unit tests**

In `process_memory.rs`, test against string fixtures (no live host):

```rust
#[test]
fn finds_heap_extent_in_maps() {
    // A well-formed maps range whose length is 0x100 pages: the parser reads
    // `end - start`, and the fixture's extent is what the assertion pins.
    let maps = "7f0000000000-7f0000100000 rw-p 00000000 00:00 0 [heap]\n";
    assert_eq!(parse_heap_bytes(maps), Some(0x100 * 4096));
}

#[test]
fn maps_without_heap_is_none_not_zero() {
    assert_eq!(
        parse_heap_bytes("7f0100000000-7f0100010000 r-xp 00000000 00:00 0 /usr/lib/libc.so.6\n"),
        None
    );
}

```

There is no cgroup parser: the binding spec's "No process metrics" carve-out keeps cgroup counters with cAdvisor, and no code in this plan reads a cgroup, so a `resolve_cgroup_memory_dir` helper would be an uncalled `pub(crate)` function — dead code under `-D warnings`, and a direct test of a private helper, which the testing rules forbid. The robustness this task must show is the parser's: a `maps` file with no `[heap]` line yields `None`, never `0`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p memory_mcp --lib platform::process_memory`
Expected: COMPILE ERROR — `parse_heap_bytes` not found.

- [ ] **Step 3: Implement the reader**

`read_alloc()` calls glibc `mallinfo2()` behind `#[cfg(all(target_os = "linux", target_env = "gnu"))]`, plus `parse_heap_bytes(read("/proc/self/maps"))`. Off Linux/glibc every field is `None`. Declare the C symbols locally — `#[repr(C)] struct Mallinfo2 { arena, ordblks, smblks, hblks, hblkhd, usmblks, fsmblks, uordblks, fordblks, keepcost: usize }` and `unsafe extern "C" { fn mallinfo2() -> Mallinfo2; }` — and use `std::os::raw` types for the Task 2 shim. **No `libc` crate and no `Cargo.toml` change**: `libc` is not a direct dependency of this crate and adding one needs operator approval (AGENTS.md). Host glibc is 2.41, past `mallinfo2`'s 2.33 floor. Wrap the call in exactly one `unsafe` block with a `// SAFETY:` note.

- [ ] **Step 4: Run the unit tests to verify they pass**

Run: `cargo test -p memory_mcp --lib platform::process_memory`
Expected: PASS.

- [ ] **Step 5: Declare the families and set the gauges**

Add the six consts and their `MetricDescription` entries to `DESCRIPTIONS` in `shared/observability.rs` (gauges carry `unit: None`; `counters_and_gauges_are_dimensionless` pins that rule, so a unit here fails the build). Add the six names to the `HTTP_OWNER_SNAPSHOT` allow-list in `src/observability.rs::tests::every_declared_family_reaches_the_exposition`, which otherwise fails on a declared-but-unrecorded family. Add an `alloc: AllocSnapshot` field to `HttpMemorySnapshot` (keep `Copy`), populate it in `capture()`, and in `record_memory_snapshot()` set each gauge only when its field is `Some`.

- [ ] **Step 6: Exposition test**

In `src/http/metrics.rs` tests, run one `spawn_upkeep_with_ticks` pass and assert the rendered text contains `memory_alloc_arena_bytes` and does **not** contain `memory_process_rss` (the contract). Gate on `#[cfg(target_os = "linux")]`.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p memory_mcp --lib http::metrics && cargo test -p memory_mcp --lib shared::observability`
Expected: PASS on Linux; the macOS build compiles and skips the gauge assertion.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/platform/process_memory.rs crates/memory-mcp/src/platform.rs \
  crates/memory-mcp/src/shared/observability.rs \
  crates/memory-mcp/src/http/runtime/memory_snapshot.rs crates/memory-mcp/src/http/metrics.rs
git commit -m "feat(observability): export glibc allocator attribution gauges"
```

---

### Task 2: On-demand heap dump and glibc tunable logging

**Files:**
- Create: `crates/memory-mcp/src/control/local_admin/heap.rs` (handler + `malloc_info` wrapper)
- Modify: `crates/memory-mcp/src/control/local_admin.rs` (register `pub mod heap;`)
- Modify: `crates/memory-mcp/src/http/router.rs` (mount `GET /api/v1/admin/heap` in the existing `#[cfg(feature = "control-plane")]` admin block, beside `/api/v1/admin/session`)
- Modify: `crates/memory-mcp/src/http/config/parse.rs` + `types.rs` (flag `MEMORY_MCP_HTTP_HEAP_DUMP_ENABLED`, default `false`)
- Test: handler tests in `heap.rs`.

**Interfaces:**
- Consumes: the admin auth extractor the other `/api/v1/admin/*` handlers use (`crate::control::local_admin::handlers`).
- Produces: `pub(crate) async fn heap_dump() -> impl IntoResponse` at `GET /api/v1/admin/heap`, returning `text/plain` `malloc_info(0, stream)` output; `pub(crate) fn malloc_info_text() -> Option<String>` (glibc-only, `None` elsewhere); `pub(crate) fn effective_glibc_tunables() -> Vec<(&'static str, String)>` for `MALLOC_ARENA_MAX`, `MALLOC_MMAP_THRESHOLD_`, `MALLOC_TRIM_THRESHOLD_` plus the process's `mallinfo2` summary.

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

`malloc_info_text()` declares `unsafe extern "C" { fn open_memstream(buf: *mut *mut c_char, size: *mut usize) -> *mut c_void; fn malloc_info(options: c_int, stream: *mut c_void) -> c_int; fn fclose(stream: *mut c_void) -> c_int; }` (the `FILE` handle is opaque, passed as `*mut c_void`) and uses them in one `unsafe` block documented as: `open_memstream` yields a stream we own, `malloc_info` writes to it, `fclose` flushes and frees the buffer. Gate the body on Linux/glibc and return `None` otherwise. No `libc` crate. Mount the route only when the flag is set, behind the admin extractor. The handler must carry a doc comment stating it is expensive (it walks the whole heap) and operator-only. Log `effective_glibc_tunables()` once at startup from the HTTP binary.

- [ ] **Step 4: Run them to verify they pass**

Run: `cargo test -p memory_mcp --lib control::local_admin::heap`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/memory-mcp/src/control/local_admin/heap.rs crates/memory-mcp/src/control/local_admin.rs \
  crates/memory-mcp/src/http/router.rs \
  crates/memory-mcp/src/http/config/parse.rs crates/memory-mcp/src/http/config/types.rs \
  crates/memory-mcp/src/bin/memory_mcp_http.rs
git commit -m "feat(observability): operator-gated heap dump and glibc tunable logging"
```

---

### Task 3: Startup-phase memory markers (logs)

**Files:**
- Modify: `crates/memory-mcp/src/http/runtime/bootstrap.rs` (`build_state`)
- Modify: `crates/memory-mcp/src/bin/memory_mcp_http.rs` (one marker after the router is built)
- Modify: `crates/memory-mcp/src/platform/process_memory.rs` (add `ProcessTotals { rss_bytes, swap_bytes, hwm_bytes }` and `read_process_totals()` — produced here, not in Task 1, because its only caller is this marker and an uncalled function is dead code under `-D warnings`)
- Test: unit test for the marker helper.

**Interfaces:**
- Consumes: `crate::platform::process_memory::read_alloc` from Task 1; produces `read_process_totals` in the same file for its own use.
- Produces: `fn log_memory_marker(logger: &StdoutLogger, phase: &'static str)` emitting an INFO event `op=http.startup_memory` with `phase`, `rss_bytes`, `hwm_bytes`, `heap_bytes`. **Logs only — no metric family.**

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn startup_markers_report_phase_and_peak() {
    let events = run_with_recording_logger(|| log_memory_marker(&logger(), "build_state"));
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].get("op").unwrap(), "http.startup_memory");
    assert_eq!(events[0].get("phase").unwrap(), "build_state");
    assert!(events[0].contains_key("rss_bytes") && events[0].contains_key("hwm_bytes"));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p memory_mcp --lib http::runtime::bootstrap::tests::startup_markers`
Expected: COMPILE ERROR — `log_memory_marker` not found.

- [ ] **Step 3: Implement and place the markers**

Call `log_memory_marker` after each named phase of `build_state` (config resolved, store connected, migrations applied, pool built, scheduler started) and once in the binary after the router is built. INFO, no request data.

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

### Task 4: Amended contract, dashboards and recording rules

The dashboards are **generated**: `observability/build_dashboards.py` writes `dashboards/technical.json` and `dashboards/product.json`, and `check_dashboards.py` / `check_rules.py` / `check_alerts.py` / `tests/test_external_metrics.py` validate them. Never hand-edit the generated JSON.

**Files:**
- Modify: `observability/README.md` (amend "Deliberate gaps" — the "No process metrics" rule gains the allocator carve-out and its reason)
- Modify: `observability/build_dashboards.py` (a new "Allocator attribution" row; update the interpretation bullet that repeats "No process metrics")
- Modify: `observability/tests/test_external_metrics.py` (pin the new row's titles and the six expressions, mirroring the existing `HTTP memory diagnostics` contract test)
- Modify: `observability/recording_rules.yml` and `observability/alerts.yml` (a recording rule for the `heap`/`in-use` ratio and an alert only if the review agrees one is warranted; a rule with no reader is a defect)

**Interfaces:**
- Consumes: the six gauge names from Task 1.
- Produces: a collapsed "Allocator attribution" row with panels for arena vs in-use vs free, mmap, keepcost and `[heap]`, wired to `memory_alloc_*`; the README carve-out.

- [ ] **Step 1: Amend the README and the generator**

Add the carve-out text next to the existing "No process metrics" paragraph; add the row to `build_dashboards.py`; update the interpretation bullet.

- [ ] **Step 2: Pin the contract test**

Add assertions next to `test_technical_dashboard_contains_http_memory_diagnostics` for the new row's titles and the six `memory_alloc_*` expressions.

- [ ] **Step 3: Run the checker**

Run: `cargo run -p xtask -- check-observability`
Expected: exit 0 — `build_dashboards.py` regenerates the JSON, and all four checkers plus the regression suite pass.

- [ ] **Step 4: Commit**

```bash
git add observability/
git commit -m "feat(observability): allocator-attribution panels and contract carve-out"
```

---

### Task 5: Scrape wiring, base-path reconciliation and overhead measurement

**Files:**
- Modify: `observability/vmagent/vmagent.yml` and `observability/prometheus.yml` (make `metrics_path` match a base-path deployment; state it, do not inherit)
- Modify: `observability/README.md` (a short note: prod runs no app scrape today)
- Test: a check that the configured `metrics_path` equals the router's mounted path for the configured base path.

**Interfaces:**
- Consumes: `HttpConfig::base_path` (the router nests `/metrics` under it, `src/http/router.rs` L56 + L379).
- Produces: a scrape config whose `metrics_path` is `${base_path}/metrics`, and a note that the exposition of a `base_path=/memory` deployment lives at `/memory/metrics` (verified on prod: `/metrics`->404, `/memory/metrics`->200).

- [ ] **Step 1: Write the failing check**

A test (or a `check_dashboards.py`-style assertion) that fails when the scrape `metrics_path` disagrees with `base_path`:
`scrape_path_matches_base_path` — with `base_path="/memory"` the expected path is `/memory/metrics`.

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p memory_mcp --lib <the new check>`
Expected: FAIL — the shipped config says `/metrics`.

- [ ] **Step 3: Fix the config and document the prod gap**

Set `metrics_path` to match the base path (or document the collector-side prefix strip where the deployment does that). Add the README note.

- [ ] **Step 4: Measure the diagnostics overhead**

On a staging/disposable host with the sampler, run three paired INFO samples and record delta footprint (gate <=5 MiB) and delta CPU (gate <=5%) with the new upkeep read enabled vs disabled; run it once with the allocator read stubbed to `None` as the control. Record the numbers in `docs/performance/`.

- [ ] **Step 5: Commit**

```bash
git add observability/ docs/performance/
git commit -m "fix(observability): match the scrape path to the base path; record overhead"
```

---

## Phase 1 — hypothesis backlog (promote to tasks after Task 1-5 data)

Do not implement these before Task 1 exposes `memory_alloc_*`: each names the one measurement that decides it. Order is by expected value per unit effort.

| # | Hypothesis | The measurement that decides it | Candidate fix |
|---|---|---|---|
| H1 | glibc **arena retention**: the 1.14 GB `[heap]` is mostly free chunks the allocator keeps, not live objects | `memory_alloc_free_bytes` >> `memory_alloc_in_use_bytes` and `keepcost` is large | periodic `malloc_trim(0)`, and/or `MALLOC_TRIM_THRESHOLD_` / a fixed `MALLOC_MMAP_THRESHOLD_`; A/B against no-op |
| H2 | **Startup peak** (~1.2 GB `VmHWM`) is an eager-init artifact | the Task 3 `http.startup_memory` markers show which phase owns the peak | make the owning subsystem lazy/bounded |
| H3 | A **transient large buffer** (query result, embedding batch, body) drives the heap up once and glibc keeps it | `memory_alloc_*` before/after one workload request via the Task 2 dump | bound the buffer; chunk or stream it |
| H4 | **Allocator choice** | A/B `--features mimalloc` on this exact dataset/config under the sampler | adopt for Linux only if the A/B wins |
| H5 | Footprint scales with **concurrency/buffer limits** (`POOL_CAP`, `GLOBAL_REQUEST_LIMIT`, `SUBSCRIPTION_LIMIT`, `MAINTENANCE_PARALLELISM`, `PREFLIGHT_BYTES`=64 MiB default) | ramp each limit and watch `memory_alloc_*` | lazy per-request buffers instead of pre-sized |
| H6 | **Idle tenants retain runtimes** | `memory_http_tenant_runtime_count` vs `RUNTIME_IDLE_TTL_SECS` under a real request then idle | free caches/pools on eviction |

Only H1-H6 evidence from staging (or a disposable host) via `cargo run -p xtask -- sample-memory` can support a RAM-reduction claim; today's idle numbers cannot (see `HTTP_MEMORY_BASELINE.md`).

## Operational hygiene (done in this session)

- **Removed the published SurrealDB port.** `docker-compose.yml` no longer publishes `8000:8000`; `memory_mcp` reaches SurrealDB over the compose network. Applied to the running host (`/opt/memory_mcp/docker-compose.yaml`, backup kept) and to this repo.
- **Still open, not a memory fix:** `/opt/memory_mcp/.env` is mode `0644` (world-readable) and holds DB passwords, session/CSRF keys, the API-key pepper and the embeddings API key. Tighten to `0600` and rotate the exposed secrets. (No value is reproduced here.)
