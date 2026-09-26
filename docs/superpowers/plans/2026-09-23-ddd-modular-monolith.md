# DDD Modular Monolith Implementation Plan

**Date:** 2026-09-23; architecture review: 2026-09-24
**Status:** Execution plan; unchecked items are future work, not completed evidence.
**Source baseline:** original review `5bcb2bf3309ecebf71417c5ab2576e6cca0ca4ee`; post-review `origin/master` `791f903fb453f5d40e4bcea9ff8e5e9b4fbeb261`, merged as `b11c12c77e86f135364006437526db9a061791dc`.
**Goal:** Enforce module ownership and clean dependencies across the full
monolith, rename the console to `ui`, and remove both runtime product-shape
switches with no other externally observable change.
**Spec:** [design contract](../specs/2026-09-23-ddd-modular-monolith.md)
**ADR:** [ADR-0058](../../adr/0058-bounded-contexts-modular-monolith.md)

## Execution contract

One coordinated initiative on `ddd-refactorings`, with buildable/reviewable
commits and one coherent final release. Do not require a half-migrated branch
to represent the final architecture or claim every intermediate phase is
already merged to `master`. Before each phase, expand its checklist into
file/symbol-level steps using the installed `writing-plans` skill.

The spec is authoritative for ownership, dependencies and compatibility;
this plan defines sequencing and evidence. Seven module names do not require
seven empty domain layers. Do not scaffold unused abstractions. Preserve
inward dependencies when moving behavior, not only when updating imports.

- Exactly four surface-change categories: (1) package/bin/assets `ui`,
  (2) UI runtime switch removed, (3) control-plane runtime switch and
  data-plane-only mode removed, (4) internal feature/build-env/generated
  asset names renamed (`ui`, `MEMORY_MCP_UI_DIST`, `ui_assets.rs`).
- Freeze MCP tools, HTTP contracts, cookie/OIDC rules, signup/auth methods,
  temporal semantics, config values/defaults and persisted records otherwise.
- Preserve one local Active Namespace per process and one immutable SaaS
  namespace per Tenant Runtime; no data-plane namespace selector.
- No migration SQL/schema changes, new MCP tools or dependency additions are
  implied. Obtain the specific repository-required approval if a later
  implementation step needs any of them; documentation changes do not do so.
- Existing public Rust callers (binaries, tests, eval harness and compatibility
  exports) must remain supported through deliberate facades, not public internals.
- Temporary legacy adapters must have exact source/target edges, an owner and
  removal phase. Extracted modules may depend on a port backed by legacy code
  through bootstrap integration, never directly on legacy service internals.
- No empty-directory guard evidence, new global service container, generic
  repository/event bus, or duplicated business rules to keep phases compiling.

## Phase 0: Baseline, ownership and executable boundary harness

**Deliverable:** verified migration manifest and behavioral baseline before
file moves. This phase resolves actual seams rather than creating empty modules.

- [ ] Record starting revision, clean/dirty state, toolchain, package names and
  CI feature matrix; inspect current AGENTS instructions. Treat
  `791f903fb453f5d40e4bcea9ff8e5e9b4fbeb261` plus the documented merge
  (`b11c12c77e86f135364006437526db9a061791dc`) as the post-review source
  baseline. Do not overwrite unrelated work.
- [ ] Enumerate all tracked `src` files and affected consumers/build artifacts.
  Produce a versioned migration manifest under `docs/architecture/` with each
  source, destination(s), symbol split, layer, canonical-table/command owner,
  phase and allowed temporary edge. Check coverage mechanically against Git.
- [ ] Map context-owned models, pure shared values and technical platform
  separately. Explicitly split `ServiceContext`, registry storage,
  `context_store.rs`, `app_store.rs`, query helpers, outbox and error mapping
  as required by the spec; include behavior in module roots.
- [ ] Record existing public Rust paths used by binaries, integration tests,
  benchmarks/eval harness and published compatibility surfaces. Define the
  public bootstrap and minimal test/eval facade before restricting visibility.
