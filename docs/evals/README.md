# Evaluation profiles

Release recovery: [2.0.2](RELEASE_2_0_2.md). Previous native qualification and evidence limits: [2.0.0](RELEASE_2_0_0.md).

## Fast deterministic development

The current regression route uses one primary capture per registered case and
needs no agent, model host, or API key. The independently reviewed baseline is frozen; this route is the active
`deterministic_quality_gate` used by CI. Historical rollout profiles remain explicit diagnostics.

```sh
cargo build --locked -p memory_mcp --bin memory_mcp
cargo build --locked -p eval-harness --bin memory-eval
# Use a fresh destination for every capture and grading attempt.
export MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp"
export MEMORY_DECISIONS_PRODUCT_REVISION="$(git rev-parse HEAD)"
export MEMORY_DECISIONS_ARTIFACT_ROOT="$PWD/target/evals/current-01/receipts"
mkdir -p target/evals/current-01
# Exit 1 records quality failures; exit 2 is an invalid execution.
target/debug/memory-eval run --profile evals/profiles/memory_decisions_current.json \
  --artifact target/evals/current-01/run.json
# Regrade saved raw evidence; this command never starts memory or an agent.
target/debug/memory-eval compare-current-memory \
  --baseline evals/paired/current-contract-baseline-2026-11-26/receipts/index.json \
  --candidate target/evals/current-01/receipts/index.json \
  --corpus evals/corpora/memory-decisions-v10 \
  --output target/evals/current-01/grade-01.json
```

A scoped no-regression result can pass with existing failures. Always inspect
`all_cases_pass`, per-case dimensions/reasons, `counts`, `current_failures` and
`regressions` separately. Grade receipts bind exact input-index hashes, the compiled
scorer/normalizer/verification sources and the grader executable. The reviewed
corpus, configuration and public schema must match; product identity may change.
The active v10 corpus separates ordinary retrieval from explicit temporal snapshots.
Current grading rejects expected temporal roles on ordinary reads, including legacy
v1 oracles. Historical v9 replay remains available through the archived loader;
its three `decision_unobservable` results are oracle-scope errors, not memory bugs.
See [scope correction and native evidence](../../evals/paired/current-contract-baseline-2026-11-26/README.md).

The command rederives scores from raw receipts and registered inputs, including
primary and temporal dimensions and new failure reasons. It refuses incomplete
coverage, missing evidence and incompatible stages. It never refreshes the baseline.

For an inner-loop reproduction, copy the current profile under `target/evals/`
and set its memory-decisions `case_ids` to the named registered cases; adjust
`expected_coverage.exact_cases` accordingly. A subset cannot replace the full gate.
Set `diagnostics: true` explicitly to capture verbose/shadow and evidence-stage
contrasts. Omitted diagnostics means primary-only; historical diagnostic
profiles explicitly retain their original diagnostics. Primary-only receipts use v5; v3/v4 loaders remain
necessary to regrade archived evidence. Skipped diagnostics are unmeasured,
not a zero success rate.

Resume at the failed stage using immutable inputs and a fresh attempt path.
A failed `compare-current-memory` produces an invalid grading receipt. Retry as
`grade-02.json`; `grade-01.json` cannot be overwritten, and all attempts must be
retained. No capture is repeated for a scorer failure. For paired agent evidence,
use the existing `score-paired-observations` command with its original plan,
tasks, observations and a fresh `--output-directory`. Its `attempt.json` retains
input hashes, grader identity and scored/invalid status even when validation fails;
partial outputs are never deleted or accepted as a complete pair. Changed upstream identities
require a new upstream capture; an old context is never a candidate execution.
There is no automatic best-attempt selection or reliability claim from a retry.
Partial memory captures retain all scheduled outcomes; diagnose selected failed
cases in a fresh directory, then run the full gate for acceptance.

