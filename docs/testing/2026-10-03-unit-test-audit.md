# Unit-test audit — 2026-10-03

## Verdict

The current test collection cannot be certified as an isolated unit-test suite.
It contains useful integration scenarios, architecture-policy lints, private
implementation checks, uncontrolled clocks/global state, and several weak
assertions that do not establish the behavior named by the test.

The first priority is to repair false assurance—not to delete useful SQL,
transport, filesystem or recovery coverage to improve a unit-test count.

The requested standard is recorded in [AGENTS.md](../../AGENTS.md#testing) and
[ADR-0073](../adr/0073-test-behavior-not-document-inventory.md). In this audit:

- **Unit:** one scenario and focal action through the owning module's public
  interface; controlled inputs/adapters; no real infrastructure, wall clock,
  uncontrolled identity/entropy or process-global state.
- **Integration/artifact:** real database, filesystem, network, subprocess,
  environment, timer wiring, model fixture or shipped-artifact behavior.
- **Architecture lint:** source/module/caller/feature-policy checks. Useful
  checker fixtures are not production-functional coverage.
- A pure private-helper test is not automatically an integration test. It needs
  rewriting through the owning interface, or retirement when redundant.
- Several assertions about one returned result are legitimate. Several
  independent actions/cases or `if`/`for`/`while` assertion logic are not the
  requested unit style.

## Scope, inventory and evidence limits

Snapshot: `be8e2d3`, with the previously merged lexer/caller repairs still
uncommitted. A separate delimiter repair was running in the shared lexer and
public/trait checker files. Those changing files are not certified here.

All workspace targets were inventoried through Cargo. Body/setup inspections
were risk-based across memory, knowledge, embedding, storage, HTTP, control,
identity, tenancy, provisioning, operations, CLI/tools/MCP, shared/platform,
evaluation, release tooling and UI. Findings below are manually confirmed
examples, not an exhaustive semantic classification of every registered test.
The registration lists cover lib/bin/integration-test targets in the two stated
profiles, not doctests, benchmarks or unenabled platform-specific features.

| Listing profile | Listing targets | Registered tests |
|---|---:|---:|
| Default workspace | 102 | 2,426 |
| Memory optional profile: `fs-watch,mcp-apps,streamable-http,test-fixtures` | 94 | 3,154 |

Default workspace library/bin registrations include memory 1,657, eval-harness
184, UI 101 and xtask 16. The optional memory library registers 2,456 tests.
Feature selection/unification changes these counts. They include ignored and
checker tests and **are not counts of compliant units or executed scenarios**.
No compliance percentage is inferred from syntax or Cargo's `unittests` label.

Inventory commands, which list rather than execute tests:

```sh
cargo metadata --format-version 1 --no-deps --locked --offline
cargo test --workspace --lib --bins --tests --locked -- --list
cargo test -p memory_mcp --lib --bins --tests \
  --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- --list
```

No mutation checks were run during this audit. “Could pass” findings below are
based on the inspected oracle and execution path, not a claim that a mutant was
measured. The full suite was not rerun as an audit verdict. The fixed-path/
destructive xtask fixtures need isolation for routine concurrent runs; a
serialized verification run with an exclusively owned temp root is a viable
interim option before permanent fixture ownership is repaired.

## P1 — Repair weak or misleading behavioral oracles first

| ID | Evidence | What the current assertion fails to establish | Required direction |
|---|---|---|---|
| A01 | [`ingest_creates_new_episode` and duplicate case](../../crates/memory-mcp/src/memory/ingestion.rs#L262-L327); [`MockDbClient::create`](../../crates/memory-mcp/src/service/mock_db.rs#L275-L299) | The tests assert a derived ID. `expect_create` is a canned answer, not a verified invocation; the mock discards the payload and defaults to success. Missing creation or an incorrect payload can escape the oracle. | Record the create command/payload; assert exactly one correct write or zero writes for a duplicate. Supply fixed `t_ref` and `t_ingested`. Keep durable lineage checks as integration. |
| A02 | [`resolve_enforces_the_rate_limit_before_touching_the_resolver`](../../crates/memory-mcp/tests/memory_consumer_ports.rs#L93-L115) | A fixed resolver answer does not show that the resolver was untouched. Resolution before a later refusal could still return the expected error. | Use a recording or fail-on-call resolver and assert zero calls through `resolve_entity`. |
| A03 | [`arm_after_use_resets_the_idle_timer`](../../crates/memory-mcp/src/platform/model_runtime.rs#L208-L224) | The assertion at about 200 ms is before both the old 500 ms and renewed 600 ms deadlines; eventual unloading does not distinguish them. The immediate `get`/`arm` sequence also does not establish a completion-time reset after a long use. | Use controlled time; observe loader reuse across the old deadline and reconstruction beyond the renewed deadline through the module interface. Do not rely on private `loaded` state. |
| A04 | [`fill_missing_loser_reports_already_current`](../../crates/memory-mcp/tests/embedding_canonical_vectors.rs#L453-L490); [advisory read](../../crates/memory-mcp/src/embedding/api.rs#L207-L224) | The fills are sequential. The second request returns at the advisory read's `Present + FillMissing` branch and does not reach the conditional write. This does not prove the CAS-loser outcome. | Retain/rename the sequential-refusal scenario; add controlled interleaving between an absent read and the adapter write. Preserve real SQL execution. Earlier claims that this test independently closes the race were too broad. |
| A05 | [`run_reports_the_first_failing_script`](../../crates/xtask/src/observability.rs#L98-L162) | Successful later scripts could run without appearing in the error, so error-text absence does not prove short-circuiting. Fixtures use fixed ambient temp paths and `remove_dir_all`; child execution is unbounded. | Keep as tooling integration. Use unique owned RAII directories, bounded children, and an observable marker/event proving later scripts did not run. Do not run concurrent instances with the current shared fixture paths. |
| A06 | [release archive/failure scenarios](../../crates/xtask/src/pack.rs#L634-L701); [`write_artifact_is_atomic`](../../crates/eval-harness/src/artifact.rs#L553-L563) | Archive membership does not prove “nothing else”; directory read errors can look like no artifact; absence of a temporary file after success does not prove atomic publication. | Compare exact archive contents fallibly; reject unexpected read errors. Rename cleanup-only coverage and, if atomic replacement is the contract, test preservation of the existing final artifact on a controlled pre-rename failure. Keep artifact integration. |
| A07 | [`regex_entity_extractor_classifies_company_types`](../../crates/memory-mcp/src/knowledge/entity_extraction.rs#L418-L432); [progress “emits” cases](../../crates/memory-mcp/src/service/reembed_progress.rs#L341-L498) | An empty candidate collection satisfies the assertion loop. Reporter tests filter out the claimed events or assert no emitted output; a silent reporter can pass. | Assert the exact nonempty name/type result. Capture reporter output through an appropriate sink; retire unjustified no-op smoke assertions rather than label them emission evidence. |
| A08 | [resolve event-order cases](../../crates/memory-mcp/src/tools/resolve.rs#L203-L218); [`ingests_then_extracts_for_inline_content`](../../crates/memory-mcp/src/tools/extract.rs#L504-L514) | Stubs do not record capability invocation or validate the episode argument. Logging before/after and ingest-result forwarding are therefore not established. | Record calls and events in one trace; make extraction validate the distinctive ID returned by ingestion. Assert the complete expected sequence/result for one scenario. |
| A09 | [`single_flight_activation_runs_once`](../../crates/memory-mcp/src/http/runtime/pool.rs#L2006-L2031) | Spawning callers without holding activation open can exercise only warm reuse. A generation counter is not a factory-call witness. | Block the leader, explicitly register pending followers, release once, and assert returned acquisitions plus one recorded factory invocation. Preserve larger fan-out as integration/load evidence. |
| A10 | [`a_maintenance_binding_never_reaches_the_request_path`](../../crates/memory-mcp/tests/tenancy_resolution.rs#L93-L123); [recovery workflow fixture](../../crates/memory-mcp/tests/operations_recovery.rs#L123-L151) | Manually independent answers in a fake do not establish real maintenance/request isolation. Separate sweep/tombstone vectors do not establish cross-operation ordering. | Keep a named suspended-resolution unit. Prove deleting-maintenance/request refusal through the real resolver integration; use a single trace or explicit gate for sweep-before-tombstone. |

These are gaps in evidence, not claims that the corresponding production
implementation currently behaves incorrectly.

Atomic replacement is not a crash-durability promise. The inspected
[`write_artifact`](../../crates/eval-harness/src/artifact.rs#L372-L389) syncs the
temporary file but not the parent directory. Crash durability would require a
separate adopted contract; it is not a requirement invented by this audit.

## P2 — Isolation and test-surface violations

| ID | Inspected family | Classification and action |
|---|---|---|
| B01 | [Pool fixture/factory](../../crates/memory-mcp/src/http/runtime/pool.rs#L923-L1060) | The controlled factory opens `Surreal::new::<Mem>` and delegates to production runtime construction. **Keep as controlled integration**, not isolated unit coverage. Injection of a factory alone does not remove the infrastructure behind its returned concrete runtime. |
| B02 | [private permit / warm acquisition shutdown cases](../../crates/memory-mcp/src/http/runtime/pool.rs#L1305-L1365) | The direct private `acquire_tenant_permit` check repeats 64 times, uses a real clock and DB setup, and relies on probabilistic detection of the old randomized branch. **Rewrite through public acquisition**, retaining race evidence; do not expose the helper solely for tests. This includes tests written during the current task. |
| B03 | [Pool timeout/TTL/deadline cases](../../crates/memory-mcp/src/http/runtime/pool.rs#L1418-L1659) | The five-second production backoff, 300 ms TTL and real interval/deadline are timer-wiring **integration** scenarios. Keep them bounded. Separate controlled policy units only at a justified time seam; paused Tokio time alone cannot control the current `std::time::Instant` values. |
| B04 | [Pool replacement/private-state observations](../../crates/memory-mcp/src/http/runtime/pool.rs#L1479-L1695) | Generation/readiness fields, edited `retry_at`, and raw Notify subscribers do not substitute for returned runtime behavior and actual recovering acquisitions. **Rewrite observable assertions** using recorded factory commands, pending public acquisitions and resource recovery. |
| B05 | [canonical vector adapter scenarios](../../crates/memory-mcp/tests/embedding_canonical_vectors.rs#L337-L548); [fact-access adapter scenarios](../../crates/memory-mcp/src/memory/fact_access_store.rs#L292-L418) | Real embedded DB/migrations, SQL binding, atomic arithmetic and field preservation: **keep integration**. Do not replace them with a recording mock that cannot validate the query. Bound concurrent joins and establish defect-relevant interleaving. |
| B06 | [recording-port canonical units](../../crates/memory-mcp/tests/embedding_canonical_vectors.rs#L86-L223); [generation orchestration](../../crates/memory-mcp/src/embedding/api.rs#L274-L286) | Replace fixture `Utc::now()` with fixed supplied timestamps. Split preparation's valid/mismatch/empty actions. Generated-write execution still has a hidden clock dependency; reuse/introduce only a narrow justified time seam, not a universal test container. |
| B07 | [model lease reclaim policy](../../crates/memory-mcp/src/embedding/model_artifacts/lease.rs#L127-L240) | The “policy” test invokes `kill -0`/`tasklist` and assumes a large PID is dead. **Rewrite policy with supplied liveness**; keep bounded platform-process and actual lease-file integration separately. |
| B08 | [remote provider error cases](../../crates/memory-mcp/src/embedding/providers/remote.rs#L834-L935); [HTTP fixture](../../crates/memory-mcp/tests/common/http_server.rs#L100-L179) | Real HTTP, DNS, sockets, subprocesses and ambient host configuration are **integration**. Bound server/startup completion independently; dropping a listener does not reserve an unused port. |
| B09 | [filesystem stability / containment cases](../../crates/memory-mcp/src/service/fs_watch/candidate.rs#L297-L471) | Pure successful path manipulation is a unit; the escape case reaches `canonicalize` and mixes scenarios. Stability relies on sleeps for ordering. **Split lexical units from coordinated, bounded filesystem integration**. |
| B10 | [rate limiter cases](../../crates/memory-mcp/src/platform/rate_limiter.rs#L89-L198) | Real `Instant::now`, sleep-based refill, assertion loops and private-map constructor assertions. **Rewrite policy with controlled time**; retire the representation-only constructor check. Preserve meaningful exhaustion/refill/caller-isolation rules. |
| B11 | [identity/provisioning command tests](../../crates/memory-mcp/tests/identity_unlink.rs#L41-L70); [deletion command tests](../../crates/memory-mcp/tests/operations_deletion.rs#L36-L70); [private fingerprint cases](../../crates/memory-mcp/src/provisioning/api.rs#L578-L611) | Public ports are useful, but independent success/refusal cases, UUIDs and wall time prevent strict units. **Split named scenarios with fixed inputs** and verify recorded public commands. Fake replay equality is not durable transaction evidence. |
| B12 | [OIDC entropy/sample checks](../../crates/memory-mcp/src/control/oidc/flow_material.rs#L231-L303) | Two unequal random samples do not prove collision resistance or unpredictability. **Retire those security claims**; use controlled encoding/PKCE vectors and preserve real auth/replay integration. |
| B13 | [request-ID oracle](../../crates/memory-mcp/src/tools/request_id.rs#L5-L32) | Global state is not isolated; string ordering is wrong across the width transition (`req_10000` precedes `req_9999`). The test also never asserts padding. **Rewrite the actual numeric/correlation contract** with controlled state where justified; do not assert lexicographic monotonicity. |
| B14 | [readiness setup](../../crates/memory-mcp/src/http/health.rs#L51-L144); [test state builder](../../crates/memory-mcp/src/http/test_state.rs#L24-L42) | Replacing the registry after building `HttpState` does not undo its real DB initialization. **Keep route/runtime integration**, and use only supplied readiness inputs for separate policy units if the interface supports them. |
| B15 | [tool resolve entry](../../crates/memory-mcp/src/tools/resolve.rs#L28-L29); [extract entry/default timestamp](../../crates/memory-mcp/src/tools/extract.rs#L34-L35) | Controlled capability stubs still reach real elapsed-time reads and a global request counter; inline extraction can also read `Utc::now`. Do not certify these as strict units. Use explicit input metadata; retain adapter/runtime integration where effects are not injectable. |
| B16 | [MCP helper setup](../../crates/memory-mcp/src/mcp/handlers.rs#L1074-L1125); [corpus preparation](../../crates/eval-harness/src/corpus/prepare.rs#L193-L304); [model fixtures](../../crates/eval-harness/src/ner_fixtures.rs#L240-L249) | Real DB, publication/cache files or real model loading behind “fake” helpers remain **integration**. Preserve corruption, rollback, empty-label/model-loading and cache scenarios; assert bytes/state/fetch avoidance rather than path equality alone. |
| B17 | [admin configuration environment fixture](../../crates/memory-mcp/src/cli/admin_config.rs#L144-L209) | Process-global environment mutation is not pure isolation; a shared lock works only if all readers/writers participate. **Keep environment-adapter integration**; test parsing policy from supplied data when separated. |

## P3 — Simplify redundant tests and misleading names

- [UI private ID/time helpers](../../crates/ui/src/admin_api.rs#L1509-L1561),
  [private extraction helpers](../../crates/memory-mcp/src/shared/triple_extractor.rs#L352-L378)
  and [private CLI renderer helpers](../../crates/memory-mcp/src/cli/commands/init.rs#L117-L252)
  should be reached through their owning interface. Do not make them public just
  for testing; retire duplication after meaningful behavior is protected.
- [UI pagination](../../crates/ui/src/state/query.rs#L231-L333) mixes a journey,
  arrangement/default checks and multiple transitions. Split next/back/refresh
  scenarios; keep failure preservation and empty-page-loaded behavior. Retire
  bare default/getter assertions without a rule.
- [`capacity_overflow_returns_503`](../../crates/memory-mcp/src/http/runtime/pool.rs#L2084-L2109)
  never returns an HTTP response. Rename the Pool result check; retain real
  HTTP status mapping separately.
- [the Pool body test](../../crates/memory-mcp/src/http/runtime/pool.rs#L2060-L2078)
  supplies no operation pin and consumes the body to completion. Its name must
  not claim pin retention or premature-drop behavior.
- [canonical protocol constant equality](../../crates/memory-mcp/src/mcp/handlers.rs#L1147-L1149)
  and [pass-through session constructor checks](../../crates/memory-mcp/src/mcp/session.rs#L150-L174)
  are retirement candidates. Preserve actual negotiation/serialized-envelope
  scenarios and explicit-failure mapping rules.

## Architecture lints are a separate evidence class

The rooted source walk, public/trait lexical caller ratchets, duplicate-source
and helper-use checks, UI request-construction checks and toolchain-pin checks
are **not production units**. Some read real source files or start Cargo.
Their fixture tests establish checker correctness, not use-case correctness.
The trait ratchet explicitly accepts test witnesses and does not resolve
receiver types.

Keep useful lints labelled as such. Do not restore deleted document inventory
or mirrored-status tests, and do not include lint counts in functional coverage.

## Existing useful units and selected execution

These five manually inspected isolated examples were run during this audit and
passed individually. Test-body execution was reported as 0.00–0.01 s; compilation
time is not included in that observation:

| Scenario | Command filter |
|---|---|
| [Recall forwards query and budget](../../crates/memory-mcp/tests/memory_recall.rs#L91-L107) | `--test memory_recall recall_delegates_the_query_and_budget_to_retrieval` |
| [Disabled generation performs no write](../../crates/memory-mcp/tests/embedding_vector_policies.rs#L158-L187) | `--test embedding_vector_policies a_disabled_provider_skips_the_write_and_reports_why` |
| [Search strips episode references](../../crates/memory-mcp/src/shared/search.rs#L270-L273) | `--lib preprocess_search_query_strips_episode_references` |
| [Validation error maps to protocol error](../../crates/memory-mcp/src/mcp/error.rs#L258-L262) | `--lib maps_validation_to_invalid_params` |
| [Regression fails despite passing the floor](../../crates/eval-harness/src/gate.rs#L178-L181) | `-p eval-harness --lib regression_fails_even_above_the_hard_floor` |

Other useful patterns include a refused lifecycle transition never touching its
recording store and a legal transition forwarding the exact command. These
assert policy through existing narrow interfaces without claiming SQL CAS
correctness.

## Recommended remediation order

1. **Repair the P1 oracles.** Add recording/fail-on-call adapters and unified
   event traces; force CAS/concurrency interleavings; make reset/publication
   scenarios distinguish the wrong behavior. Confirm each regression fails
   against its defect, rather than trusting its name.
2. **Separate evidence classes and isolate fixture ownership.** Keep real SQL,
   transport, timers, filesystem and artifact cases as integration; eliminate
   shared destructive temp paths and bound all child/wait lifetimes. Do not
   advertise `cargo test --lib` as a pure-unit command in the current layout.
3. **Control unit dependencies at narrow seams.** First use fixed input
   timestamps/IDs and existing ports. Introduce time/identity adapters only
   where real variation warrants them; no shared service/test container, new
   configuration knob or visibility expansion solely for tests.
4. **Split independent cases and retire redundancy.** Separate success,
   rejection and edge cases; remove private/default/pass-through assertions only
   after the meaningful owning-interface behavior is covered.
5. **Establish explicit unit/integration/lint execution contracts.** Preserve
   integration coverage when reorganizing Cargo targets or module naming.
   A new feature/dependency is not authorized by this audit.

## Completion criteria for subsequent repairs

A retained unit must have controlled dependencies, one named scenario/focal
action, a public owning interface, and a non-vacuous outcome assertion. It must
be repeatable and fast without real infrastructure or process-global state.
Integration must still exercise the actual adapter/transaction/transport where
that is the contract. Lints remain labelled separately. This report and an
increasing test count are not themselves passing production evidence.
