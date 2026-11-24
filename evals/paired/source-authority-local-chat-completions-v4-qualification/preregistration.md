# Qwen source-authority qualification v4

**Registered:** 2026-11-23, before capture.  
**Purpose:** one short go/no-go screen for whether the current local Qwen model can complete the source-authority workflow after the evaluator gained granular, content-free final-answer failure classes. This does not test a memory implementation effect.

## Frozen design

- Four new synthetic history families, disjoint from the earlier v1–v3, LFM and maintenance cohorts: two conversation controls and two literal-file-import workflows.
- One trial per family in each arm; the incumbent and candidate use the same `memory_mcp` binary, product configuration, public MCP schemas, model alias, prompts, tools and budgets. The only registered difference is the setup workflow: model-managed `ingest` + `extract` versus operator import followed by read-only recall.
- Model alias: `memory-local-eval-qwen38`, reported by LM Studio as `lmstudio-community/Qwen3.8-27B-MLX-4bit`, 4-bit, 16,384-token runtime context. We do not have a model-weight hash or LM Studio server build identity; this screen is not a pinned-model confirmation.
- Endpoint: `http://127.0.0.1:1234/v1`; the runner restricts the endpoint to numeric loopback. No Codex runner, bearer token, API key, remote endpoint, external prompt service or network fetch is used.
- Task, answer, tool and wall-time budgets are copied unchanged from v3. Retries are disabled. Every model response is scored by the frozen task oracle; the independent probe uses a separate read-only `assemble_context` session.
- Final-answer failure labels are content-free. The capture must not persist raw provider response text.

## Screening decision fixed before capture

Proceed only to a separately preregistered, broader workflow pilot if all capture/artifact checks are valid, there are zero hard violations, independent probes pass 4/4 in each arm, task-oracle success is at least 3/4 in each arm, and candidate file-import task success is at least 1/2. Otherwise stop model-backed source-authority evaluation for this model/workflow configuration. This small screen cannot qualify a powered comparison, establish non-inferiority or acceptance, or support an agent-benefit claim.

Latency is reported only as within-capture diagnostics. It is not compared causally with prior LFM or Qwen runs, whose family sets, receipts and evaluator versions differ. Exposed v4 families must never be reused for confirmation.

## Interpretation limits

A pass only shows that this current Qwen configuration is worth further pilot work. Both product arms are identical, so any task/probe difference is workflow feasibility, not memory-quality treatment effect. A fail is a no-go for this model/configuration and does not show a defect in Memory MCP. The missing immutable model-weight and server-build identities preclude a confirmatory claim even if the screen passes.
