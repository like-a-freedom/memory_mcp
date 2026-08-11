# Architecture Hardening Plan — 2026-07-30

> Implements the architecture review at `target/arch-review/architecture-review-2026-07-30.html`.
> Quality baseline: benchmark v5 (2026-07-29) — 17/17 gates green across PR/Release/Nightly,
> 363/363 cases passed. Every stage below preserves those gates.

## New ADRs

- **ADR-0023** — Typed command descriptors for app command dispatch (card 1)
- **ADR-0024** — Complete the DbClient capability narrowing (card 2)
- **ADR-0025** — Single formula home for evaluation metrics (card 3)

ADR-0022 (compact responses) is already Draft — card 5 closes its Phase B.

## Cards, order, and dependencies

| # | Candidate | ADR | Write-scope | Depends on |
|---|-----------|-----|-------------|------------|
| 5 | Wire or delete dangling ends | — | `claims/normalize.rs`, `service/claims.rs`, `evals/profiles/response_size.json`, `crates/eval-harness/src/suites/response_size.rs` | — |
| 2 | Complete DbClient narrowing | 0024 | `storage/client.rs`, `storage/claims.rs`, `service/mock_db.rs`, service consumers | — |
| 1 | Typed command dispatch | 0023 | `mcp/handlers.rs`, new `service/apps/dispatch.rs` | — |
| 3 | Single formula home for metrics | 0025 | `crates/eval-harness/src/{reducer.rs, metrics.rs, suites/*_metrics sites}` | — |
| 6 | Locality fixes | — | `service/core/builder.rs`, `models/request.rs`, `crates/memory-mcp/tests/fixtures/evals/` | — |
| 4 | MemoryService composition-root | completes 2026-07-23-capability-seam-completion plan | `service/core.rs`, `service/core/builder.rs`, `service/apps/graph.rs` | Card 2's mocks + narrow stores |

**Run order:** Card 5 → Card 2 → Cards 1, 3, 6 in parallel (disjoint write-sets) → Card 4 last.

## Stage gates (each card)

- `cargo build` — green
- `cargo test -p <affected-crate>` — green
- `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` — zero warnings
- `cargo fmt --all --check` — zero diff
- Benchmark non-regression: run the eval profile(s) whose gates cover the card's blast radius, compare observed values to v5.

## Execution status (2026-07-30)

| Card | Status | Notes |
|------|--------|-------|
| 5 | ✅ complete | ADR-0022 Accepted on Phase B data (39.5% mean reduction, 66/66) |
| 2 | ✅ complete | Six commits land the capability narrowing: stores in `storage/{context_store,app_store,fact_store,episode_store}.rs`; PR gates at v5 parity across every step |
| 6 | ✅ partial | Builders moved to `models/request.rs`. Eval corpora move deliberately deferred: ADR-0020 pins digests on the current path; moving requires re-pinning across all profiles |
| 1 | ✅ complete | ADR-0023 Accepted (2026-07-30); `service/apps/dispatch.rs` ships the `AppCommandDescriptor` table, `find_descriptor` dispatcher, and all `execute_*` handlers; no `unreachable!()` remains in `mcp/handlers.rs`. Verified by audit 2026-08-11 |
| 3 | ⏸  planned | ADR-0025 defines the renderer; implementation touches every suite's evidence path across eval-harness (one follow-on PR) |
| 4 | ✅ complete | Composition-root end-state reached: capability seam fully migrated (tools call `*Capability::*` via `&ServiceContext`), `find_intro_chain` in `apps/graph.rs`, no one-line delegates on `MemoryService`; `core.rs` test module split into owning modules (startup / fact / value_helpers / models::access). Verified by audit 2026-08-11 (PR profile 7/7, 119/119) |

Card 2 verified end-to-end: `cargo test --workspace --all-targets --features cli-watch,mcp-apps` passes; PR profile matches v5 observed values for every gate (7/7, 119/119).

## Card 5 details (executed now)

1. **Delete `service/claims/normalize.rs`** — 1-line stub module. Remove the file and the `pub(crate) mod normalize;` line in `service/claims.rs`.
2. **Create `crates/eval-harness/src/suites/response_size.rs`** — a measurement-only suite (no gates) that measures byte size of `assemble_context` and `explain` responses under `compact=true` vs `compact=false` from the existing lifecycle/extraction fixtures, and emits metric evidence `bytes_total` per mode per tool.
3. **Create `evals/profiles/response_size.json`** — profile selecting the suite; no gate entries.
4. **Register the suite** in `crates/eval-harness/src/suites.rs` and the suite-id match in `crates/eval-harness/src/main.rs`.
5. **Mark ADR-0022 Accepted** — its remaining validation item was Phase B data; once the profile exists and runs, edit status to `Accepted (2026-07-30)` and link to this plan.
6. Keep `downstream_qa.rs` — ADR-0019 explicitly defers it until its model/prompt/pinning is done; it is intentionally-parked diagnostic code, not drift. Add `// PINNED-DEFERRED — see ADR-0019` comment so future audits don't re-flag it.

## Card 2 details (queued next)

- Extract `create/update/select_one/select_table/query/apply_migrations` as the *core* `DbClient`.
- `SurrealClaimStore` is the reference shape; replicate it for Context, App, Fact, Embedding domains.
- Collapse the trait-over-trait forwarding in `storage/client.rs`.
- Shrink `MockDbClient` to the core surface; move per-capability test doubles next to the capability's tests.
- Card gate: PR + Release profiles.

## Card 1 details (parallel with 3 and 6)

- New module `service/apps/dispatch.rs` (started by the ADR in `docs/adr/0023-typed-command-descriptors-for-app-command.md`).
- Command table is data — add/remove commands by touching one row; handler dispatch is a fixed ~50 lines.
- Remove all production `unreachable!()` in `handlers.rs`.
- Card gate: `cargo test -p memory_mcp`; lifecycle Release gates.

## Card 3 details (parallel with 1 and 6)

- Introduce renderer for per-case diagnostics from `MetricEvidence`.
- Remove `metric_map.insert("<string>", <float>)` construction from suites.
- Key naming moves into `metrics.rs`.
- Card gate: full PR + Release + Nightly artifact values must match v5 exactly.

## Card 6 details (parallel with 1 and 3)

- Move the three request builders (`IngestRequestBuilder`, `InvalidateRequestBuilder`, `AssembleContextRequestBuilder`) to `models/request.rs`.
- Move `crates/memory-mcp/tests/fixtures/evals/` to `evals/fixtures/`; re-run a profile against the new path to prove ADR-0020 manifest digests resolve.

## Card 4 details (after card 2)

- Remove `ingest`/`explain`/`extract`/`resolve`/`assemble_context` one-line delegates from `MemoryService`; `tools/*.rs` call their capability directly.
- Relocate BFS `find_intro_chain` into `service/apps/graph.rs` alongside graph expansion.
- `MemoryService` keeps only: construction (`builder.rs`), worker lifecycle, logging shapers that *are* the service.
- Split `core.rs`'s 1335-line test module into per-capability test files under their modules.
- Card gate: PR + Release + Nightly profiles; pipeline benches ingest/extract within v5 noise band.

## Followups (non-blocking)

- Append one paragraph to ADR-0001 naming the end-state reached in ADR-0024.
- Update CONTEXT.md if the `service/` seam bullets change after cards 2 and 4.
