# Unit-test remediation implementation plan

**Status:** completed. Final verification is recorded below; the changes are
committed together with this plan.
The user confirmed the first five seams and authorized autonomous completion of
the audit's remaining actions. Completion is scoped to those audit actions,
not certification of every repository test as an isolated unit.

**Source:** [unit-test audit](../../testing/2026-10-03-unit-test-audit.md).
**Policy:** [ADR-0073](../../adr/0073-test-behavior-not-document-inventory.md).

## Seams and scope

The confirmed first batch exercises existing public interfaces:

1. `memory::api::ingest_episode` and `knowledge::api::owned_episode_scan`.
2. `memory::api::resolve_entity`.
3. `embedding::api::update_canonical_vector` and `CanonicalVectorPort` reads.
4. Public Pool acquisition and the returned operation/runtime interface.
5. `xtask::observability::run` and its externally observable script effects.

The remaining audit actions use their existing owning interfaces: model
retention/inference acquisition, rate limiting, public tool entry points,
reporter output, identity/provisioning/tenant/deletion operations, provider and
model-artifact operations, corpus/artifact preparation/publication, CLI
commands/parsers, UI query transitions and observable serialized results.
Private helpers and test-only state inspection are not additional test seams.

No new MCP tool, migration, dependency, feature/configuration knob, or
visibility expansion solely for tests is authorized. Controlled time/identity
dependencies may be supplied at the owning interface without changing wire
contracts or production defaults.

## Execution rules

- Work vertically: one observable scenario, its sensitivity evidence, minimal
  repair, targeted tests/typecheck; then the next scenario.
- For already-correct behavior whose old oracle was weak, do not invent a
  production failure. Prove the stronger oracle rejects a controlled
  defect/mutation in an isolated checkout, restore it, and record this as
  sensitivity evidence rather than a newly discovered production bug.
- Mock only external effects. Use real owning modules and public reads instead
  of internal spies, raw SQL assertions or private state.
- Give each unit case fixed inputs, one focal action and straight-line
  assertions. Use separately named/parameterized cases, not assertion loops.
- Preserve actual storage/transport/filesystem/timer/artifact scenarios as
  integration evidence. Label checker fixtures/lints separately.
- Run targeted tests and typechecking regularly. Run full profile gates after
  integration/review, with an exclusively owned temp root.
- Review completed changes independently, then commit to the current branch.
  No publication/push is part of this plan.

## Workstreams and completion ledger

Classification/retirement is an explicit disposition, not a fabricated unit
replacement.

