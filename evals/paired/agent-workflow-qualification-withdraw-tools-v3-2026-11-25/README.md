# Ordinary agent workflow after tool-budget enforcement — v3

**Decision: ELIGIBLE for a separately frozen study of this exact configuration.**
This two-capture screen does not estimate a memory treatment effect. The
incumbent and candidate used the same memory binary, product configuration and
claim stage.

## Frozen design

The preregistered cohort contains four fresh synthetic ordinary-recall
families, repeated in two captures. Each capture required candidate task
success at least 3/4, actual candidate memory reads at least 3/4, independent
probes 4/4, zero hard violations and zero budget or timeout exits. Both
captures had to pass. The frozen inputs and exact rules are in
[`preregistration.md`](preregistration.md), [`plan.json`](plan.json),
[`tasks.json`](tasks.json), [`probes.json`](probes.json),
[`criteria.json`](criteria.json), and [`freeze-manifest.sha256`](freeze-manifest.sha256).
All four manifest checksums passed before the captures. Re-running the offline
qualifier on the captured artifacts produced a byte-identical report.

## Local execution

The run used native Apple Silicon (`aarch64-apple-darwin`), direct
`mlx_lm.server` 0.32.2 on loopback, and local Qwen3.8-27B MLX 4-bit weights.
The runtime was started directly with `HF_HUB_OFFLINE=1`; no LM Studio app,
server or CLI was used. No Codex executable, account, API key, bearer token,
remote model or network retrieval was used. The weights were stored under a
cache directory whose name contains `lm-studio`; that storage path does not
mean the LM Studio runtime was involved. All task, evidence and memory content
was synthetic.

The model is identified by its local alias/path, and its weight digest was not
available. The result therefore qualifies only this model alias and runtime
configuration, not an immutable model artifact or another provider.

## Results

| Capture | Arm | Task oracle | Actual agent reads | Independent probes | Tool calls | Hard violations | Budget/timeout exits | Mean / p50 / max trial latency |
|---|---|---:|---:|---:|---:|---:|---:|---:|
| 01 | incumbent | 4/4 | 4/4 | 4/4 | 4 | 0 | 0 | 8,002 / 7,677 / 10,135 ms |
| 01 | candidate | 4/4 | 4/4 | 4/4 | 4 | 0 | 0 | 8,505 / 8,571 / 10,852 ms |
| 02 | incumbent | 4/4 | 4/4 | 4/4 | 4 | 0 | 0 | 7,860 / 7,451 / 9,889 ms |
| 02 | candidate | 4/4 | 4/4 | 4/4 | 4 | 0 | 0 | 7,878 / 8,191 / 9,027 ms |

Every scored task returned the exact accepted answer, cited the required
evidence, matched the clarification oracle and used exactly one public memory
read. Independent fixed-query probes passed separately; they are not counted
as agent reads or task outcomes. The gate report records both candidate
captures as eligible with no failed gates. The
[`pilot-summary.json`](pilot-summary.json) is descriptive: its paired latency
intervals include zero in both captures, so the small sample does not establish
a latency gain or regression.
The independent source-coverage scores for all four arms/captures are saved
as [per-probe scorer outputs](independent-probe-scores/README.md).

## Agent-control diagnosis and change

The earlier ordinary screen failed because the local model emitted
`final_answer_not_json` and made no memory read. A subsequent frozen v2
diagnostic showed that sending `tool_choice: "none"` after the one-call budget
was exhausted was insufficient: the provider still returned another tool
call, which the runner correctly rejected as a budget exit in all four
candidate trials.

The minimal runner fix omits `tools`, `tool_choice`, and
`parallel_tool_calls` from follow-up requests after the registered tool-call
budget is consumed. Before the budget is consumed, the normal tool schema and
`tool_choice: "auto"` remain present. A focused regression first failed on the
old request shape, then passed after the change. The contract suite passed 26
tests with one opt-in local smoke ignored; the qualification CLI test passed,
formatting passed, and the required workspace Clippy command passed.

The v3 screen therefore clears the ordinary-workflow gate for this exact
configuration. It does **not** establish general agent usability, a memory
quality improvement, a treatment effect, or readiness of the other C1
populations. Typed contradiction, agent-managed correction and operator-import
populations remain to be frozen and evaluated separately. Do not reuse these
four exposed ordinary families as fresh evidence.

## Artifacts

- `capture-01/` and `capture-02/`: paired observations, independent probe
  envelopes and tool receipts.
- [`qualification.json`](qualification.json): frozen offline acceptance
  decision and input/artifact fingerprints.
- [`pilot-summary.json`](pilot-summary.json): descriptive paired summaries,
  not a promotion decision.
- Earlier diagnostic records: [initial response-format NO-GO](../agent-workflow-qualification-2026-11-25/README.md),
  [v1 invalid schema-fingerprint preflight](../agent-workflow-qualification-after-budget-fix-2026-11-25/README.md),
  and [v2 `tool_choice: none` NO-GO](../agent-workflow-qualification-after-budget-fix-v2-2026-11-25/README.md).
