# Evaluation profiles

## Current memory-quality evidence

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
has complete scoring coverage on seven verified questions and upstream
accuracy of 0/7. All seven prompts lost the memory context at the token
cap. Broader agent utility and benchmark coverage remain open. See the
[follow-up design and tasks](../superpowers/specs/2026-11-24-agent-memory-follow-up-design.md).

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

The separate `extract` fact-evidence response defaults to bounded evidence;
its compact-response opt-out and narrow deterministic no-regression record are
documented in the
[`response-default report`](../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

`memory_decisions_v4.json` deliberately pins `shadow`: its frozen v4 acceptance
plan expects that historical configuration. Current product-default profiles
use `claim_rollout_stage: "default"`; do not let an old profile inherit the
new default implicitly.

Every committed evaluation is run through a profile. The profile fixes the
selected suites, expected coverage, time budget, gates, and (where applicable)
the corpus root. Schema-v2 artifacts reject missing, duplicate, or unexpected
suite-scoped cases.

| Profile | Command | Scope | Status |
|---|---|---|---|
| `pr` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/pr.json --artifact target/evals/pr.json` | local retrieval, extraction, claims, external mini-corpus | blocking |
| `release` | `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/release.json --artifact target/evals/release.json` | PR suites plus lifecycle checks | blocking |
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
