# C1 agent-managed correction: setup failure

This is an archived setup attempt, not a valid agent-workflow result. All
eight task receipts are `model_failure` with zero turns, tool calls and tokens;
process elapsed time was below 0.5 seconds. The runner did not capture a model
turn, so none of these receipts can be scored as a memory-agent outcome. The
comparator is retained only to reproduce the zero-success setup report.

A separate synthetic Codex CLI preflight succeeded after running the authorized
local process with access to Codex's normal local state. The later valid C1
screen uses four new synthetic families under
[`c1-agent-managed-correction-codex-direct-mlx-2026-11-25`](../c1-agent-managed-correction-codex-direct-mlx-2026-11-25/README.md).
This cohort was not retried with the same families, so it cannot be mistaken
for the valid C1 capture or increase its independent-family count.

To reproduce the offline report without Codex:

```sh
target/debug/memory-eval compare-agent-managed \
  --plan "$PWD/evals/paired/c1-agent-managed-correction-codex-2026-11-25/plan.json" \
  --incumbent "$PWD/evals/paired/c1-agent-managed-correction-codex-2026-11-25/capture-01/incumbent.json" \
  --candidate "$PWD/evals/paired/c1-agent-managed-correction-codex-2026-11-25/capture-01/candidate.json" \
  --incumbent-stages "$PWD/evals/paired/c1-agent-managed-correction-codex-2026-11-25/capture-01/agent-managed-incumbent.json" \
  --candidate-stages "$PWD/evals/paired/c1-agent-managed-correction-codex-2026-11-25/capture-01/agent-managed-candidate.json" \
  --output /tmp/c1-agent-managed-correction-setup-failure.json
```
