# Typed Conflict Agent Benefit Development Pilot

**Status:** two valid synthetic paired captures completed and deterministically scored. The frozen ordinary-control gate failed, so the preregistered confirmatory run was not authorized by the pilot criteria. This closes the pilot decision as NO-GO; it is not a powered agent-benefit result.

This exploratory, all-synthetic pilot checks whether typed evidence-stage annotations help an agent safely handle conflicting memories under a one-read budget. It does not estimate a general memory improvement: both arms use the same product binary, and only the registered claim-rollout stage differs.

## Frozen artifacts

- [Preregistration](preregistration.md)
- [`plan.json`](plan.json)
- [`tasks.json`](tasks.json)
- [`independent-probes.json`](independent-probes.json)
- [`impact-rationale.json`](impact-rationale.json)
- [`probe-preflight/preflight-summary.json`](probe-preflight/accepted/preflight-summary.json)
- [`freeze-manifest.sha256`](freeze-manifest.sha256)
- [`preflight-failures/registration-v1.json`](preflight-failures/registration-v1.json) records a stale task-prompt fingerprint rejected before any Codex task was sent. Registration v2 fixes the fingerprint and changes no task, treatment, budget, or analysis.
- [`execution-failures/run-01-sandbox.json`](execution-failures/run-01-sandbox.json) records an invalid sandboxed attempt: the CLI could not initialize its local app server, all receipts have zero turns, and no task reached the model. It is excluded from the pilot outcomes.

The deterministic preflight accepted eight conflict families and four ordinary controls. For conflict probes, `shadow` scored 0/8 and `evidence` 8/8. Both configurations scored 4/4 on ordinary source-coverage controls. All accepted scores were regenerated with the offline evaluator before any remote task run.

## Result

Both valid captures completed all 12 paired trials per run. Offline paired-comparison replay and independent-probe rescoring succeeded. The product binary was identical in both arms (`3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`); only the claim-rollout stage differed.

| Outcome | Run 02 | Run 03 |
|---|---:|---:|
| Conflict task success, `shadow` | 0/8 | 0/8 |
| Conflict task success, `evidence` | 2/8 | 3/8 |
| Ordinary task success, `shadow` | 0/4 | 1/4 |
| Ordinary task success, `evidence` | 0/4 | 0/4 |
| Independent conflict probes, `shadow` | 0/8 | 0/8 |
| Independent conflict probes, `evidence` | 8/8 | 8/8 |
| Independent ordinary probes, both arms | 4/4 | 4/4 |
| Hard violations / budget overruns | 0 / 0 | 0 / 0 |

The agent task requires the exact safe conflict answer, correct clarification state, a supporting-source citation, and completion within one memory read. The independent probe confirms that the returned source itself exposes the typed conflict annotation. These measures disagree in scope: the memory treatment is consistently visible to the independent probe, while agent task success remains low.

The preregistered ordinary-control gate required `evidence` to score at least 3/4 in each capture and no lower than `shadow`. It scored 0/4 in both captures, so the gate failed. No confirmatory sample was run, no family was reused, and no general agent-usability or memory-benefit claim follows. The observed conflict-task difference is exploratory: +2/8 in run 02 and +3/8 in run 03. The three conflict families that differed in at least one retest are not three independent new confirmations; the independent denominator remains eight.

The [paired run artifacts](runs/) preserve the receipts, independent probes, and comparisons. Offline replay summaries: [run 02](runs/run-02/diagnostic-comparison.json) and [run 03](runs/run-03/diagnostic-comparison.json). The invalid sandbox startup and stale v1 registration remain separate preflight failures and are excluded from outcomes.

Power planning for the conflict primary uses three discordant families out of eight, the frozen 95% one-sided Clopper-Pearson upper bound (`0.7107591834981906`), and the exact McNemar rule. At the frozen +0.25 alternative, +0.10 observed-effect floor, one-sided alpha 0.025, and 90% power, the minimum is 123 independent families (`0.900812327940497` power); 122 is below 90%, and the power at the cap of 200 is `0.9877975121444901`. Rust unit tests pin these values. The machine-readable [power report](runs/power-analysis.json) has SHA-256 `799a1fa91b4c2b91e51b8414e8e5a567e8e1762fecaafaf60e02cb81d2399711`; a fresh CLI replay produced identical bytes. This is planning for the conflict primary only; it cannot override the failed ordinary-control gate or authorize a confirmatory run. The provider-independent evaluator CLI command is documented in [PAIRED_AGENT_EVAL.md](../../../docs/evals/PAIRED_AGENT_EVAL.md).

## Boundaries

- All remote task inputs are invented synthetic data; no user or project memory is sent.
- Repeated captures assess repeatability but do not increase the count of independent families.
- Ordinary cases are finite regression controls, not a population sample or powered non-inferiority study.
- A same-binary paired comparison estimates only the registered configuration-stage effect for this narrow task/model setup; it is not evidence of broad product improvement.
- Confirmation, if justified by the pilot, requires a disjoint preregistered cohort and exact power analysis under the frozen cap and margins.