- [ ] For each atomic operation in the spec, record exact tables, namespace,
  store method, audit/event side effects, failure/retry semantics and test.
  For identity invitations, keep `oidc_request` issue/consume, guarded
  `external_identity` link/replace plus audit, and optional first-login
  `control_plane_session` creation as distinct boundaries. Include
  single-use/ten-minute expiry, policy epoch, sealed-intent compatibility,
  exactly-one replacement, replay and conflict-classification cases. Keep
  existing transaction boundaries; design narrow atomic ports before
  replacing the broad registry or splitting any store.
- [ ] Record the allowed dependency graph, public data contracts and bootstrap
  integration exceptions. Specify identity administration/link/auth-method/
  invitation commands as well as sign-in/verification. Specify tenant runtime
  factory and canonical vector-update ports without reverse context dependencies.
- [ ] Add `tests/module_boundaries.rs` harness with positive and negative
  fixtures for privacy/paths/re-exports/signatures/cycles and feature branches.
  Record legacy violations explicitly; start production enforcement with the
  first real extraction in Phase 2. Fixtures must fail when the checker is
  deliberately disabled; do not call fixtures proof of migrated production code.
- [ ] Capture baseline HTTP/MCP responses and relevant transaction, authorization,
  namespace, task/lease and temporal tests. Run the local gate set below.
- [ ] Exit gate: no unmapped source, no undefined transaction owner, no hidden
  context cycle and an implementable public composition path. Revise the spec
  before extraction if these cannot be satisfied.

## Phase 1: UI rename and declared runtime-mode retirement

**Deliverable:** rename and breaking-mode removal are independently reviewable;
no domain extraction is hidden in this phase.

- [x] Rename `crates/control-plane-ui/` → `crates/ui/`, package/bin → `ui`,
  Dioxus name → `memory-mcp-ui`; update workspace membership and lockfile
  package entry without opportunistic dependency updates.
- [x] Rename internal feature to `ui`, build variable to `MEMORY_MCP_UI_DIST`,
  generated manifest to `ui_assets.rs`; update all current build/cfg/container/
  CI references. Derive the hit list from the checkout, not a historic count.
- [x] Remove both runtime enable fields/env parsers and their product-mode
  branches. Preserve method-dependent provider validation and security checks.
- [ ] Replace obsolete mode tests with unconditional SaaS mount tests and
  method-specific routing tests; do not simply reduce coverage.
- [ ] Test upgrades from formerly disabled control plane under OIDC, local,
  and combined auth methods: discovery/startup failure, local bootstrap,
  method/key drift, durable reconciliation, readiness and protected routes.
- [ ] Prove optional-store fallbacks unreachable in every supported constructor
  before deleting them; otherwise preserve them. Runtime-flag removal alone
  is not evidence that an `Option` can no longer be absent.
- [x] Update README, AGENTS, compose and operator runbooks with concrete upgrade
  prerequisites. Historical ADRs 0001–0057 and old plans/specs remain unchanged.
- [ ] Rebuild the actual bundle; verify JS/WASM names and the complete existing
  sentinel/base-stamping behavior at root and `/memory`. Derive expected
  markers/scenarios from tests, not stale hardcoded counts (4 vs 5, 22/22).
- [ ] Verify empty-catalog developer/test build separately from the bundled
  release image. Update `test_ui_bundle_pin.py`, `test_assert_embedded_ui.py`,
  image tests and actual-binary asset checks.
- [ ] Exit gate: local checks plus final-image/browser acceptance and an upgrade
  note. Rollback uses the previous artifact and its matching configuration;
  no data migration is performed.

## Phase 2: First real extraction — UI and operations

- [x] Move asset delivery to layer-free `ui`; cookie/CSRF request handling stays
  HTTP, while identity owns session/recent-auth policy.
- [ ] Establish only the pure kernel/platform/bootstrap pieces this extraction
  needs; do not postpone a clean dependency foundation until Phase 5.
- [x] Extract operations use cases and the narrow atomic deletion ports; keep
  HTTP handlers in HTTP. Preserve existing deletion transactions and recovery.
- [ ] Where identity/tenancy/provisioning implementations are still legacy,
  inject exact temporary port adapters from bootstrap with Phase 3/4 expiry.
  Do not export legacy internals through operations API.