| Audit rows | Owner workstream | Required disposition | State |
|---|---|---|---|
| A01, A02 | Memory/knowledge | Observable persisted ingestion/dedup and rate-refusal effect through real interfaces | Implemented: ingestion target 5 and consumer-port target 2 passed in parent; denial checks real resolver persistence through owner read; worker's reversed-order mutation rejected |
| A04, B05, B06 | Embedding/persistence | Actual CAS interleaving; owner reads; fixed timestamps; preserve/classify real SQL cases | Done: vector targets 12 + 11 passed; CAS loser reads the winning signature, access write preserves an explicitly interleaved increment/vector; clock-free skip asserted |
| A07 regex | Knowledge extraction | Exact nonempty company classifications through extraction interface | Done: exact extracted name/type collection replaces the vacuous loop |
| B07, B08, B09 | Core infrastructure | Remove private/mixed policy claims; coordinate/bound real process/network/FS integration | Done: supplied liveness policy, bounded platform probe, bounded loopback/DNS integration, initial synchronous file sample established by explicit poll; retained as integration |
| A09, B01–B04 | HTTP runtime | Public blocked cold acquisitions/recovery; remove private counters/notifier/permit oracles; explicit integration classification | Done: blocked cold single-flight, actual router admission/body drop, old-versus-renewed idle expiry, caller-waker observation and bounded startup/reap scenarios; pool group 31 passed |
| A10, B11, B12, B14 | Identity/tenancy/control | Real isolation/order effects, split command cases, fixed inputs, retire weak random/default claims | Done: real maintenance/request isolation, shared recovery trace, split fixed-input commands, persisted OIDC replay and actual readiness routes; independent review findings addressed |
| A03, B10 | Platform time policies | Controlled time at owning interface; observable retention/refill; retire representation-only checks | Done: limiter 12 and model runtime 11 passed; ignored-clock/old-deadline mutations rejected; default Tokio timer separately proves actual resource release |
| A07 progress, A08, B13, B15 | Tools/reporting | Captured public output and actual capability effects; correct request-ID contract; explicit runtime integration where effects remain real | Done: captured reporter output; actual persistence plus start/completion ordering; inline/stored extraction in separate process targets; numeric ID padding/progression across 9999/10000; integration classification |
| A05 | xtask runner | Unique owned fixtures, bounded children, real first-failure short-circuit effects | Done: owned fixtures, marker-based short-circuit, direct-child deadline/reap, independent-offset output snapshots; inherited output cannot block the runner; xtask 17 passed |
| A06 | Release/evaluation artifacts | Exact archive contents, fallible reads, pre-rename old-final preservation—not crash-durability claims | Done: exact members/bytes/digests and pre-rename failure preservation; positive independent source review |
| B16 corpus/models | Evaluation | Correct integration labels, publication content/integrity/fetch avoidance, explicit fixture-dependent execution | Done: artifact/corpus integration, six public fixture-builder absent/incomplete no-fetch scenarios; ten named real-model cases restored and explicitly unrun |
| B17, P3 CLI/shared/UI | Auxiliary interfaces | Split public-interface cases; remove helper/default/pass-through redundancy after preserving meaningful behavior | Done: environment adapter integration, public CLI behavior, shallow helper/constructor checks retired; three-page UI refresh preserves both back transitions; UI 98 passed |
| Architecture checks | Checker tooling | Finish existing delimiter repair; retain as lints/checker evidence, not product unit coverage | Done: shared opaque-group turbofish scanner, cfg signature/item extents; scanner mutation rejected; public/trait checker targets 27 + 12 passed; lint evidence only |
| Full gates and review | Parent integration | Targeted evidence, full runs, independent review, truthful audit/plan reconciliation | Done: final gates below; independent scoped reviews completed, follow-up worker failures repaired and self-reviewed by parent |

## Final verification

```sh
cargo fmt --all --check
cargo check --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
cargo test -p memory_mcp --no-default-features --test fs_watch_process_disabled --locked
cargo run --locked -p xtask -- check-observability
```

The whole-suite commands are executed serially with a private temp root until
every legacy fixture is known to own its paths. Optional model/platform cases
that cannot run are reported as unrun, not passing.

## Completion contract

Every audit row has a tested repair or an explicit justified classification/
retirement. Integration evidence is not lost while units become isolated.
Passing Cargo targets or a larger test count is not a unit-compliance
certificate; no inventory/status-table test is added. Any remaining semantic
coverage gap is recorded rather than hidden behind a green aggregate.

## Parent verification checkpoint

This evidence applies to the integrated snapshot before the remaining
model-retention and denied-resolution changes, not to a finished plan:

- The default workspace library/binary/integration run completed successfully.
- Both required Clippy profiles completed with `-D warnings`; the expanded
  workspace all-target check also passed.
- Ingestion persistence and duplicate preservation are now separate focal
  integration scenarios; all five ingestion cases passed.
- The expanded HTTP run exposed a test deadline that equalled the production
  activation backoff. Its factory-entry wait now allows the backoff plus a
  bounded scheduling margin. The exact regression passed after the change.
- On the subsequent expanded run, the library group passed: 2,352 passed and
  four ignored. Many integration targets passed, but the overall command reached
  its 200-second process limit during `service_integration`; this is incomplete
  suite evidence, not a full-profile pass.
- The no-default-feature filesystem process group passed both cases.
- `xtask check-observability`, formatting and whitespace checks passed.
- A separate single-threaded expanded-profile `service_integration` run passed
  all 51 cases. This does not replace the incomplete full-profile run.

Final gates must be repeated after the remaining changes and reviews land.

### Build-evidence integrity

The denied-resolution worker ran a source-copy mutation with the shared Cargo
target directory. Cargo subsequently reused the mutant artifact for an
original-source run. That original-source failure is contaminated evidence,
not a production regression. The worker is coordinating package-scoped cleanup
and an original-source rebuild. Final gates must run after restoration;
source-copy mutations must use a distinct `CARGO_TARGET_DIR`.

