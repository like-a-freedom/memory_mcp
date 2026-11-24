# Ornith 1.5 35B-A3B source-authority qualification screen

**Registered:** 2026-11-23, after the standalone tool-call smoke and before this paired capture.  
**Purpose:** test whether this already-downloaded local model can complete the source-authority workflow under the existing provider-neutral adapter. This does not compare memory implementations or establish agent benefit.

## Frozen design

- Four new synthetic history families, disjoint from all prior Qwen, LFM, maintenance, and source-authority cohorts: two conversation controls and two literal-file-import workflows.
- One trial per family in each arm. Both arms use the same `memory_mcp` binary, product configuration, public MCP schemas, model alias, prompts, tools, adapter parameters, and budgets. The sole registered difference is model-managed `ingest` + `extract` versus operator import followed by read-only recall.
- Model alias: `memory-local-eval-ornith`, loaded from the already-downloaded `ornith-ai/Ornith-1.5-35B-A3B-MLX-4bit`, reported as 35B total / approximately 3B active per token, 4-bit, 16,384-token context. The model-weight hash and LM Studio server build are unavailable; this is an exploratory screen, not a pinned model confirmation.
- Endpoint is numeric loopback `http://127.0.0.1:1234/v1`; no Codex process, bearer token, API key, external model endpoint, network fetch, or raw response storage. Tasks contain only invented facts.
- The existing adapter uses temperature 0 and leaves `top_p` unset; this is frozen by the runner binary. The model card recommends temperature 0.6 / `top_p` 0.95 for general tasks. The adapter-contract smoke passed at its existing settings; no tuning is allowed within this cohort. Any future sampling change requires a new registered plan and a fresh cohort.
- Retries are disabled. Task, answer, tool and wall-time budgets are copied unchanged from Qwen v4. All receipts and independent fixed-query probes are scored offline by the pinned deterministic evaluator.

## Screening decision fixed before capture

Proceed only to a separately preregistered, broader workflow pilot if capture/artifact checks are valid, hard violations are zero, independent probes pass 4/4 in each arm, task-oracle success is at least 3/4 in each arm, and candidate file-import task success is at least 1/2. Otherwise stop this model/workflow configuration. This screen cannot establish non-inferiority, powered confirmation, memory benefit, or agent utility.

Latency is descriptive within this capture only and cannot be compared causally with previous models or cohorts. Exposed families must not be reused for confirmation.
