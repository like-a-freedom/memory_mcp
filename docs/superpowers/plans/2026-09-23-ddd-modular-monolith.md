# DDD Modular Monolith Implementation Plan

> **For agentic workers:** before executing, drill each phase to step-level
> detail with superpowers:writing-plans (the phase boundaries below are the
> stable contract; step granularity is per-phase). Steps use checkbox
> (`- [ ]`) syntax for tracking.

**Goal:** Reorganize `memory_mcp` into explicitly bounded contexts with clean
architecture inside each, rename the console package to `ui` (fixing outward
asset names), and delete both runtime product-shape switches — zero other
behavior change.

**Architecture:** Seven bounded contexts (`identity`, `tenancy`,
`provisioning`, `operations`, `memory`, `knowledge`, `embedding`) plus a
`shared` kernel inside one crate; each context is `api` (facade) +
`domain`/`application`/`infra`. Adapters (`http`, `mcp`, `cli`, `tools`, `ui`,
`bin`) call facades only; `ui` (console-bundle delivery) is deliberately
layer-free — cookie/CSRF hardening belongs to `identity::infra` (Control Plane
Session), and there is no `delivery` context by review decision.
Dependency rules are source-guarded by `tests/module_boundaries.rs`.

**Tech Stack:** Rust 1.97.1 workspace, axum 0.8, SurrealDB 3.2, Dioxus 0.7.10
(`dx bundle` sentinel contract unchanged), stdlib Python + Playwright CI checks.

**Spec:** `docs/superpowers/specs/2026-09-23-ddd-modular-monolith.md`
**ADR:** `docs/adr/0058-bounded-contexts-modular-monolith.md`

## Global Constraints

- **Behavior freeze** (spec non-goals): eight-tool MCP surface, HTTP routes and
  envelopes, bi-temporal model, namespace/tenant invariants, cookie contracts,
  and every deployment knob except the two declared breaks.
- **Declared surface changes (exactly four):** (1) package + outward asset
  names renamed to `ui`; (2) `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI`
  removed; (3) `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE` removed (the
  data-plane-only runtime mode is retired — product shape lives in the build
  profile alone, route exposure in the reverse proxy); (4)
  `MEMORY_MCP_CONTROL_PLANE_UI_DIST` → `MEMORY_MCP_UI_DIST` (hard rename).
- `MEMORY_MCP_HTTP_PUBLIC_BASE_URL` semantics are not touched. No new
  environment variables.
- The relocatable-bundle sentinel `/__memory_mcp_base__` and its five stamping
  sites keep working unchanged (content-driven, name-independent).
- **Gate matrix per phase — the exact CI mirror** (a reduced matrix shipped
  broken code twice):
  1. `cargo fmt --all --check`
  2. `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings`
  3. `cargo clippy -p ui --target wasm32-unknown-unknown --all-targets --locked -- -D warnings` (name after Phase 1; `control-plane-ui` before it)
  4. `cargo test --workspace --lib --bins --tests --locked`
  5. `cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked`
  6. `cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,test-fixtures --locked` and the same with `control-plane`
  7. `python3 -m unittest discover -s scripts/ci -p 'test_*.py'`
- Every phase leaves `master` green. Phases are reviewable commit series;
  do not merge a phase half-done.

## Module → context inventory (phase input)

