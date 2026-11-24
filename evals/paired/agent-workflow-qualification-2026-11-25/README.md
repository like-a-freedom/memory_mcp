# Ordinary agent-workflow qualification — 2026-11-25

**Decision: NO-GO.** This completes the registered ordinary-workflow
development screen only. The typed-contradiction, agent-managed correction,
and operator-import populations were not run. The screen does not establish a
memory treatment effect: incumbent and candidate used the same product binary.

## Question and frozen gates

Can the frozen local model and provider-neutral runner carry out ordinary
memory-backed tasks, while the memory itself passes independent fixed-query
checks? The frozen denominator is four distinct synthetic families, captured
twice. Each capture required at least 3/4 candidate task successes, 3/4 actual
agent memory-read successes, all 4 independent probe successes, zero hard
violations, and no budget or timeout exits.

The freeze is recorded in [`plan.json`](plan.json), [`tasks.json`](tasks.json),
[`probes.json`](probes.json), [`criteria.json`](criteria.json), and
[`freeze-manifest.sha256`](freeze-manifest.sha256). The manifest pins the raw
file digests; `qualification.json` records the normalized plan, manifest,
probe, criteria and capture-artifact digests used by the qualifier. Both arms
used memory binary SHA-256
`3bb3c321e640d3205d938cedc0b9f34db6cea0f3203281854b931b0d435234ea` from
revision `3f1c829939f1759ef1be24eb782ced5f66e3177d`.

## Execution and results

The run was on native Apple Silicon (`aarch64-apple-darwin`). Agent requests
went to a direct local MLX service at `http://127.0.0.1:18767/v1`, using the
registered `LFM2.5-1.2B-Instruct-MLX-8bit` model, temperature zero, one-call
tool budget, three-turn budget, 384 output-token cap, and 25-second trial cap.
No Codex executable, account, credential, LM Studio app, LM Studio server, or
`lms` command was used. The model weights happened to be stored under a cache
directory named `.cache/lm-studio`; that directory name is not evidence that
LM Studio ran. The provider configuration and evaluator binary are identified
by the fingerprints in `plan.json`. The plan identifies the model by local
model alias/path rather than a separately verified weight digest, so exact
weight provenance is a remaining reproducibility limitation.

| Capture | Arm | Task success | Agent memory reads | Independent probes | Tool calls | Hard violations / budget exits | Model failure | Mean / p50 / max trial latency | Capture elapsed |
|---|---|---:|---:|---:|---:|---:|---|---:|---:|
| 01 | incumbent | 0/4 | 0/4 | 4/4 | 0 | 0 / 0 | `final_answer_not_json` 4/4 | 356.8 / 312.5 / 502 ms | 121.259 s |
| 01 | candidate | 0/4 | 0/4 | 4/4 | 0 | 0 / 0 | `final_answer_not_json` 4/4 | 331.5 / 316.5 / 394 ms | 121.259 s |
| 02 | incumbent | 0/4 | 0/4 | 4/4 | 0 | 0 / 0 | `final_answer_not_json` 4/4 | 313.0 / 298.5 / 366 ms | 118.827 s |
| 02 | candidate | 0/4 | 0/4 | 4/4 | 0 | 0 / 0 | `final_answer_not_json` 4/4 | 341.8 / 341.5 / 384 ms | 118.827 s |

The model completed one turn per trial but did not produce the required final
answer JSON in any of the eight trials per capture. It made no memory tool
calls. The repeated failure and the zero direct-memory denominator make this
a runner/model response-contract failure, not a measured memory defect.
Independent probes passed 4/4 in both arms and both captures, showing that the
fixed read path can retrieve the registered records under this product build;
they cannot substitute for a real agent read or task pass.

The machine-readable [qualification report](qualification.json) is NO-GO in
both captures because the task-success and direct-memory-read floors failed.
The [pilot summary](development-pilot-summary.json) is descriptive and does
not convert the same-binary comparison into a product effect. The raw paired
artifacts, independent envelopes and receipts are in [`capture-01/`](capture-01/)
and [`capture-02/`](capture-02/).

## What this result does and does not authorize

- Keep the ordinary workflow unqualified and do not enroll a confirmatory
  cohort.
- Do not change memory retrieval based on these agent trials: no agent read it.
- Do not run the other three C1 populations yet. First diagnose and validate a
  model/runner response-format contract using fresh synthetic diagnostics;
  then freeze a new qualification registration before any further captures.
- The earlier transport/schema smoke proves only that the adapter can exchange
  a tool call with a compatible local server. It does not prove task-level
  usability.
- The deterministic qualification implementation is provider-neutral and
  offline scoring needs no model service or credentials. See the
  [paired evaluation contract](../../../docs/evals/PAIRED_AGENT_EVAL.md).

The preflight capture whose launch profile omitted required stdio runtime
variables is archived under `attempts/preflight-launch-profile-mismatch-01/`
and is excluded from the two valid captures.