The worker subsequently reported package-scoped cleanup and a rebuilt original
`memory_consumer_ports` run with four passing cases. Its source change and
sensitivity evidence still require parent inspection after the isolated changes
are delivered; this report alone does not close A02 or the final gates.

After delivery, the parent inspected the actual persistent-resolver bridge and
owner read, labelled the scenario as integration, and reran the final reduced
target: two cases passed. Recorder-only ingestion and object-safety smoke cases
were retired in favour of the existing actual ingestion scenarios.

### Independent review follow-up

The auxiliary reviewer confirmed the xtask group passes but found that the
observability runner joins pipe readers without a deadline. A descendant
inheriting stdout/stderr can therefore keep `run` blocked after its immediate
Python child exits or is killed. A05 remains open until the inherited-pipe case
and resource cleanup are repaired and checked. The reviewer's eval/UI build
reached its process limit, so it adds no test-pass evidence for those targets.

The reviewer also identified B16 coverage lost during test retirement:
the ignored real-model quality run now selects only one of the original ten
corpus cases, and the corrupt-model test does not cover the public fixture
builder's absent/incomplete-checkpoint no-download behavior. Both are assigned
for restoration as actual input/model behavior, not document inventory.
Exact corpus publication/cache/hash assertions, pre-rename artifact failure
preservation and same-cursor UI back-navigation received positive source review.

The reviewer's lower-priority UI history gap was repaired in the parent:
the refresh scenario now starts on page three and follows both previous
transitions back to page one, asserting each returned cursor/page. Its targeted
UI test and the workspace format check passed. The production equality rule did
not need another change.

The HTTP reviewer found further incomplete lifecycle oracles: the body-drop
case constructs a standalone gate/body rather than exercising HTTP middleware
lease attachment; idle retention does not distinguish an old deadline from a
renewed deadline; and startup has no alive/no-address child scenario. The
capacity-wakeup test also records producer activity without proving that the
waiter observed the wakeups. These are assigned for public-path remediation;
the corresponding HTTP rows remain open. Positive source review covered cold
single-flight identity, persisted OIDC replay, recovery trace order and real
health routing. The reviewer's compilation timeout supplies no test evidence.

The standards reviewer found an unrelated extract metric label change from
`links` to `items`; the parent restored `links` to preserve the published metric
contract. Reporting event order and request-ID progression/format are still
under remediation: merely finding start/done lines or one positive number is
not evidence for those rules. The reviewer confirmed that the CAS decorator
intercepts both conditional-query and legacy full-row writes, and classified
the real tools/reporting scenarios as integration evidence.

The parent strengthened the disabled-generation case with a clock adapter that
fails if consulted: the public `generate_and_update` scenario passes without
reading the write clock. Its targeted test and formatting check passed. The
checker owner received the reviewer's lexical-duplication suggestion; sharing
is conditional on the two scanners actually having identical semantics.

The model-runtime patch was delivered and inspected. Production construction
uses the same monotonic scheduling seam as controlled retention tests, while
public method signatures remain unchanged. The parent added bounded watchdogs
to scheduler acknowledgements, loader ordering and permit waits so a broken
scenario fails rather than hanging. All ten focused cases passed again and
formatting passed. The old-deadline mutation was rejected by the worker.
Inference timing/scheduling remains runtime evidence; the controlled retention
cases do not claim coverage of Tokio's actual timer adapter.

### Provider-failure recovery checkpoint

The core, observability and model-fixture follow-up workers failed before
successful delivery. Their assignments were not counted complete. The parent
continued directly:

- The access-write interleaving now commits an external count increment as well
  as the vector replacement before the focal write; the owner read observes
  count two and the replacement signature. The targeted scenario passed.
- The observability runner captures output in owned temporary regular files and
  reads a bounded byte snapshot after the immediate checker finishes. There are
  no output-reader threads or pipe EOF joins left. A coordinated descendant
  fixture proves inherited output does not block `run`; all five runner
  scenarios and xtask Clippy passed. The fixture descendant is explicitly
  released and its completion observed. This is not a general descendant
  process-tree termination guarantee.
- All ten real-model corpus scenarios were restored as separately named,
  ignored tests. The target compiled and reported ten ignored cases; no actual
  inference or model download was performed. Fixture-builder gating remains
  open.
