# Agent-managed fact-evidence feasibility v6

Registered before the first v6 Codex invocation on 2026-11-23. This new fixed cohort uses eight synthetic correction families disjoint from v1-v5. The exact task manifest, protocol assets, product binaries, and identities are frozen in freeze-manifest.sha256.

## Question

Does the bounded extract.include_fact_evidence=true response let an agent verify an extracted correction without a separate episode read, while preserving exact invalidation, fresh-session recall, independent-probe correctness, and per-phase budgets? Candidate evidence is all-or-none under a 4096-byte serialized evidence budget. The incumbent exposes the compact extract response and must verify by reading the returned episode.

This is a repeated fixed-cohort feasibility comparison for Codex CLI 0.171.0 and the registered model alias. The two captures reuse the same eight families, so they are repeatability checks, not sixteen independent observations, a powered population estimate, or proof of a broad memory-quality effect. The treatment changes only the extract response capability. Stored-memory and retrieval quality are measured as regression controls; this response-only treatment cannot itself improve persisted facts or retrieval metrics.

## Frozen execution

- Two complete paired invocations (run-01, run-02), each with fresh stores and all eight task families in both arms.
- V6 family names, facts, tasks, and oracles are disjoint from all prior maintenance cohorts. The hidden oracle is withheld from the agent.
- The incumbent and candidate binaries are pinned by SHA-256. The product configuration, claim-rollout stage, prompts, tool profile, simulator, and Codex identity are held fixed; only the candidate binary exposes the bounded optional evidence response.
- Each formation phase must perform one exact inline extract, verify the returned episode and correction evidence before writing, then perform one exact invalidation of only the superseded fact. If any prerequisite fails, it must not invalidate.
- Each trial then runs fresh-session recall and an evaluator-owned independent probe against the same trial store.
- No shell, web, app, plugin, or permission-request tools. Use the existing local Codex login through --codex-home; do not copy or store credentials. The Codex child remains read-only; only its scoped memory tools are enabled.
- Every failed or incomplete invocation is retained. No retries are silently substituted and no task replacement or prompt tuning is permitted after registration.

## Feasibility criteria

A feasibility pass requires both complete paired invocations; 8/8 candidate formation, recall, and independent-probe outcomes correct in each capture; exactly one successful extract and one successful exact invalidation per candidate family; zero unsupported/prohibited source use, hard violations, or phase budget overruns; evidence requested and received before invalidation in every candidate case; evidence response bytes at or below 4096; and fewer candidate formation tool calls with no recall or independent-probe regression. Deterministic response-contract tests, explicit-false and omitted-parameter compatibility, oversized all-or-none omission, and the current-master deterministic memory-quality gate must remain green.

Passing this gate supports only feasibility on these eight synthetic families. It does not show a persistent-memory quality gain, establish statistical non-inferiority, or by itself authorize a global default change. The report must distinguish usability/resource measurements from memory-quality metrics. A failure is a no-go for the frozen workflow; later candidates require a new preregistered cohort.
