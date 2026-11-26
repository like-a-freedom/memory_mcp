# Deterministic memory quality — 2026-11-21

> **Current gate, 2026-11-26:** The historical rollout evidence below remains
> archived. Routine acceptance now runs one full primary capture and compares
> each case/dimension/reason with an independently reviewed immutable baseline.
> The final Apple Silicon run had 0 regressions, 5 passed, 3 known quality failures
> and 0 invalid/unobserved cases. See the [current runbook](README.md#fast-deterministic-development)
> and [verified capture and review](../../evals/paired/harness-simplification-current-2026-11-26/README.md).

Status: development and frozen-candidate confirmation both accepted. This report concerns deterministic public-MCP memory behavior in explicitly selected temporal/evidence mode. It does not claim improved agent productivity or an official external-benchmark score.

> **Evaluator and C3 update (2026-11-25):** A TDD review found that the
> historical absent-subject score depended on one exact phrase in `guidance`.
> The normalizer now classifies a structurally valid, successful empty result
> as `unknown` regardless of guidance prose; no read response is `unobservable`,
> and malformed responses remain invalid input. The paired runner also unwraps
> standard JSON-RPC `result` and adapter `payload` envelopes before classifying
> read evidence. Content-only replies remain unknown. The product rollout gate
> was rerun against the native arm64 binary: default scored 5/8 decisions and
> 8/8 evidence, shadow scored 1/8 decisions and 8/8 evidence, repeated receipts
> matched, and there were no decision losses or evidence regressions. The
> separate C3 query-boundary cohort passed 4/4 twice; a 252-event historical
> scale control passed 3/3 twice with matching decision replays. These are
> finite deterministic outcomes, not population or agent-utility claims. See
> the [C3 characterization](../../evals/paired/c3-history-characterization-2026-11-25-v6/report.md),
> [scale registration](../../evals/paired/c3-historical-scale-2026-11-25/registration.json),
> [rollout acceptance](../../evals/paired/native-quality-candidate-2026-11-25/claim-rollout-default-acceptance.json),
> and [qualification plan](../superpowers/plans/2026-11-24-memory-evaluation-qualification-implementation-plan.md).

> **Later rollout update (2026-11-21):** This report's statement that the
> default remained `shadow` was accurate at its capture. The product default
> has since become `evidence`; see the
> [same-binary rollout comparison](../../evals/paired/claim-rollout-current-master-2026-11-22/README.md).
> Historical metrics below remain tied to their original frozen binaries and
> profiles.

> **Rollout verification update (2026-11-22):** `evidence` was already the
> product default; this audit tested whether its safety case was adequate. The
> v7 repeated capture exposed an absent-subject false positive in episode
> fallback. A public regression reproduced it, and a full product test run then
> caught an ordinary conversation-neighborhood regression in the first fix.
> The fallback was narrowed to records with a declared structured subject,
> preserving normal episode admission. The preregistered v9 same-binary gate
> accepted the final candidate: evidence stayed 8/8 in both arms, decisions
> improved from 1/8 in `shadow` to 5/8 in `evidence`, with four gains, zero
> losses, zero evidence regressions, and zero candidate safety findings. The
> full `memory_mcp` test suite, full `eval-harness` suite, strict workspace
> clippy and formatting check passed. This is a finite synthetic cohort, not an
> agent benefit or population claim. Full identities, receipts, preserved
> failures and limits are in the
> [v9 rollout record](../../evals/paired/claim-rollout-current-master-2026-11-22/README.md).
> A second preregistered replay on the final verification build also passed on
> the same v9 cohort. The older source-authority v3 profile was repeated on
> the final default-feature binary and scored 2/4 primary; corrected v4 later
> used fixed past timestamps and scored 4/4 candidate against a repeated 3/4
> baseline. Neither finite fixture comparison establishes source-authority
> usability or non-inferiority.

> **Current-master replay (2026-11-23):** After the bounded fact-evidence
> response changes, the frozen v9 gate passed again on binary
> `58695c4ab2f0da8f5897bb677e51ebb48247d993ccffd6207b42aad77a09c231`.
> Default claim-reconciliation `evidence` scored 5/8 decisions and 8/8
> evidence; `shadow` scored 1/8 decisions and 8/8 evidence. Repeated captures
> matched, with four gains, no losses, and no candidate safety findings. This
> is a regression check for the claim-reconciliation stage, not an evaluation
> of `extract.include_fact_evidence`; its one-run agent diagnostic and no-go
> result are in the [v5 response report](../../evals/paired/agent-managed-maintenance-v5/README.md).

> **Current product gate rerun (2026-11-23):** The current binary
> (`0c465a019c824602e1adc02c37b97f93ff3a6a5b98226d3007b7c3d93c990f25`, schema
> `9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`) passed
> the registered two-repeat default-versus-shadow gate: 5/8 vs 1/8 decisions,
> 8/8 evidence in both profiles, four gains, no losses or evidence regressions,
> no hard failures, and exact scored repeatability. The acceptance report and
> all four captures are in
> [`extract-evidence-default-2026-11-23`](../../evals/paired/extract-evidence-default-2026-11-23/README.md).
> This validates current claim-reconciliation behavior, not the response-only
> `extract` default; its response-specific deterministic preflight is reported
> separately.

> **Fact-evidence response feasibility update (2026-11-23):** The subsequent
> disjoint v6 agent cohort completed two preregistered captures and replayed
> both comparisons byte-identically. Candidate formation fit its context
> budget in 16/16 phases versus 0/16 for the incumbent, used fewer tool calls,
> returned evidence before invalidation in 16/16 cases, and passed all 16
> candidate independent probes. However, candidate task correctness was 7/8
> in the second capture against the frozen 8/8 gate, so the result is **NO-GO**
> for promoting the response workflow based on agent utility. The later
> bounded product-response default is a separate safety decision. These are repeated
> runs on eight families, not sixteen independent cases, and they measure
> workflow feasibility rather than a stored-memory-quality treatment effect.
> The run-02 failing receipt marks both direct-memory success and task-oracle
> success false, while the fixed post-write probe passes. Receipts retain only
> payload hashes, so this record cannot distinguish the agent's retrieval
> result from answer formulation or establish a product defect.
> This no-go is separate from the accepted deterministic claim-reconciliation
> default evidence stage above. See the [v6 report](../../evals/paired/agent-managed-maintenance-v6/README.md).

> **Bounded response-default verification (2026-11-23):** The separate
> `extract.include_fact_evidence` response now defaults to true under the
> all-or-none 4 KiB serialized-addition limit; callers can request compact
> output with `include_fact_evidence=false`. Eight isolated operator-import
> fixtures returned complete default evidence and passed independent source
> and value probes (8/8); maximum serialized result was 1,023 bytes. A fixed
> public retrieval request produced the same normalized result fingerprint in
> two pre-default and two default-on captures. This verifies the tested
> response contract and a narrow retrieval no-regression. It does not
> supersede v6's no-go for agent-workflow promotion, prove broad agent utility,
> or claim improved persisted-memory quality. See the
> [response-default report](../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

> **Post-default regression controls (2026-11-23):** On the same dirty-tree
> product build (`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`),
> the registered retrieval profile passed 37/37 with all six metrics at 1.0;
> claim reconciliation passed 41/41 with F1, precision and recall at 1.0. The
> corrected source-authority v4 profile ran twice, passed all four primary
> cases in both captures, and its replay comparator matched the captures. The
> nested temporal subscore remains 3/4 and is reported explicitly. Raw reports,
> evaluator/profile hashes, receipts and reproduction commands are in the
> [regression-controls record](../../evals/paired/extract-evidence-default-2026-11-23/regression-controls/README.md).
> These are unchanged-product controls on fixed fixtures: they show no observed
> regression in these profiles, not a response-default treatment effect,
> source-authority usability, or broad memory/agent improvement.

## Protocol and identity

The archived incumbent was captured on authorized dirty master at `1ae1da4a4e3120f2de94753fc1a949caee3aced7`, with the pre-existing temporal implementation. It is not a pristine upstream checkout. Its executable SHA-256 is `f3b079afd89889151f34b20ee6a28ae06cf06df7c3bf1a98a6214aeaff085751`. The final reviewed candidate is `150fb81d21832e674d61fe2d7c8326e82c68958820b506f4576338edb22e7146`. The [candidate freeze](../../evals/paired/deterministic-quality-2026-11-21/reviewed-candidate-freeze.json) pins all 425 production Rust source files and records that confirmation outcomes were unopened. Portable raw receipts, normalized indexes and executable digests are the repository evidence.

[Preregistration](../../evals/paired/deterministic-quality-2026-11-21/preregistration.json) froze v4 development and v5 confirmation before memory tuning. Each has 8 cases/history families and two ordinary controls. Subjects, values and histories differ across cohorts; their synthetic scalar templates overlap. The frozen rule requires at least +25 percentage points of equal-family complete-decision success, evidence nonregression for every case, ordinary-decision nonregression, at most 5 output items, exact semantic repeat equality, fresh raw capture for every repeated case, full coverage and zero hard safety violations. Repeats add no independent examples. A bootstrap interval is descriptive; finite-cohort acceptance does not make a population claim.

Every capture forms a fresh embedded store through public `ingest`/`extract`, with regex extraction and embeddings disabled, then queries fixed valid time and server-observed revision checkpoints. Oracles never enter the product. The registered loader verifies raw event order/payloads, seed identities, extraction/checkpoint requests, actual question, budget, snapshot, registered private oracle and corpus/configuration/public-schema hashes; it re-normalizes and re-scores raw responses. This detects detached results, accidental capture reuse and self-consistent changes to the evaluated question. Hashes are not signatures proving honest execution by an adversarial external runner.

All frozen baseline/corpus/plan checksums were verified unchanged. Earlier v1–v3 corpora, malformed preflights and historical diagnostic artifacts remain untouched; they are not confirmation evidence. The earlier missed pilot-before-product ordering is not retroactively repaired by these runs.

## Results

| Cohort | Incumbent complete | Candidate complete | Evidence incumbent → candidate | Wins / losses | Hard violations in candidate | Verdict |
|---|---:|---:|---:|---:|---:|---|
| v4 development, 2 captures per arm | 1/8 | 7/8 | 6/8 → 8/8 | 6 / 0 | 0 | Accepted |
| v5 confirmation, 2 captures per arm | 1/8 | 7/8 | 6/8 → 8/8 | 6 / 0 | 0 | Accepted |

The development gain is +75 percentage points; descriptive family bootstrap 95% interval is [37.5, 100] percentage points (8 clusters, seed 20261121, 10000 draws). [Actual CI-gate acceptance report](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v4-ci/acceptance.json), [first raw replay](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v4-ci/first/index.json), [second raw replay](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v4-ci/second/index.json).

Confirmation: [acceptance report](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v5-acceptance.json), [first replay](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v5-1/index.json), [second replay](../../evals/paired/deterministic-quality-2026-11-21/reviewed-v5-2/index.json). Gain 75 percentage points; descriptive 95% family interval [37.5, 100] percentage points. Both independent captures agree semantically, with distinct per-case raw receipts. There were no losses, hard violations or invalid cases. The candidate and all 425 frozen source hashes were unchanged before opening confirmation outcomes.

At the historical capture, `absent-subject` returned correct empty evidence from
a partial snapshot, while the evaluator classified the decision as
unobservable because it relied on guidance prose. The 2026-11-25 normalizer
correction makes this response shape score as unknown, without asserting global
absence. The tool still discloses omitted unversioned channels and the snapshot
remains partial. The frozen historical score is unchanged; the later v9 public
gate passed for its separately registered finite configurations, as recorded
above. That result does not retroactively rescore this historical suite. There
is no claim of 100% complete-decision success.

## Rollout-stage scope

At the time this report was written, the +75 pp complete-decision gain was measured for explicit temporal requests with `MEMORY_CLAIM_ROLLOUT_STAGE=evidence`, and the product default remained `shadow`. In both historical cohorts the ordinary shadow/default diagnostic stayed 1/8 complete and 5/8 evidence. Historical shadow evidence improved 6/8→8/8, but complete decisions stayed 0/8 because claim roles were not exposed. These channels are recorded separately and are not replaced with the successful evidence-stage score. The standalone `run` command therefore reported quality failure for its legacy primary channel; the registered candidate acceptance CLI reported success for its explicitly preregistered channel. There is no model-mediated agent-benefit claim. The later default-stage comparison is recorded in [the current rollout archive](../../evals/paired/claim-rollout-current-master-2026-11-22/README.md).

## Mechanisms and controls

| Development stage | Complete | Evidence | Incremental complete change | Interpretation |
|---|---:|---:|---:|---|
| Frozen incumbent | 1/8 | 6/8 | — | Fails new acceptance |
| Subject admission | 1/8 | 8/8 | 0 | Removes forbidden evidence; rejected for no decision gain |
| Bounded revision-visible fact pool | 6/8 | 8/8 | +5/8 | Passes +25 pp rule against immediate subject incumbent |
| Explicit canonical subject identity | 7/8 | 8/8 | +1/8 | Positive correctness gain, below separate +25 pp promotion margin |
| Reviewed final candidate | 7/8 | 8/8 | No additional cohort points | Closes adversarial regression gaps; overall candidate qualifies against frozen baseline |

Registered re-evaluations: [subject no-gain](../../evals/paired/deterministic-quality-2026-11-21/subject-vs-baseline-verified.json), [fact pool](../../evals/paired/deterministic-quality-2026-11-21/fact-pool-vs-subject-verified.json), [identity incremental threshold rejection](../../evals/paired/deterministic-quality-2026-11-21/identity-vs-fact-pool-verified.json). Rejected intermediate mechanisms are not presented as independently promoted features. Historical first-generation acceptance reports are preserved with their original semantics; the registered-loader reports are the stronger current verification.

The complete candidate retains the identity correction because public regression tests reproduce false conflicts, including an ordinary extraction that creates a fuzzy alias before explicit-subject extraction. It is a correctness fix within the accepted package, not an independently powered product-effect claim. Review-driven regressions also cover subject mentions in values, distinct numeric identifiers, multi-subject questions and future FTS statistics changing an earlier fixed-revision ranking. Historical ranking no longer consumes mutable global BM25 statistics. Explicit subjects resolve by canonical normalized name before any alias lookup. Existing misattributed historical claims are not repaired automatically.

Unchanged native controls were run after each mechanism and on the reviewed product: [retrieval 37/37](../../evals/paired/deterministic-quality-2026-11-21/reviewed-retrieval/run.json) and [claims 41/41](../../evals/paired/deterministic-quality-2026-11-21/reviewed-claims/run.json), zero invalid and quality failures. Retrieval uses pinned local Candle embedding and reranking models, not the lexical-only diagnostic configuration. There is no external model account or API key requirement.

The [unchanged-product negative control](../../evals/paired/deterministic-quality-2026-11-21/reviewed-no-gain-control.json) exits 1 with zero wins and three safety violations under the strengthened rules. The actual ignored CI gate was also executed against the old product and failed on quality, then executed against the final product and passed (411.87 seconds). Positive/negative CLI fixtures separately prove exit 0/1/2 semantics. A missing rich observation cannot fall back to a legacy successful score.

Full local command summaries and evaluator source fingerprints are archived in [reviewed-verification.json](../../evals/paired/deterministic-quality-2026-11-21/reviewed-verification.json).

## Verification and CI

Latest local verification (2026-11-23): `cargo test -p memory_mcp --locked` passed 2,773 tests with 45 ignored across 166 targets; `cargo test -p eval-harness --locked` passed 691 with 22 ignored across 31 suites. The first managed run could not start macOS FSEvents in three watcher cases; the explicitly authorized laptop rerun passed the full production suite, including the watcher target (14/14). Strict workspace Clippy passed with `fs-watch,mcp-apps,streamable-http,test-fixtures` and `-D warnings`; `cargo fmt --all --check` and `git diff --check` passed. These counts describe the actual feature profiles, not an assertion that all ignored/platform/model-dependent tests ran.

Strict workspace clippy passed with `fs-watch,mcp-apps,streamable-http,test-fixtures` and `-D warnings`; format checking passed. Real integrated independent probes were executed separately against the public product; changing agent read count cannot change their fixed-probe score. Standalone stdio and HTTP transport captures are archived in [independent-probe evidence](INDEPENDENT_MEMORY_PROBES.md).

The `evaluation` job now builds the public executable, explicitly runs `deterministic_quality_gate` with a fresh output directory, and uploads the current raw capture and its scoped regression report even on failure. Historical repeated rollout captures are opt-in. The exact gate has been proven locally on this laptop. A hosted Linux CI run has not been triggered or claimed.

After ADR-0084 changed the public `explain` schema, the native gate
replayed all 32 registered v9 trials and retained the same acceptance
outcome. Both modes also passed the 62-case public response-size profile.
[Acceptance and replay data](../../evals/paired/native-rollout-v9-explain-2026-11-24/report.md)

To re-evaluate a built product without an agent or provider account:

```sh
cargo build -p memory_mcp --bin memory_mcp --features streamable-http --locked
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_QUALITY_GATE_OUTPUT="$PWD/target/new-deterministic-memory-run" \
cargo test -p eval-harness --test deterministic_quality_gate --locked -- --ignored
```

The output path must be fresh. For a historical promotion experiment with existing repeated captures use `memory-eval accept-memory-candidate --corpus-root <registered-directory> --plan <frozen-plan> --incumbent-first <index> --incumbent-second <index> --candidate-first <index> --candidate-second <index> --output <new-report>`. Successful standalone diagnostic execution is distinct from promotion; legacy/default channels may fail while the registered rich channel passes. Invalid execution never qualifies.

For a new two-build temporal comparison, freeze
`"cross_build_binary_sha256": {"incumbent": "<sha256>", "candidate": "<sha256>"}`
with distinct lowercase SHA-256 digests. Each arm must match its registered
binary across both captures. Use `product_binary_sha256` for a shared build
pin. The frozen older registrations still replay with their original schema.

## Limits and remaining work

These results prove improvement on the registered finite histories and deterministic controls. They do not prove arbitrary-language/entity resolution, general absence, production-scale recall or latency, semantic historical selectors, model-mediated agent benefit, authenticated source authority or an official LongMemEval-V2 score. A bounded 250-fact supplement can still miss older facts outside direct FTS hits; truncation is disclosed. Lowercase/ambiguous subjects retain the prior lexical policy.

The [temporal compatibility contract](../compatibility/TEMPORAL_CONTEXT.md) documents store-local revisions, omitted legacy provenance, server-owned transaction time, unsupported mixed-version writers and restore requirements. The original agent-outcome protocol still requires its own powered, genuinely independent-family confirmation and resource guards. Source-authority transport, watcher, diagnostic redaction and extraction-retry contracts now have targeted product tests; corrected v4 source recall passes its four registered candidate cases against a repeated 3/4 baseline. Legitimate-workflow non-inferiority and agent usability remain open. These source-authority changes do not add to the accepted memory-quality score. Retention, repair/index experiments and official external-benchmark lifecycle remain conditional/separate tasks. See the [follow-up design](../superpowers/specs/2026-11-24-agent-memory-follow-up-design.md).
