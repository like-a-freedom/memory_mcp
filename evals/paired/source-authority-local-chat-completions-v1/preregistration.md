# Local source-authority workflow pilot v1

Status: frozen at 2026-11-23T07:44:50Z after correcting absolute-path instructions; registration inputs are bound by `freeze-manifest.sha256`. Two CLI attempts were rejected during preflight because product and output paths were relative; no trial or model call has begun.

## Question

On the same pinned current product binary, compare (A) the incumbent agent workflow, which reads one synthetic relative-path source through `ingest` and `extract`, with (B) operator file import and extraction before a read-only agent session. Eight ordinary conversation controls use the same product in both arms. This isolates the workflow path and its measured interaction/setup cost; it does not compare memory implementations.

## Cohort and execution

- Sixteen new synthetic families: eight source-file tasks and eight conversation controls. Names, facts, questions, and source values are disjoint from the frozen Codex v1/v2 source-authority cohort.
- One case per family, counterbalanced incumbent-first order, fresh store per arm/case, and a fixed independent public-MCP probe after the agent phases.
- Run the frozen manifest twice in separate output directories. Repeats are test-retest observations on the same sixteen families, not 32 independent samples. Do not tune prompts or task content between runs.
- Both arms use product binary SHA-256 `0c465a019c824602e1adc02c37b97f93ff3a6a5b98226d3007b7c3d93c990f25`, feature `fs-watch`, claim stage `evidence`, embedded SurrealDB, regex extraction, and embeddings disabled. The exact plan also pins the tool schema, product configuration and evaluator executable.
- The runner is `openai-compatible-chat-completions`, model alias `memory-local-eval-qwen38`, local endpoint `http://127.0.0.1:1234/v1`, and LM Studio CLI commit `dcefd5a`. The plan pins the SHA-256 of the normalized effective endpoint URL under `chat_completions_endpoint_config`; the runner rejects a missing or mismatched fingerprint before a trial. Use the same endpoint/model for both arms. Only the alias is available as model identity; weight and server-build hashes are unavailable, so cross-invocation model identity is not established.
- Input, tool, turn, output and wall-time limits are frozen in `plan.json`; retries are zero. LM Studio reports a 16,384-token model context, so the input-token cap is 15,360 with up to 1,024 output tokens. All source text and facts are generated synthetic fixtures. No project or personal memory is included.
- The model endpoint is numeric loopback, and the adapter disables proxy and redirect handling. This cannot prove whether a local provider process forwards requests elsewhere. This pilot contains synthetic-only data regardless.
- LM Studio is the selected provider for this laptop's exploratory capture, not a harness dependency. Required provider-neutral tests use a fake HTTP endpoint and a fake MCP session; contributors can run them with Cargo on Windows, Linux, or macOS without a model, provider CLI, or API key. Only macOS execution is verified in this registration.

## Outcomes and interpretation

Record task-oracle success, direct-memory source coverage, the independent probe, unsupported/prohibited evidence, hard violations, formation/recall turns, MCP calls, input/output tokens, wall time, and operator setup actions and duration separately. Preserve failed, skipped, invalid and budget-exceeded trials in the denominator. Replay deterministic summaries from the frozen receipts.

This is a runner/workflow feasibility and descriptive-cost pilot only. It has no promotion criteria or power analysis. It cannot establish memory-quality improvement, agent benefit, workflow non-inferiority, causal product effect, or population reliability. The repeated invocations do not increase the independent-family count. A valid local run does not close the separate powered agent-benefit requirement.
