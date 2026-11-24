# Agent-managed fact-evidence feasibility v5

Registered before the first v5 Codex invocation on 2026-11-23. The task manifest contains eight fixed synthetic correction families that do not reuse v1-v4 family names or facts. The manifest, protocol, prompt, schema migration and product identities are frozen in `freeze-manifest.sha256`.

## Question

Does a bounded `extract.include_fact_evidence=true` response let an agent safely verify the newly extracted fact without an extra `assemble_context` read, while preserving exact invalidation, subsequent recall correctness, independent-probe correctness and the existing per-phase limits? The candidate response cap is 4096 serialized evidence bytes, all-or-none; the incumbent exposes only compact extraction results.

This is an exploratory, fixed-cohort feasibility comparison for Codex CLI 0.171.0 and the registered model alias. It is not a powered population estimate, does not establish a general memory-quality improvement, and cannot be used to claim that evidence changes stored facts or retrieval. The deterministic memory-quality gate is separate and remains a required regression check.

## Frozen execution

- Two complete paired invocations (`run-01` and `run-02`), each with fresh stores and all eight task families in both arms. The hidden oracle is not given to the agent.
- The baseline binary is pinned by SHA-256 to the old compact-response build. The candidate binary is pinned to the current capped evidence build. Only the extract contract differs materially; both use the same feature profile, prompt, tool allowlist, simulator, and Codex identity.
- Each formation phase must perform one exact inline `extract`, verify the returned episode and evidence before writing, then perform one exact invalidation of only the superseded fact. If any prerequisite fails, it must not invalidate.
- Each trial then runs a fresh-session recall plus the fixed evaluator-owned independent probe against the same trial store.
- No shell, web, app, plugin, memory, or permission-request tools. The Codex child remains read-only; direct host invocation is explicitly authorized. Local Codex login is selected via `--codex-home`; no API key is copied or stored.
- Task, context, tool-call, turn, output-token, wall-time, and tool timeout budgets are inherited from the frozen v4 protocol and bound in `plan.json`. Every failure and incomplete repeat counts; no retries are silently substituted.

## Feasibility criteria

A candidate feasibility pass requires both paired invocations to finish all cases; every candidate maintenance and recall answer and independent probe must be correct; each candidate must make exactly one successful extract and one successful invalidation per family; there must be zero prohibited/unsupported source use, hard violations, budget overruns, or omitted evidence for these short facts; evidence response bytes must stay at or below 4096; and candidate tool-call count must be lower without a recall/probe regression. Deterministic response-contract and compatibility tests and the current-master memory-quality gate must also pass before considering any default change.

A pass supports only this bounded workflow and cohort. A default change, if justified after review, must retain explicit `include_fact_evidence=false` compatibility and must be separately tested with the field omitted, including the 4096-byte cap and oversized all-or-none behavior. No tuning or replacement of failed v5 tasks is permitted.
