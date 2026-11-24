# Qwen v3 paired pilot — repeat 2

**Capture:** 2026-11-23, local `memory-local-eval-qwen38` through the provider-neutral Chat Completions adapter. The frozen plan, task manifest, model alias, prompt, policies, product binary, evaluator and budgets were unchanged from run-01. No Codex API key or bearer token was supplied.

**Artifacts:** 16/16 preregistered families in each arm; 32 receipts total. The run completed from `2026-11-23T10:21:54.096813Z` through `2026-11-23T11:08:37.645819Z` (46m 43.549s). The frozen plan SHA-256 is `af0d4f6d7de9061fe1ec9bcef5c065b686a1dbd3cd7fc5349400bef31cf66368`; the paired manifest SHA-256 is `a6f5c19561ea781918bc01ba431bb659abf783333394c3739fd7779981bcf25c`.

## Results

| Measure | Incumbent | Candidate | Interpretation |
|---|---:|---:|---|
| Task-oracle success | 0/16 | 0/16 | No agent completed a task to the frozen oracle in either arm. |
| Direct-memory success | 11/16 | 13/16 | Descriptive difference of +2 families; not a product treatment comparison. |
| Independent fixed-query probe | 11/16 | 13/16 | Repeats the workflow-specific storage outcome, not the agent's answer quality. |
| Hard violations | 0 | 0 | No prohibited evidence or other registered hard violation. |
| Task receipt status | 4 completed, 12 model failures | 7 completed, 9 model failures | Candidate had no task-budget exceedance in this repeat. |
| Formation status | 6 completed, 10 model failures | 9 completed, 6 model failures, 1 budget exceeded | Sidecar formation metric; not the task receipt status. |

The deterministic paired comparison estimates a +0.125 direct-memory difference with a 95% family-cluster interval of [-0.1875, 0.4375]. Task success is 0 in both arms. Independent probes pair as 8 families where both succeed, 3 incumbent-only, 5 candidate-only and 0 neither. `compare-agent-managed` used 10,000 SplitMix64 bootstrap draws over 16 independent synthetic families; `summarize-pilot` reports the two-repeat family means and test-retest variance.

The average agent-trial wall time was 42.2 seconds per receipt in run-02. In the separate LFM pilot it was 0.64 seconds per receipt, about 66 times faster on these captures. This is not a controlled speed comparison: the LFM pilot used four different families, its eight receipts all failed with normalized `internal` model errors, and the Qwen pilot used 16 families with a different distribution of turns and tools. Both pilots had zero task-oracle passes. The LFM speed is useful for quick adapter/tool-call smoke, but did not produce a usable workflow evaluation. The content-free trace stores only the normalized `internal` class; it intentionally omits raw model output, so the current artifact cannot attribute those failures to a particular response or tool-call shape.

The same frozen cohort was repeated once in run-01. Both repeats returned the same arm-level task and direct-memory counts and the same independent-probe pairing. Run-01 had 4/16 incumbent and 6/16 candidate completed task receipts, with 2 candidate budget exceedances; run-02 had 4/16 and 7/16 completed, with no budget exceedance. There were zero hard violations in both. These repeats are nested within the same 16 synthetic families and are not 32 independent examples.

Both arms used the exact same product binary, configuration and tool schema. Their difference is the source-preparation workflow, so the +2 probe result cannot be attributed to a memory implementation change. The task oracle passed 0/16 in both arms in both repeats, preventing a claim of successful source-authority workflow usability. This pilot is a feasibility NO-GO for the tested model-workflow combination and does not estimate memory benefit, general usability or powered agent benefit. Do not tune or confirm on these exposed families.

## Reproduction

The raw receipts, sidecar stages, paired comparison and deterministic two-repeat summary are retained beside this report. Recreate the summary from the frozen artifacts with:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- summarize-pilot \
  --plan evals/paired/source-authority-local-chat-completions-v3/plan.json \
  --first-run-directory evals/paired/source-authority-local-chat-completions-v3/runs/run-01 \
  --second-run-directory evals/paired/source-authority-local-chat-completions-v3/runs/run-02 \
  --output /tmp/source-authority-v3-repeatability-summary.json
```

This offline scoring command does not call a model provider. The repeated agent captures themselves are model-backed observations and are not deterministic merely because their scoring is reproducible.