| Phase | From (today) | To (target) |
|---|---|---|
| 2 | `control/static_assets.rs`, UI packaging | `ui/` adapter (layer-free) |
| 2 | `control/{operator.rs, deletion.rs}` | `operations::{api, domain, infra}` |
| 3 | `control/{account_api.rs (auth part), oidc.rs, oidc/, local_admin.rs, local_admin/, session.rs, recent_auth.rs, secret.rs, csrf.rs}` (cookie/CSRF hardening), `control/application/oidc_signup.rs`, `service/{local_admin/, credential_material.rs}`, `http/{oauth.rs, principal/, principal.rs}` | `identity::{api, domain, application, infra}` |
| 3 | `http/{registry/, registry.rs, runtime/, runtime.rs, sync.rs}`, namespace-selection of `service/` | `tenancy::{api, domain, application, infra}` |
| 4 | `http/{leases/, leases.rs, tasks/, tasks.rs, app_sessions/, app_sessions.rs}`, `service/{apps/, apps.rs}`, `control/application/api_keys.rs`, `account_api.rs` (client/key admin part), `storage/{app_store.rs, context_store.rs}` | `provisioning::{api, domain, application, infra}` |
| 5 | `service/{episode/, episode.rs, context/, capabilities/, capabilities.rs, agent_memory/, lifecycle/, lifecycle.rs, core/, core.rs, procedures/, procedures.rs, ingestion.rs, explanation.rs}` (incl. procedural memory), `storage/{episode_store.rs, agent_memory.rs, procedures.rs}` | `memory::{api, domain, application, infra}` |
| 5 | `service/{fact.rs, entity.rs, entity_resolution.rs, claims/, conflict_resolver.rs, community.rs, content_extraction/, content_extraction.rs, entity_extraction/, entity_extraction.rs, triple_extractor.rs, query/, query.rs}`, `storage/{fact_store.rs, entity_store.rs, triple_store.rs, claims.rs, inbox_revision_store.rs}` | `knowledge::{api, domain, application, infra}` |
| 5 | `service/{embedding/, embedding.rs, embedding_service.rs, embedding_recovery.rs, embedding_runtime.rs, model_artifacts/, model_artifacts.rs, model_artifact_refresh.rs, model_loader.rs, model_runtime.rs, cache/, cache.rs, reembed.rs, reembed_options.rs, reembed_progress.rs}`, `storage/{embedding_backfill_store.rs, embedding_state_store.rs, reembed_store.rs}` | `embedding::{api, domain, application, infra}` |
| 5 | `models/`, `error.rs`, `control/error.rs`, config parsing (root `config.rs` + `config/`, `http/config*`), `service/{service_context.rs, util/, util.rs, value_helpers.rs, durable_work.rs}`, `storage/{client.rs, migrations.rs, close.rs, helpers.rs, queries.rs, types.rs}` | `shared/` |
| 6 | `http/`, `mcp/`, `cli/`, `tools/`, `bin/`, `http/{subscriptions/, subscriptions.rs}` (stream machinery) | outer adapters over `*::api` only |
| 6 | platform residuals: `logging.rs`, `observability.rs`, `runner.rs`, `eval_support.rs`, `service/{startup.rs, fs_watch/, fs_watch.rs, mock_db.rs}`, root module glue | stay at root/platform (unchanged) |

The **census rule** from the spec applies: every top-level unit of `src/` has
exactly one home (context row, transport/platform residual, or the `storage/`
split). Unit-level assignment beyond this table is resolved in each phase's
drill-down and must amend the spec if it moves anything un-censused.

---

### Phase 0: Scaffolding and the guard harness

**Deliverable:** empty-but-compiled context skeletons (seven contexts + the
`shared` kernel) plus this docs package. The `tests/module_boundaries.rs` harness lands **with Phase 2**,
pinned against the first real extraction — a harness asserted against today's
unmoved layout would either fail or be vacuous.

- [ ] Create `src/{identity,tenancy,provisioning,operations,memory,knowledge,embedding}/mod.rs` with `api`/`domain`/`application`/`infra` stubs and `src/shared/` kernel stubs (`pub(crate)` discipline from day one; no `delivery` module — `ui` is a layer-free adapter that arrives with Phase 2)
- [ ] Wire the modules into `lib.rs`; full gate matrix
- [ ] Commit: `refactor: scaffold bounded-context layout (ADR-0058)`

### Phase 1: Rename to `ui` + runtime product-shape knobs removal

**Deliverable:** all 133 identifier hits and outward asset names renamed; both
runtime product-shape switches (`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI`,
`MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE`) and the data-plane-only runtime mode
deleted; the one `dx` probe re-run.