Explicitly public synthetic Chat Completions tasks may opt into
`--public-synthetic-traces <fresh-directory>`. Sidecars retain the actual tool
arguments and replies, including omitted versus null selectors, and bind the
plan/product/schema/case/arm/phase. Sidecars are committed after each phase
returns, not after every call: a process crash can lose the unfinished phase.
`call_failed` means no transport response; MCP/JSON-RPC operation errors remain
in the raw reply and must be inspected there. Private runs do not enable this flag.
`PublicSyntheticTrace::replay_for` executes exact requests only against a
caller-supplied isolated session with the registered identity. It does not
reconstruct a lost store or establish fresh quality/latency evidence.

See the [matched simplification capture](../../evals/paired/harness-simplification-current-2026-11-26/README.md)
for timings, independent review, final native-gate evidence, retained failures and limitations. The Rust budget starts at `Runner::run`: CLI profile loading, corpus preparation
and binary hashing precede that timer. It is not a bound on total CLI wall time.
Python bounds each child invocation; copying is measured separately and is outside
that bound.

Routine development does not
repeat the seven LongMemEval questions or a model experiment unless a named
empirical question requires it.

## Current memory-quality evidence

The [November 26 repair report](MEMORY_QUALITY_REPAIRS_2026-11-26.md)
is the current consolidated interpretation. Matched extraction on the same v2
labels improved from 5/9 to 9/9 cases (precision .65 to 1.00, recall 1.00 in
both). The source-linked packing reproductions improved from 0/2 to 2/2 complete
blocks. These are finite development repairs, not benchmark-wide gains. Stronger
public CLI controls reject wrong numeric assertions, changed repeated reads,
incomplete/extra sets and ungrounded quotes. The eight-case native memory gate
passes. Reader and autonomous selector workflow outcomes, retained failed
registrations, resource limits and next actions are recorded in that report.

The [phase-deadline follow-up](../../evals/paired/phase-deadlines-2026-11-26/README.md)
adds real stdio cancellation, retained failed/completed reads and valid persisted
timeout artifacts without a model. This bounds the agent phase, not store setup.
The [full-snapshot diagnosis](../../evals/longmemeval_v2/results/2026-11-26-full-history/README.md)
confirms both reviewed source carriers exist (2/2), but both reads exceeded the
external resource limit. A native sample localizes expensive lexical FTS sorting;
no completed full-history contexts or new answer scores were obtained.

The [preceding audit](MEMORY_QUALITY_CLOSURE_2026-11-26.md) retains the historical
v1 extraction score of 4/9 (precision .60, recall 1.00, F1 .75). Its v1 and the
new v2 denominator are not interchangeable. The seven frozen long-history
answers remain 2/7 under strict reviewed truth; earlier 3/7 replay scoring
accepted an unsupported extra list member. No new full-history reader capture
was performed by the packing repair. This is a diagnostic slice, not an
official aggregate score.

The registered [deterministic candidate gate](DETERMINISTIC_MEMORY_QUALITY.md)
passed the development and separate confirmation histories. The
[v10 default comparison](../../evals/paired/memory-decisions-v10-baseline-2026-11-23/README.md)
scores default `evidence` at 8/8 primary decisions versus 2/8 under `shadow`,
with sufficient evidence at 8/8 in both arms. The
[post-default controls](../../evals/paired/extract-evidence-default-2026-11-23/regression-controls/README.md)
cover claims, retrieval and source authority.

The [typed-conflict agent pilot](../../evals/paired/typed-conflict-agent-pilot-2026-11-24/README.md)
stopped at its registered ordinary-task gate. The
[source-authority workflow screen](../../evals/paired/source-authority-codex-gpt62-sol-qualification-2026-11-24/runs/run-02/report.md)
qualified one exact configuration on four synthetic families. The
[LongMemEval-V2 Small web text baseline](LONGMEMEVAL_V2_SMALL_WEB_BASELINE.md)
has a seven-question audited slice, not a full benchmark score. The repeated
[post-B5 comparison](../../evals/longmemeval_v2/results/2026-11-24-b5-post-fix/REPEATED_COMPARISON.md)
measured context retention, while the November 25 paired B5/C3 recheck used an
identical concise-answer prompt and local standalone MLX-LM for both arms. Its
pinned scorer was 1/7 for B5 and 3/7 for C3; a supplemental strict list metric
was 0/4 and 2/4, respectively. This prompt-adapted diagnostic is not an
official LongMemEval score or a population-level quality claim. The full
[paired recheck report](../../evals/longmemeval_v2/results/2026-11-25-c3-final-query-fix/RECHECK.md)
records per-case evidence, scoring limits and latency. It supports a narrow
improvement on two list questions, while false-premise answers remain wrong
and the seven one-shot latency observations do not establish a speedup. See the
[follow-up design and tasks](../superpowers/specs/2026-11-24-agent-memory-follow-up-design.md).