- [ ] Enforce boundary guards on actual extracted code, including bootstrap
  exceptions and the adapter/API signatures. Record and test allowed edges.
- [ ] Exit gate: local checks, deletion recovery/auth/atomicity tests and UI
  acceptance; manifest updated, no new legacy edge.

## Phase 3: Identity and tenancy

- [ ] Split registry by policy/table ownership; do not move the entire registry
  into tenancy. Introduce the explicit control transaction integration adapter
  for existing cross-owner atomic methods, with narrow published ports.
- [ ] Extract identity verification, links, invitations, administrator/session/
  auth-method use cases and provider/KDF adapters. HTTP handlers/cookies/CSRF
  are adapters. Keep invitation request consumption, identity mutation/audit and
  optional first-login session creation as separate use-case ports/transactions.
  Preserve the `jsonwebtoken` crypto-provider feature and the real RS256/JWKS
  verification path.
- [ ] Test last-link/last-admin and mutation-with-audit invariants, including
  concurrent failure paths on the durable adapter. Cover invitation
  single-use/TTL and policy-epoch guards, exact-one replacement with both audit
  rows, same-tuple replay, cross-Account conflict, refusal-vs-storage error
  classification, legacy sealed-intent defaults, and first-login session
  creation without changing an existing session.
- [ ] Extract trusted tenant resolution and runtime lifecycle. Bootstrap supplies
  a runtime factory; tenancy application does not construct memory services.
- [ ] Test tenant A/B isolation, immutable binding, eviction/reactivation,
  credential revocation/cache behavior and rejected caller namespace fields.
- [ ] Expose minimal public bootstrap paths for both binaries; verify local and
  SaaS configurations and feature-gated test/eval consumers compile.
- [ ] Remove Phase 2 identity/tenancy bridge exceptions. Extend the guards and
  run local checks plus applicable protocol conformance from
  `docs/operations/CONFORMANCE.md`.

## Phase 4: Provisioning and durable workflows

- [ ] Extract client/API-key administration and provisioning workflows. Identity
  remains the single owner of credential verification and administrator rules.
- [ ] Keep `create_account_bundle` atomic through its published command port;
  no sequential account/tenant/identity facade writes.
- [ ] Extract task/app-session submodules and lease handling. Preserve fences,
  cancellation, idempotency, terminal results and recovery across process exit.
- [ ] Split `service/apps*` and `app_store.rs` by canonical data owner; call
  memory/knowledge APIs for their behavior. For still-legacy implementations,
  use exact bootstrap port bridges expiring in Phase 5.
- [ ] Separate durable outbox transaction mechanics from HTTP subscription
  streaming; owner stores commit mutation + event together.
- [ ] Remove previous provisioning bridges. Run local checks and durable
  lease/task/session race, rollback and event-order tests; update the manifest.

## Phase 5: Knowledge, memory and embedding

- [ ] Extract bottom-up within this phase: embedding technical contracts,
  knowledge policies/queries and memory orchestration, using the approved DAG.
  *(Partially done: the contexts' `api.rs` interfaces and their ports exist and
  are consumed, and the knowledge read scope now backs `build_diff`; the
  remaining legacy modules are still in place.)*
- [ ] Replace `ServiceContext` with explicit consumer-owned ports/dependencies.
  Split model types and database row conversions; do not make a shared god-model.
  *(Partially done: the six MCP tool handlers no longer take `ServiceContext`
  — they take the transport-facing `tools::context::ToolContext`, implemented
  once in `service/capabilities/tool_context_impl.rs`. The container still
  exists as the thing the capabilities adapt, so the god-struct is contained
  rather than removed.)*
- [ ] Split `context_store.rs` into owner-owned knowledge reads and memory
  episode/access-log storage. Remove arbitrary-table query/update interfaces
  from application-facing APIs. Keep cross-owner optimized reads only behind
  explicit provider contracts; benchmark any query decomposition.
- [ ] Move canonical storage to owner infra, technical clients/transaction
  mechanisms to platform, wiring to bootstrap. Do not rewrite migration SQL.