- [ ] `crates/control-plane-ui/` → `crates/ui/`; package + `[[bin]]` → `ui`; `Dioxus.toml` name → `memory-mcp-ui`
- [ ] Feature `control-plane-ui` → `ui` (memory-mcp `Cargo.toml`, all `cfg` sites, AGENTS.md, CI)
- [ ] `MEMORY_MCP_CONTROL_PLANE_UI_DIST` → `MEMORY_MCP_UI_DIST` (`build.rs`, Dockerfile, README)
- [ ] `control_plane_assets.rs` → `ui_assets.rs` (`build.rs`, `static_assets.rs`)
- [ ] Delete `enable_control_plane_ui` / `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE_UI` **and** `enable_control_plane` / `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE`: config parsing + validation branches (`http/config/{types,validate}.rs`), mode branches (`http.rs` ~:171/:211/:228), router gating and its "control off" feature-matrix test rows (`http/router.rs`), store-level data-plane-only degradation paths (`http/registry/surreal_store/local_admin_rate.rs`), `docker-compose.yml` (:110 comment + env), README (:402, :678, :686, :792, :818, :1112)
- [ ] Update `scripts/ci/test_ui_bundle_pin.py`, `test_assert_embedded_ui.py`, `local_admin_image.py` names; extend pins for the `ui` asset name
- [ ] `Cargo.toml` workspace members and the `Cargo.lock` package entry follow the rename
- [ ] Historical documents stay verbatim by design (interview Q6): ADRs 0001–0057 and past specs/plans are not touched; only living docs (README, AGENTS.md, `docs/operations/`, compose, CI) change
- [ ] `dx bundle` probe: verify emitted names are `ui-dxh*.js` / `ui_bg-*.wasm` and the sentinel contract is intact (4 sentinel sites in the shell)
- [ ] Acceptance: gate matrix + `docker compose build` + browser `ui` scenario at `/memory` and root (22/22 twice)
- [ ] Commit series: `refactor: rename control-plane-ui package to ui`, `feat!: remove runtime product-shape switches (data-plane-only mode retired)`

### Phase 2: `ui` adapter + `operations` (prove the layout)

- [ ] `control/static_assets.rs` → `src/ui/` (console-bundle serving + base-stamping contract); explicitly **no** domain/application/infra layers — it is delivery plumbing, and inventing layers for it is the DDD theater this rework removes
- [ ] Move `operations` per inventory; author `operations::api`
- [ ] Land `tests/module_boundaries.rs`: facade-only cross-context rule + `domain`-purity rule (no axum/surrealdb in any `domain/`), pinned against `operations`; adapters (`http`, `mcp`, `cli`, `tools`, `ui`, `bin`) must not import context internals
- [ ] `http/router.rs` fallback switches to `ui::serve_asset` and `operations::api`
- [ ] Full gate matrix + acceptance browser run
- [ ] Commits: `refactor: extract ui adapter and operations context`

### Phase 3: `identity` + `tenancy`

- [ ] Move per inventory (the largest surface: OIDC, local-admin, sessions, registry)
- [ ] `identity::api` exposes sign-in/verification use cases only; `tenancy::api` owns namespace selection — codify "never a request argument" as a `tenancy::domain` policy test
- [ ] Extend `module_boundaries.rs` rows to both contexts
- [ ] Full gate matrix + `docs/operations/CONFORMANCE.md` protocol conformance run
- [ ] Commits per context

### Phase 4: `provisioning`

- [ ] Move per inventory (clients, keys, leases, tasks, app sessions)
- [ ] Last-administrator rule and lease expiry remain behavior-identical: existing tests must pass unmodified (the freeze's proof)
- [ ] Extend guard rows; full gate matrix

### Phase 5: `knowledge` + `memory` + `embedding` + `shared`

- [ ] Split `service/` by inventory; `ServiceContext` narrows into per-context ports (its successor shape is decided in this phase's drill-down)
- [ ] `storage/` dissolves into per-context `infra`
- [ ] Bi-temporal and extraction tests pass unmodified (freeze proof)
- [ ] Extend guard rows to every context; full gate matrix + eval harness smoke (`cargo test -p eval-harness` per its feature)

### Phase 6: Transports sweep and documentation close-out

- [ ] `http/`, `mcp/`, `cli/`, `tools/` import only `*::api` + `shared`; guard rows final
- [ ] `CONTEXT.md` `Module seams` section rewritten to the target map (drop the ADR-0058 interim banner)
- [ ] README architecture section, AGENTS.md code-navigation table, `docs/operations/` references updated
- [ ] Full gate matrix + full acceptance (compose `/memory` + root, browser 22/22 ×2, probe matrix from the relocatable-bundle acceptance)
- [ ] Final commit: `docs: close out ADR-0058 module seams`

---

## Risks carried into execution

- Conflict-hot surface (this repo has concurrent writers): announce the
  branch, land phases fast, rebase onto `master` between phases.
- Phase 5 is the riskiest (ServiceContext/DbClient seams); its drill-down must
  decide the port shape before any move.
- Any gate failure blocks the phase; no "fix forward" across phase boundaries.