The initial ordinary local [agent-workflow screen](../../evals/paired/agent-workflow-qualification-2026-11-25/README.md)
was a NO-GO: independent probes passed 4/4, but the model did not return the
required final-answer JSON in any agent trial, so it made no memory reads and
task success was 0/4 in both captures. After adding a tested tool-budget
control, the fresh [v3 ordinary workflow screen](../../evals/paired/agent-workflow-qualification-withdraw-tools-v3-2026-11-25/README.md)
passed both captures: task, actual-read and independent-probe scores were each
4/4 per arm and capture, with zero violations or budget exits. This qualifies
only that runner/model configuration for a separately frozen study. It is not
a memory benefit estimate. The latest [C1 cross-capture review](../../evals/paired/c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/CROSS_CAPTURE_REVIEW.md)
is NO-GO for workflow qualification: capture 01 passed 4/4 exact cited tasks
in each same-product arm, but the single preregistered repeat fell to 0/4 in
both arms. Independent probes, exact-once corrections and invalidations, and
pre-invalidation evidence checks stayed 4/4. Capture 02 had six empty reads
per arm over three tasks; valid-time requests and returned snapshots matched
on every read. The per-field selector report identifies frequent `known_at`
and `compact` mismatches, but these also appeared in successful capture 01 and
the receipts intentionally do not retain raw selector values. One incumbent
read returned sufficient evidence but was not used in the final answer; the
candidate's only nonempty read lacked sufficient evidence and recorded one
prohibited-evidence violation. The failed repeat shows run-to-run
agent-workflow instability on these four synthetic families. It does not
identify a causal memory defect or estimate a memory treatment effect. The
earlier [C1 v6 report](../../evals/paired/c1-agent-managed-correction-recovery-v6-valid-time-2026-11-25/capture-01/REPORT.md)
remains the historical valid-time screen. Separate typed-conflict and
operator-import screens were also completed and failed their 3/4 task gates:
typed conflict scored 0/4 in both arms, while operator import scored 0/4 in
both arms and required 31 candidate formation calls versus 8 for incumbent.
These are workflow NO-GO results, not evidence of a memory treatment effect.
See the [qualification plan](../superpowers/plans/2026-11-24-memory-evaluation-qualification-implementation-plan.md#task-c1-qualify-workflows-under-independent-gates)
for per-screen reports and denominators.

The [A8 evidence-action hint screen](../../evals/paired/a8-evidence-action-hint-2026-11-25/README.md)
also found no task-success gain (2/3 per arm) on three synthetic families.
The evaluator-only hint improved clarification matching for an empty read, but
the candidate still missed the exact answer. This result does not justify a
product response change.

### Keeping evaluation evidence

Keep registered inputs, raw receipts needed for offline scoring, accepted and
negative-control results, normalized reports and the associated identities.
Write new scratch runs and locally built executables under `target/evals/`.
Historic executable digests identify those captures. A new local build is
expected to have a different digest. Old host paths document the original
capture environment; use the Cargo commands in this guide to reproduce it.

The original checksum lists for the
[first 16-family Codex screen](../../evals/paired/source-authority-current-codex-qualification-2026-11-23/runs/run-01/)
and [Qwen v4 registration](../../evals/paired/source-authority-local-chat-completions-v4-qualification/)
contain digests of Markdown before later notes were added. The lists retain
their historical values. Use the JSON receipts and archived scorer outputs
for replay; those two Markdown entries do not attest the current prose.

Current claim-reconciliation behavior defaults to `evidence`; set
`MEMORY_CLAIM_ROLLOUT_STAGE=shadow` explicitly for the rollback/control arm.
Deterministic memory evaluation runs directly against the local product binary
and needs no Codex executable, account, or API key. The current eight-case
same-binary stage comparison, acceptance plan, preregistration, and raw receipts
are archived in
[`claim-rollout-current-master-2026-11-22`](../../evals/paired/claim-rollout-current-master-2026-11-22/README.md).

Current deterministic memory-quality results: [DETERMINISTIC_MEMORY_QUALITY.md](DETERMINISTIC_MEMORY_QUALITY.md). Independent probes and artifact compatibility: [INDEPENDENT_MEMORY_PROBES.md](INDEPENDENT_MEMORY_PROBES.md). The finite-cohort memory gate is separate from optional model-mediated agent confirmation.

Current product regression controls after the bounded extract-evidence default
are archived in
[`extract-evidence-default regression controls`](../../evals/paired/extract-evidence-default-2026-11-23/regression-controls/README.md):
claim reconciliation 41/41, retrieval quality 37/37, and two matching
source-authority v4 captures. These are fixed-fixture controls, not an effect
estimate for the response-default change.

The latest local Apple Silicon `pr` profile run is archived at
[`2026-11-26-pr-local-candle`](../../evals/results/2026-11-26-pr-local-candle/README.md).
It passed 150/150 cases and all 10 configured gates. Only three gates had
baseline values; the external answer-presence and six retrieval-quality gates
were checked against their hard floors. The archived report explains the
baseline cohort mismatch and does not claim a full-cohort no-regression result.

The separate `extract` fact-evidence response defaults to bounded evidence;
its compact-response opt-out and narrow deterministic no-regression record are
documented in the
[`response-default report`](../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

`memory_decisions_v4.json` deliberately pins `shadow`: its frozen v4 acceptance
plan expects that historical configuration. Current product-default profiles
use `claim_rollout_stage: "default"`; do not let an old profile inherit the
new default implicitly.

Rust suite evaluations run through a profile. The profile fixes the
selected suites, expected coverage, time budget, gates, and (where applicable)
the corpus root. Schema-v2 artifacts reject missing, duplicate, or unexpected
suite-scoped cases.

Bounded reader and temporal-workflow diagnostics instead freeze their own
plan, task/source truth, prompts, product/model identities and stop rule before
capture. `python3 evals/paired/memory_quality_diagnostic.py --help` exposes
provider-neutral `freeze`, `capture` and `score` commands; the optional model
endpoint must be numeric loopback. It requires neither Codex nor LM Studio.
Its source-handle mode maps **all** delivered sources, never hidden correct
labels. `source_span` checks a full registered proposition within an exact
quote from both its cited raw source and delivered evidence. Model-call
eligibility, per-socket I/O timeout and MCP call timeout are distinct; none
should be described as a whole-capture hard deadline.

`python3 evals/paired/temporal_read_audit.py --tasks <tasks.json> --capture
<capture-directory> --traces <trace-directory>` supplements the authoritative
Rust scorer for the fixed temporal-development protocol. It checks both arms,
actual queries/selectors linked to scored reads, exact observed coverage,
sources and observed resource eligibility. It does not cancel executing MCP
calls and is not a general configurable replacement for paired scoring.
The current workflow registration separately bounds whole execution. Regrade
archived evidence with its archived script and registered inputs; use fresh
destinations for new attempts.

| Profile | Command | Scope | Status |
|---|---|---|---|
| `pr` | `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/pr.json --baseline evals/baselines/one-active-namespace-pr.json --artifact target/evals/pr.json` | local retrieval, extraction, claims, external mini-corpus, native retrieval-quality | blocking |
| `release` | `cargo run -p eval-harness --locked --bin memory-eval -- run --profile evals/profiles/release.json --baseline evals/baselines/one-active-namespace-release.json --artifact target/evals/release.json` | PR suites plus lifecycle checks | blocking |
| `nightly` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/nightly.json --artifact target/evals/nightly.json` | scheduled breadth | blocking when scheduled |
| `memory_decisions` | Set `MEMORY_DECISIONS_SERVER_BIN`, `MEMORY_DECISIONS_PRODUCT_REVISION`, and `MEMORY_DECISIONS_ARTIFACT_ROOT`, then run `cargo run -p eval-harness --bin memory-eval --locked -- run --profile evals/profiles/memory_decisions.json --artifact /tmp/memory-decisions.json` | pinned public-MCP diagnostic for late corrections, conflicting sources, temporal limits and abstention | manual/nightly diagnostic; deliberately gate-free and not blocking |
| `memory_decisions_v10` | Run `memory_decisions_v10_default.json` and `memory_decisions_v10_shadow.json` twice each with distinct artifact roots; compare each arm with `compare-decision-replays` | eight new synthetic families; current/historical/conflicting roles are graded only on explicit valid-time + known-revision snapshots; ordinary retrieval remains a separate no-role control | repeated deterministic baseline: default 8/8 primary decisions and 7/8 temporal decisions vs shadow 2/8 and 1/8; evidence 8/8 in both. Finite fixture result, not powered agent benefit; see [`MEMORY_DECISION_BASELINE.md`](MEMORY_DECISION_BASELINE.md#corrected-temporal-role-baseline-v10-2026-11-23) |
| `memory_decisions_temporal_heldout` | Use the same environment as `memory_decisions`, a fresh receipt directory for each run, and `evals/profiles/memory_decisions_temporal_heldout.json` | nine disjoint temporal/conflict histories, one probe per `family_id` | exploratory diagnostic only; two runs replayed identically on 2026-11-21, but only 1/9 evidence-stage probes passed and it is not promotion evidence; see [`MEMORY_DECISION_BASELINE.md`](MEMORY_DECISION_BASELINE.md) |
| `memory_decisions_source_authority_v4` | Use the same environment as `memory_decisions`, run from the repository root, and select `evals/profiles/memory_decisions_source_authority_v4.json` | literal file/URL references, ordinary retrieval control, unknown-reference abstention with corrected past timestamps | repeated current-product fixture result: 4/4 primary cases, with a visible 3/4 temporal subscore; workflow usability/non-inferiority remains open; see [`current regression controls`](../../evals/paired/extract-evidence-default-2026-11-23/regression-controls/README.md) and historical [`source-authority-2026-11-22`](../../evals/paired/source-authority-2026-11-22/README.md) |
| paired agent evaluation | `export-paired-inputs` and `score-paired-observations`, then `summarize-pilot`, `compare-paired-diagnostic`, or confirmatory `compare-paired` | deterministic scoring of recorded trials from any compatible runner, using a private oracle and family-cluster intervals | optional agent evidence. Memory decisions run deterministically without an agent or harness; see [`PAIRED_AGENT_EVAL.md`](PAIRED_AGENT_EVAL.md) for the runner-neutral format and optional live Codex adapter |
| `response_size` | `make eval-response-size` | compact vs verbose `assemble_context` and `explain` serialization | diagnostic; execution integrity blocking |
| `ner_quality` | `make eval-ner-quality` | five extractor backends on the committed fixture | manual/platform-aware |
| `perf_weekly` | `make perf-weekly` | ingest/assemble latency and throughput gates vs the committed baseline | manual, local machine only |
| `external_*` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/external_<name>.json --artifact target/evals/external_<name>.json` | one pinned classic corpus per profile | requires prepared corpus |

The `pr`, `release`, `nightly`, and `external_*` profiles have no Make
target — run the `cargo run` command above directly. CI inlines the same
invocation.

### Reproduce the native PR profile locally

The `pr` and `release` profiles include `retrieval-quality`, which requires the
two checksum-pinned local models documented in the
[native retrieval-quality guide](../../evals/corpora/retrieval-quality-v1/README.md).
The harness runs them through the Rust `local-candle` provider; it does not
require LM Studio, an LLM service, or an API key. Model preparation downloads
the pinned weights once unless verified bundles are already cached:

```sh
cargo run -p eval-harness --locked --bin memory-eval -- prepare-model --manifest evals/models/multilingual-e5-small.json --output-root target/eval-models
cargo run -p eval-harness --locked --bin memory-eval -- prepare-model --manifest evals/models/mmarco-minilm.json --output-root target/eval-models
```

On macOS/Linux, set the native provider configuration and run the profile with
its registered regression baseline:

```sh
export EMBEDDINGS_ENABLED=true
export EMBEDDINGS_PROVIDER=local-candle
export EMBEDDINGS_MODEL=intfloat/multilingual-e5-small
export EMBEDDINGS_MODEL_DIR="$PWD/target/eval-models/intfloat--multilingual-e5-small/614241f622f53c4eeff9890bdc4f31cfecc418b3"
export RETRIEVAL_RERANKER_MODEL_DIR="$PWD/target/eval-models/cross-encoder--mmarco-mMiniLMv2-L12-H384-v1/1427fd652930e4ba29e8149678df786c240d8825"
export RETRIEVAL_RERANKER_ADMISSION_FLOOR=0
export MEMORY_CLAIM_ROLLOUT_STAGE=evidence

mkdir -p target/evals
cargo run -p eval-harness --locked --bin memory-eval -- run \
  --profile evals/profiles/pr.json \
  --baseline evals/baselines/one-active-namespace-pr.json \
  --artifact target/evals/pr.json
```

PowerShell uses the same profile and model bundles; only environment-variable
syntax differs:

```powershell
$modelRoot = Join-Path $PWD.Path 'target/eval-models'
$env:EMBEDDINGS_ENABLED = 'true'
$env:EMBEDDINGS_PROVIDER = 'local-candle'
$env:EMBEDDINGS_MODEL = 'intfloat/multilingual-e5-small'
$env:EMBEDDINGS_MODEL_DIR = Join-Path $modelRoot 'intfloat--multilingual-e5-small/614241f622f53c4eeff9890bdc4f31cfecc418b3'
$env:RETRIEVAL_RERANKER_MODEL_DIR = Join-Path $modelRoot 'cross-encoder--mmarco-mMiniLMv2-L12-H384-v1/1427fd652930e4ba29e8149678df786c240d8825'
$env:RETRIEVAL_RERANKER_ADMISSION_FLOOR = '0'
$env:MEMORY_CLAIM_ROLLOUT_STAGE = 'evidence'

New-Item -ItemType Directory -Force -Path 'target/evals' | Out-Null
cargo run -p eval-harness --locked --bin memory-eval -- run `
  --profile evals/profiles/pr.json `
  --baseline evals/baselines/one-active-namespace-pr.json `
  --artifact target/evals/pr.json
```

The recorded local run was verified on Apple Silicon; CI runs the profile on
Linux. The commands are Rust/Cargo based and avoid a vendor-specific runner,
but this report does not claim a Windows execution result.

Response-size reports separate `assemble_*` and `explain_*` metrics. It is
intentionally gate-free: the design target is diagnostic, while missing or
empty response classes are invalid execution and fail the run.

The latest temporal candidate paired-agent feasibility result is recorded in
[`TEMPORAL_CANDIDATE_AGENT_EVAL.md`](TEMPORAL_CANDIDATE_AGENT_EVAL.md). It is
exploratory and does not support promotion or an agent-usability claim.

The latest agent-managed maintenance workflow (v6) used a disjoint eight-family
cohort and two repeated captures. Candidate formation stayed within budget in
16/16 phases, exact correction writes and independent probes passed, and the
candidate used 24 tool calls per capture versus 32 for the incumbent. The
candidate task gate still failed at 7/8 in the second capture (required: 8/8 in
each); the repeated families are not sixteen independent cases. This is a
feasibility **NO-GO**, not a memory-quality or agent-benefit result. The bounded
`extract` evidence response was later made the product default under separate
product-contract and deterministic no-regression checks; that does not reverse
the v6 agent-workflow decision. See the [v6 report](../../evals/paired/agent-managed-maintenance-v6/README.md)
and [response-default report](../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

## Deterministic memory decisions

Start here for late corrections, contradictory sources and abstention. The
suite starts the local `memory_mcp` binary and sends scripted MCP requests to
fresh stores with fixed inputs and a code-based oracle. It needs no agent
runner, model account or API key.

```sh
cargo build --locked -p memory_mcp --bin memory_mcp

run_root="$(mktemp -d)"
export MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp"
export MEMORY_DECISIONS_PRODUCT_REVISION="$(git rev-parse HEAD)"
for run in first second; do
  status=0
  MEMORY_DECISIONS_ARTIFACT_ROOT="$run_root/$run" \
    cargo run --locked -p eval-harness --bin memory-eval -- run \
    --profile evals/profiles/memory_decisions.json --artifact "$run_root/$run.json" || status=$?
  [ "$status" -le 1 ] || exit "$status"
done
cargo run --locked -p eval-harness --bin memory-eval -- compare-decision-replays \
  --left "$run_root/first/index.json" --right "$run_root/second/index.json"
```

PowerShell equivalent (Cargo selects the `.exe` files on Windows):

```powershell
cargo build --locked -p memory_mcp --bin memory_mcp
$runRoot = Join-Path $env:TEMP ("memory-decisions-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $runRoot | Out-Null
$env:MEMORY_DECISIONS_SERVER_BIN = Join-Path $PWD "target\debug\memory_mcp.exe"
$env:MEMORY_DECISIONS_PRODUCT_REVISION = (git rev-parse HEAD).Trim()

foreach ($run in @("first", "second")) {
  $artifactRoot = Join-Path $runRoot $run
  $artifact = Join-Path $runRoot "$run.json"
  $env:MEMORY_DECISIONS_ARTIFACT_ROOT = $artifactRoot
  cargo run --locked -p eval-harness --bin memory-eval -- run `
    --profile evals/profiles/memory_decisions.json --artifact $artifact
  if ($LASTEXITCODE -gt 1) { exit $LASTEXITCODE }
}

cargo run --locked -p eval-harness --bin memory-eval -- compare-decision-replays `
  --left (Join-Path $runRoot "first\index.json") `
  --right (Join-Path $runRoot "second\index.json")
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Set the product revision to the build's actual source label when testing
another revision or local changes. Each index also records the binary hash.
Use distinct receipt directories and the same build and profile for repeat
runs. Exit 1 means the run completed with quality failures and still wrote
its artifacts. The comparison checks case-level results, including normalized
source order, roles, temporal observations and scores. It ignores receipt
locations and hashes that include per-run details. Check the score artifacts
for failed probes and diagnostic gaps even when the replays agree. For the v2
and v3 profiles and measured baseline failures, see
[the memory-decision baseline](MEMORY_DECISION_BASELINE.md).

## Independent fixed-query probes

`capture-independent-probe` (stdio/HTTP) and `score-independent-probe` provide standalone, runner-free capture and offline source-coverage scoring. The paired integration and deterministic acceptance gates are implemented; their source-coverage metric remains distinct from decision quality and agent usability. See [the v1 contract and limitations](INDEPENDENT_MEMORY_PROBES.md).

## Ingestion source authority

The source-authority profile runs a fixed public-MCP history against the
checked-in fixture path. Run it from the repository root so the child process
resolves the same relative fixture path. It has no model, Codex, account, or
network dependency. The pre-fix result, previous candidate, and current
post-lineage candidate repeats are archived in
[`source-authority-2026-11-22`](../../evals/paired/source-authority-2026-11-22/README.md).
The current profile remains below acceptance: literal source-reference recall
fails in the ordinary primary channel even though the raw episode persists and
the separate temporal diagnostic can surface it. Do not interpret this
security hardening as an agent-usability or memory-quality gain.

The optional model-backed migration feasibility protocol is separate from this
deterministic profile. Its v2 plan and limits are recorded in
[`source-authority-agent-workflow-v2`](../../evals/paired/source-authority-agent-workflow-v2/README.md).
There are now accepted paired pilot artifacts, but no model/configuration has
passed its registered source-authority qualification floor. The local
LFM2.5-1.2B pilot and its disjoint diagnostic each scored 0/4 task-oracle
successes per arm; Qwen3.8-27B v4 scored 0/4 per arm, and corrected-sampler
Qwen v5 scored incumbent 1/4 versus candidate 0/4. The Codex literal-input
diagnostic scored 1/4 per arm. These are model/workflow screens,
not memory treatment estimates: the relevant pilots used the same memory
binary in both arms. See the [LFM report](../../evals/paired/source-authority-local-lfm25-pilot-2026-11-23/run-01-report.md),
[LFM diagnostic](../../evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/post-capture-audit.md),
[Qwen report](../../evals/paired/source-authority-local-chat-completions-v4-qualification/runs/run-02/report.md),
[Qwen corrected-sampler report](../../evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/runs/run-01/report.md),
and [Codex diagnostic](../../evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/runs/run-01/report.md).
Do not reuse their exposed task families or claim memory improvement from
these results. Contract tests use a fake loopback server and need no model
provider; no deterministic test depends on LM Studio. See the runner-neutral
[observation contract](PAIRED_AGENT_EVAL.md).

Later local screens also failed their frozen task gates: Qwen v6, Flint, Qwen
Splash, Saluki v2, and the Qwen speed-optimized configuration each scored 0/4
per arm. Qwen speed candidate independent probes reached 4/4 versus 1/4
incumbent, but accepted-answer match stayed 0/4 and candidate source-file task
success was 0/2. Its capture took about 7m12s, which is not a controlled
cross-cohort speed comparison. LFM's model calls took about 5.1s total, but
task success was 0/4 per arm in both pilot and disjoint diagnostic; use it for
provider/tool smoke only. See [paired results](PAIRED_AGENT_EVAL.md).

A later corrected-sampler Ornith 1.5 35B A3B screen scored task-oracle 0/4
incumbent and 2/4 candidate; independent probes were 1/4 and 3/4. Candidate
file tasks and imports/extractions passed 2/2, with zero hard violations or
budget overruns. The screen failed its frozen task/probe gates. Six formation
failures were normalized as `transport`, whose specific cause is unavailable;
this is a NO-GO for the tested configuration, not evidence of a memory effect
or model-wide failure. See the [run report](../../evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/runs/run-01/report.md)
and [paired evaluation notes](PAIRED_AGENT_EVAL.md).

## External corpora

The manifests in `evals/corpora/` pin URL, immutable revision, checksum, byte
size, row count, license, adapter version, and any auxiliary files required by
the adapter (PersonaMem uses its pinned CSV plus JSONL context file).
Preparation validates every file, stages atomically, and never overwrites a
previously valid revision. The external suite reports
`answer_presence_proxy_at_5`, a weak lexical diagnostic; it is not document
recall or answer correctness. A corpus-backed benchmark is not release
evidence until its pinned data has been prepared and its artifact is available.

## Platform benchmarks

`make bench-check` compiles all Criterion targets. `make bench-cpu` measures
CPU targets and requires model fixtures. `make bench-metal` is only meaningful
on Apple Silicon; set `MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1` to fail closed when
the Metal model asset is absent.

LongMemEval-V2 remains text-only and uses the pinned adapter in
`evals/longmemeval_v2/`. Run `run_pinned.sh --smoke-only` or
`--integration-smoke BINARY` for contract checks; these are not an official
leaderboard score. The script fails closed without an explicit mode, and the
official upstream launcher must be present before publishing such a result.

The current NER floors and their review rationale are recorded in
[`NER_BASELINE_REVIEW_2026-09-05.md`](NER_BASELINE_REVIEW_2026-09-05.md).

## Perf

Perf gates live outside CI by design (ADR-0082): `make perf-weekly` compares
the weekly profile against
[`evals/perf/baselines/perf-weekly-local.json`](../../evals/perf/baselines/perf-weekly-local.json)
on the maintainer's local machine, and dated artifacts are archived under
[`evals/perf/results/`](../../evals/perf/results/). Commands, the host-tag
policy, artifact layout, and the baseline-update ritual are documented in
[`docs/performance/PERF_HARNESS.md`](../performance/PERF_HARNESS.md).
