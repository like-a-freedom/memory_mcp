# Architecture Audit Follow-up — 2026-08-11

> Status: Complete (2026-08-11)
> Parent: Architecture audit (`/improve-codebase-architecture`) + `/grill-with-docs`
> Quality baseline: eval PR profile 7/7 gates, 119/119 cases — observed values at v5 parity

## Context

Audit of `memory_mcp` against four goals: (1) maintainability and testability,
(2) best engineering/architectural practices and system design, (3) DRY, KISS,
YAGNI, DDD, and (4) no non-wired or dangling functionality (checked against
`docs/superpowers/plans/`). All findings below are resolved; the working tree
passes every quality gate listed under Verification.

## Findings and resolutions

| ID | Severity | Finding | Resolution |
|----|----------|---------|------------|
| F1 | Major | `LlmEntityExtractor` docstring lied: claimed an `ENTITY_EXTRACTOR=llm` config flag and a graceful empty-list fallback that don't exist. The type itself is an intentional, ADR-0029-backed code-injected extension seam — not dead code | Docstring corrected: no config flag, no `NerExtractorKind::Llm` variant, errors propagate, ADR-0029 referenced. Type + test preserved |
| F2 | Minor | 8 one-shot CLI arms duplicated service-build + error-mapping boilerplate; `mode_label` was a parallel 12-arm match at risk of drift | `Command::mode_label()` (single label source) + `Command::into_one_shot()` (erased-runner table) in `cli.rs`; `runner.rs` dispatch collapsed to 4 service-mode arms + one `run_one_shot` helper. Exhaustiveness keeps both matches in sync at compile time |
| F3 | Major | `reembed_all_facts` was a 545-line orchestrator (maintainability blocker) | Split into thin coordinator (76 lines) + `prepare_reembed_pass`, `process_reembed_pass`, `process_reembed_batch`, `persist_interrupted` (dedupes two identical cancel blocks), `finalize_reembed_pass`; module docstring added. All 30 reembed tests pass unchanged |
| F4 | Minor | Plan `2026-07-30-architecture-hardening.md` Card 1 still marked blocked though fully implemented | Row flipped to ✅ with verification note (ADR-0023 Accepted, `dispatch.rs` shipped, no `unreachable!()`); Card 4's now-false "blocked on 1" note updated to "not started, blockers cleared" |
| F5 | Minor | DDD seam check (`service/` boundary, `main.rs` thin) | No action — verified clean |
| F6 | Minor | DRY check on error mapping / response helpers | No action — verified clean (`cli_error_json`/`report_cli_error` single-sourced; F2 folded the remaining copies) |
| F7 | — | Subsumed by F1 (dangling-end reframe) | Resolved with F1 |
| F8 | Nit | Duplicate `#[cfg(feature = "mcp-apps")]` on `create_test_entity` | Duplicate removed |
| F9 | Nit | YAGNI check on `CaptureReasonCode` | No action — all 15 variants exercised |

### Goal-4 sweep beyond the findings

- **Lifecycle wiring inventory** (plan `2026-07-23-lifecycle-wiring-completion.md`):
  all "built but never called" items verified wired — `capture_lifecycle_event` /
  `recall_lifecycle_event` live on `MemoryService` and are invoked by the
  `LifecycleCapture` / `LifecycleRecall` CLI commands; `LEGACY_EMBEDDING_SAMPLE_SIZE`
  no longer exists. `run_agent_memory_lifecycle_baseline` remains a `panic!` stub
  but is formally deferred by ADR-0017 (documented, not drift).
- **`model_loader` dead code** (all `#[allow(dead_code)]`-flagged, unreachable):
  deleted `ensure_gliner_model_cached` (+ its `GLINER_*` consts) — superseded by
  the ADR-0036 `NerArtifactStore` used in `gliner::build`; deleted the redundant
  `is_model_cached` wrapper (production uses `is_model_cached_with_files`; tests
  rewired onto it, coverage preserved); deleted `sanitize_model_name` (unreferenced);
  deleted the now-purposeless `download` module (folded `log_message` into
  `model_loader.rs`). Net −84 lines, zero behavior change.
- **`try_parse_json_scalars`** (`claims/structural.rs`): wired into the production
  claim path but an always-`None` stub. No plan/ADR mandates the JSON-scalar
  format, and deleting it would remove ADR-0004's documented Priority-1 slot —
  so the deferral is now explicitly documented (falls through to key-value /
  sentence patterns). Feature gap, not drift.
- **Remaining `#[allow(dead_code)]` sites** (`tool_router`, `claim_service`,
  telemetry reserved variants, gliner `run_forward`/`max_concurrency`, config-gated
  observability stubs, test helpers): all verified wired or explicitly documented.

## Verification

```bash
cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings  # zero warnings
cargo fmt --all --check                                                                       # zero diff
cargo test --workspace --all-targets --features cli-watch,mcp-apps                            # 44/44 binaries ok
make eval-pr                                                                                  # 7/7 gates, 119/119, PASSED (v5 parity)
```

CLI smoke tests: help surface unchanged; `init` / `resolve` / `invalidate` exercise
the collapsed dispatch path; error envelope (`kind`/`exit_code`) and JSON output
identical to before.

## Constraints honored

- No new ADRs (all changes were localized edits, not design decisions).
- No new dependencies; frozen surfaces untouched (8 MCP tools, 12 CLI commands).
- No `unwrap()`/`expect()` introduced in production code.
- Migration files, `LlmEntityExtractor`, and the bi-temporal model untouched.

## Remaining backlog (not audit findings — roadmap items)

- Hardening Card 3: ADR-0025 metrics renderer across eval-harness suites.

Hardening Card 4 (MemoryService composition-root) completed 2026-08-11: capability
seam fully migrated, `find_intro_chain` in `apps/graph.rs`, one-line delegates
removed, `core.rs` test module split into owning modules. PR profile 7/7, 119/119.