- [ ] Preserve pure temporal semantics and the single knowledge close/write
  implementation, atomic retraction/outbox scope, source provenance, claims,
  inbox revisions and procedure review/lifecycle semantics.
- [ ] Keep canonical vector writes owned by the record's module: embedding
  job integration uses narrow owner-approved ports wired by bootstrap; no
  backdoor database access or recursive callback cycle. Test the static DAG
  separately from the approved runtime integration paths; vector endpoints
  must never call embedding again.
- [ ] Preserve behavior assertions; imports/fixtures may change when paths do.
  "Tests unchanged" means semantic expectations, not literal source identity.
- [ ] Remove remaining legacy bridges. Run local checks, actual evaluation
  profiles/response-size gate and relevant workload measurements (latency,
  query count, allocations) against the Phase 0 baseline. A harness unit-test
  run is not a retrieval-quality evaluation.

## Phase 6: Adapter sweep, docs and release gate

- [ ] Enforce final graph and facade-only adapter use; delete legacy business
  service/storage implementations and expired exceptions. Keep only recorded
  outward compatibility aliases with no internal consumers.
- [ ] Check shutdown, worker recovery, durable SaaS state and local stdio log
  framing against the spec's Twelve-Factor table. Record real limitations;
  behavior changes outside the freeze require separate scope.
- [x] Update CONTEXT module seams and current implementation status, README,
  AGENTS and runbooks. In AGENTS replace the old `service/` business-logic
  placement rule with context-local use cases and state that current default
  features are `["fs-watch"]`; do not leave the conflicting `default = []` rule.
- [x] Re-run manifest coverage and boundary negative tests across supported
  features. Verify no ordinary transport obtains SQL/privileged storage via API.
- [x] Run complete CI and release acceptance below on the final revision.
  Record failures/skips explicitly; no green claim based on partial jobs.
- [ ] Exit gate: final spec/ADR/code agree, all required evidence is attached,
  no expiring bridge remains, release and rollback configuration documented.

## Verification commands and evidence

The following is the **local gate set**, not an exact mirror of every CI job.
Run per completed phase; reuse results only for the same revision/features.
Before Phase 1 substitute `control-plane-ui` for `ui`.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo clippy -p ui --target wasm32-unknown-unknown --all-targets --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,test-fixtures --locked
cargo check -p memory_mcp --all-targets --no-default-features --features control-plane,test-fixtures --locked
cargo check -p memory_mcp --all-targets --no-default-features --locked
python3 -m unittest discover -s scripts/ci -p 'test_*.py'
```

The internal `control-plane` compatibility feature remains a build check, not
a supported data-plane-only runtime mode. Keep the four supported profile
combinations explicit and compile each with no implicit default feature:

```sh
cargo check -p memory_mcp --all-targets --no-default-features --features fs-watch --locked
cargo check -p memory_mcp --all-targets --no-default-features --features fs-watch,mcp-apps --locked
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http --locked
cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,mcp-apps --locked
```

Additive Cargo features must not accidentally make SaaS dependencies mandatory
in either local combination.

Run the **actual** evaluation job for the phase/final revision; create the
artifact directory first, as CI does:

```sh
mkdir -p target/evals
cargo run -p eval-harness --bin memory-eval --locked -- run --profile evals/profiles/pr.json --artifact target/evals/pr.json --baseline evals/baselines/one-active-namespace-pr.json
make eval-response-size
```

For release, use the release profile/artifact/baseline named in current CI.
`cargo test -p eval-harness` tests the harness; it does not execute these profiles
or certify quality. Missing corpora/providers are missing evidence, not passes.

Final integration requires all applicable jobs in `.github/workflows/ci.yml`,
including native target builds/packages, macOS feature lint, filesystem tests,
Docker final-image asset checks and aggregate gate. Run browser/conformance
acceptance at root and `/memory`, with configured authentication variants;
execute durable multi-replica/failure tests for changed transaction seams.
Capture exact artifact names and scenario counts from the run. A documentation-
only revision reports document/link checks and any attempted baseline lint
separately; it does not claim to have executed this future migration plan.