- The process-liveness probe now has a two-second child deadline and treats
  ambiguous Unix failure as unknown rather than death. Standard missing-process
  diagnostics preserve dead-owner reclamation. The real current-process and
  reaped-child scenarios passed, as did all ten lease cases. Windows execution
  remains unverified.
- Inspection confirmed candidate metadata sampling is synchronous before the
  first suspension; the explicit initial poll therefore establishes sample
  order. All nine existing filesystem candidate scenarios passed. No claim is
  made that they are isolated units.
- Production library Clippy passed with `-D warnings`; final workspace gates
  are still pending.

## Final acceptance

This section supersedes the open states in the chronological checkpoints above.
Every A01–A10, B01–B17 and P3 action now has a repair or an explicit
classification/retirement in the completion ledger. The architecture follow-up
plan's reopened cold-single-flight, CAS-loser and delimiter findings are closed.

The remaining HTTP/reporting workers stopped at provider limits. Their partial
changes were inspected, not blindly merged; the parent completed:

- Actual router response admission retention and recovery on body drop.
- Public warm reacquisition across original/renewed idle deadlines.
- Public guard releases that wake the caller's actual waker, followed by
  explicit re-polls that cannot extend the capacity deadline.
- A live child without a startup address; deadline rejection checks the child
  actually ran and no longer exists after kill/reap.
- Separate inline/stored extraction process targets and correlated ordered
  start/completion events; resolve additionally checks numeric progression.
- The process-global request source's exact padding and width transition
  (`req_0001`, `req_0002`, `req_9999`, `req_10000`), without string ordering.
- Local checkpoint-root injection at the real fixture-builder composition
  interface. Six separately named absent/incomplete cases run in fresh child
  processes with an observed local proxy/endpoint and no observed fetch.
  Existing default-root callers delegate through the same builder.
- Shared const-generic call scanning and cfg-gated item extent handling.
  The first paired-comparison fixture did not reject the scanner mutation;
  independently named `<` and `>` cases replaced that weak oracle. The isolated
  no-group-skipping mutation then failed and the real checker targets passed.
- The default Tokio timer's actual model-resource release, separate from
  controlled retention policy units.
- Independent output-reader offsets in the observability runner and a real
  dead-PID observation after direct-checker timeout.

### Final commands and results

Run serially on the final source with private temp roots for the suites;
`--test-threads=1` is the explicit conservative integration execution profile.
These are mixed unit/integration/lint totals, **not compliant-unit counts**.

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | Pass |
| Required workspace all-target Clippy, `fs-watch,mcp-apps,streamable-http`, `--locked -- -D warnings` | Pass |
| Expanded workspace all-target Clippy, also `test-fixtures` | Pass |
| Expanded workspace all-target `cargo check --locked` | Pass |
| Default workspace `--lib --bins --tests --locked -- --test-threads=1` | 114 result groups; 2,384 passed, 0 failed, 37 ignored registrations |
| Expanded memory profile `--lib --bins --tests --locked -- --test-threads=1` | 102 result groups; 3,075 passed, 0 failed, 30 ignored registrations |
| No-default `fs_watch_process_disabled` | 2 passed |
| `cargo run --locked -p xtask -- check-observability` | Pass |
| `git diff --check` | Pass |

### Review and execution limits

Three independent scoped reviews supplied the recorded standards/spec findings.
The parent repaired them and inspected the subsequent source changes; there was
no additional independent post-fix approval after provider limits were reached.
Passing checks are evidence, not a substitute for that distinction.

Real optional GLiNER/VAGO/ONNX inference and non-host platform execution remain
unrun; model downloads were not performed. The ten restored quality cases allow
the same scored `QualityFailed` outcome as before, not a new quality guarantee.
Some ignored helper registrations are deliberately executed by their parent
integration scenarios and are not skipped behavior.

Atomic replacement still does not promise crash durability. The observability
runner bounds its immediate checker and output snapshot; it does not promise
general process-tree supervision of arbitrary descendant programs. Environment,
filesystem, real SQL, timers, global identity/log sinks and source checkers
remain honestly classified rather than relabelled as isolated units. The
pre-remediation audit is preserved as a snapshot and no document-inventory
test or hand-maintained status-table assertion was introduced.
