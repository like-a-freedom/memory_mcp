# Evidence-stage agent pilot (2026-11-22)

## Purpose and scope

This is an exploratory usability pilot of the already accepted `shadow` versus
`evidence` product configuration. It does not modify the memory implementation
or establish a population-level agent benefit. The deterministic product gate
and its accepted v9 result are documented separately in
[`claim-rollout-current-master-2026-11-22`](../claim-rollout-current-master-2026-11-22/README.md).

The frozen agent plan contains 14 independent history families: six
`ordinary_workflows` and eight `temporal_decisions`. It has no agent-managed
memory writes, so formation/maintenance costs are outside this pilot's
estimand. Each of two unsandboxed local repetitions used the frozen product
binary and the same paired plan. The agent adapter used the existing local
Codex login; no separate API key was needed. Deterministic scoring and report
validation themselves require no model account.

## Valid artifacts

- [`plan.json`](plan.json) and [`impact-rationale.json`](impact-rationale.json)
  are the preregistered inputs. They were not altered after observations.
- [`results/run-1-unsandboxed/`](results/run-1-unsandboxed/) and
  [`results/run-2-unsandboxed/`](results/run-2-unsandboxed/) are the two valid
  14-family paired runs.
- [`results/exploratory-pilot-summary-v1.json`](results/exploratory-pilot-summary-v1.json)
  is the stage-aware report. A second invocation wrote
  [`results/exploratory-pilot-summary-v1-repeat.json`](results/exploratory-pilot-summary-v1-repeat.json)
  with the same SHA-256, proving byte-identical offline summarization.
- Independent memory probes passed 14/14 in both arms in both repetitions
  (56/56 observations). These are fixed-history source-evidence scores and are
  distinct from agent-observed `direct_memory_success`.

The stale v2 preflight and any sandbox attempt that failed before an agent turn
are invalid and excluded. The recalibrated v3 preflight passed 28/28
standalone arm-probe checks; it is a runner preflight, not a product-effect
observation.

## Results

| Repetition | Arm | Agent-observed direct memory | Task success | Hard prohibited-evidence violations |
|---|---|---:|---:|---:|
| 1 | incumbent / shadow | 13/14 | 11/14 | 1 |
| 1 | candidate / evidence | 13/14 | 11/14 | 1 |
| 2 | incumbent / shadow | 14/14 | 12/14 | 0 |
| 2 | candidate / evidence | 13/14 | 11/14 | 1 |

All three hard violations occurred on `unknown-target` in the
`unknown-abstention-history` family and were classified as
`prohibited_evidence`. This pilot does not reinterpret them as temporal leakage.

Paired task-success and agent-observed direct-memory deltas were zero in run 1
and −1/14 (−7.14 percentage points) in run 2. Across repeats, the 14
family-level paired means are −0.5 for that one abstention family and zero for
the other 13 families (−3.57 percentage points overall). Repair-turn gain was
zero in run 1 and −0.5 turns in run 2. Candidate input-token and latency
savings were positive descriptively in both runs, but their 95% family-cluster
bootstrap intervals crossed zero. Neither task utility nor resource evidence
supports a candidate benefit claim.

The report estimates average agent-observed direct-memory success at 96.43%
for incumbent and 92.86% for candidate; the maximum remaining incumbent
headroom is 3.57 percentage points. The frozen worthwhile direct-memory
margin is +10pp, and its target alternative is +25pp, so both are impossible
at this observed baseline. The incumbent task-success mean is 82.14%, making
the frozen +25pp task-success alternative impossible as well. Therefore no
valid joint-power sample size or confirmatory run can be derived from these
frozen targets. No fixed-agent confirmation was run and no usability or
promotion claim is made. This is a no-go for that confirmation design, not
proof of equivalence or proof that the treatment has no effect.

## Reproduction and integrity

```sh
cargo run -p eval-harness --locked --bin memory-eval -- summarize-pilot \
  --plan evals/paired/evidence-agent-pilot-2026-11-22/plan.json \
  --first-run-directory evals/paired/evidence-agent-pilot-2026-11-22/results/run-1-unsandboxed \
  --second-run-directory evals/paired/evidence-agent-pilot-2026-11-22/results/run-2-unsandboxed \
  --output /absolute/path/to/new-report.json
```

The report schema is `memory-mcp-agent-exploratory-pilot-report/v1`. It
reproduces paired family deltas and retest variance separately for incumbent
and candidate outcomes. It has no promotion field. The existing development
pilot report schema v3 and its same-product calculation remain unchanged.

| Artifact | SHA-256 |
|---|---|
| `plan.json` | `493d4a460f794c0be6e0e20f5d8f6f14f670d0f8d43c3130f8da6df604a817cb` |
| `impact-rationale.json` | `66e855ff3b189b9baaef9985a02134abb9d790f40159902432f2222c5ed9c708` |
| `product-binary.sha256` | `62ef80a120cb11631e49a400cfed762d7ae09a1b60a00537321902959c01e3df` |
| `results/exploratory-pilot-summary-v1.json` and repeat | `5a9a39dffe20d2409291889ae60525add949f48ff9a74ba8d5ee89ec7e10b984` |
