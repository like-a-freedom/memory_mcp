# Qwen source-authority qualification v4 — run 02

**Result: NO-GO for further workflow evaluation with this model/prompt/budget.** The preregistered short-screen criteria failed; no acceptance, memory-benefit or agent-usability claim follows.

## Capture integrity

- The run completed from `2026-11-23T13:24:51.369527Z` to `2026-11-23T13:36:24.135490Z` (11m 32.766s).
- All four registered families have one receipt in each arm. Both arms bind the same `memory_mcp` binary SHA-256 `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`, same config and public tool schema, and the same local Qwen alias.
- The `compare-agent-managed` offline scorer accepted the artifact identities and produced [agent-managed-comparison.json](agent-managed-comparison.json). It reports zero hard violations and one candidate budget-exceeded task.
- No raw model response text is retained. Receipts contain content-free failure classes, token/timing counts, source IDs and payload hashes.
- `run-01` remains an empty output directory for the recorded pre-model setup failure: the first command used a relative output path, rejected before model process creation. The valid capture is `run-02`.

## Frozen screen outcomes

| Measure | Incumbent | Candidate | Gate / interpretation |
|---|---:|---:|---|
| Task-oracle success | 0/4 | 0/4 | Fails the preregistered minimum 3/4 in each arm |
| Agent-observed direct-memory success | 3/4 | 3/4 | No paired difference; not the independent probe |
| Independent fixed-query probe | 3/4 | 3/4 | Fails the required 4/4 per arm |
| Candidate file-import probes | — | 2/2 | Import/extraction and source coverage succeeded; this does not rescue failed agent tasks |
| Hard violations | 0 | 0 | Safety check only |

Independent probe pairs were both-success 2, incumbent-only 1, candidate-only 1, neither 0. The family-weighted direct-memory paired estimate is 0 with a descriptive 95% cluster interval of `[-0.75, 0.75]` across only four families. Task success was zero in both arms. This interval is too wide to support a comparative conclusion.

Task receipt statuses were incumbent: 1 completed, 3 model failures; candidate: 2 completed, 1 model failure and 1 budget exceeded. The new granular diagnostics found incumbent model failures `final_answer_not_json` (2) and `invalid_tool_call` (1); candidate had `final_answer_not_json` (1). Completed receipts still failed the task oracle, so valid response shape did not guarantee a correct answer. Formation sidecars were incumbent: 2 completed and 2 model failures; candidate: 1 completed, 2 model failures and 1 budget exceeded.

The candidate operator import and extraction succeeded for both registered file fixtures, and both source-file independent probes passed. That is evidence the deterministic import/setup path can create retrievable facts. The model still failed the final task oracle on both source-file tasks. This separates product/import correctness from agent workflow reliability.

## Decision and limits

All registered stop conditions were met: task-oracle success was below 3/4 in both arms and independent probes were below 4/4. Do not run a larger Qwen workflow cohort under this configuration, and never reuse these four exposed families as confirmation. The task evidence points to model/tool-workflow failures (malformed final answer, malformed tool call, wrong completed answer and a budget overrun), not a newly demonstrated memory defect.

The same product binary in both arms means this screen cannot estimate a memory implementation effect. Four synthetic families, one capture, a mutable LM Studio alias, unavailable model-weight hash and unreported server build also make this unsuitable for population inference, model qualification beyond this screen, non-inferiority or a powered agent-benefit claim. The 11m 33s capture duration is specific to this run; comparisons with the prior LFM pilot are not controlled speed benchmarks.

## Reproduction and artifact integrity

The frozen registration is verified with `shasum -a 256 -c evals/paired/source-authority-local-chat-completions-v4-qualification/freeze-manifest.sha256`. The run artifacts are covered by [SHA256SUMS](SHA256SUMS), which validates from the repository root. Recreate only the offline summary by rerunning `compare-agent-managed` with this directory's plan and four JSON artifacts; do not call the model again or overwrite this run.
