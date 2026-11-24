# Preregistration: local source-authority workflow pilot v3

**Plan ID:** `source-authority-local-chat-completions-2026-11-23-v3`  
**Stage:** development pilot  
**Independent families:** 16  
**Captures:** two repeats on the same frozen families  
**Purpose:** validate the corrected provider-neutral paired runner and measure local workflow feasibility only

## Question and estimand boundary

On one pinned product binary, compare (A) an agent that reads one synthetic relative-path source through public `ingest` and `extract` tools with (B) an operator import and extraction followed by a fresh read-only agent recall session. Eight ordinary conversation controls exercise the same product in both arms. An independent public-MCP probe verifies stored state after each workflow. Both arms use the identical memory product binary; this is not a memory-treatment comparison and cannot establish a causal product effect or broad agent benefit.

## Frozen cohort and procedure

The task manifest contains eight conversation-control families and eight source-file-import families. All names, facts, answers, and file contents are synthetic and disjoint from v1 and v2 source-authority families. Each arm/family uses a fresh isolated store. Arm order is counterbalanced and fixed in `plan.json`. The runner gets one attempt per arm/family, zero retries, and both captures must be retained separately. No task, prompt, runner, evaluator, product, or budget changes are allowed after first model exposure.

The incumbent resolves a synthetic relative path under the evaluator-owned source root. The candidate imports the identical one-file content through the confined operator import command before starting a read-only agent. Shell access is disabled in both formation and recall. Agent-interaction outcomes and independent fixed-query probe outcomes are reported separately. A failed or missing receipt never passes.

## Provider and privacy boundary

The adapter uses the provider-neutral OpenAI-compatible Chat Completions contract over numeric loopback at `http://127.0.0.1:1234/v1`. LM Studio is this laptop's optional local provider, not an eval-harness dependency. Its current developer docs also describe a native `/api/v1/*` REST API; this runner uses `/v1/chat/completions`, not the native REST API. The deterministic contract tests use a fake in-process loopback server and must remain runnable on Windows, Linux, and macOS without LM Studio, a model, credentials, or Codex.

At registration, `lms ps --json` reported a downloaded local Qwen3.8 27B MLX 4-bit model under alias `memory-local-eval-qwen38`, runtime context 16,384 tokens; `lms server status` and `/v1/models` confirmed the local server and alias. The runner caps input at 15,360 tokens, output at 1,024, tool calls and turns at eight each, and phase wall time at 120 seconds. The alias, CLI identity, endpoint configuration, runner/evaluator, prompts/policies, product, task manifest, and budgets are fingerprinted. Model weight and server-build hashes are unavailable; model identity between separate captures is not independently proven. No model request is sent before the preregistration freeze.

Adapter loopback restrictions do not prove that an arbitrary local provider cannot forward traffic. This pilot sends only synthetic task text. LM Studio's offline guide and privacy policy are provider statements, not an OS-level packet audit; cloud and LM Link modes are outside this registration. No API key or bearer token is required.

## Registered outcomes and interpretation

Keep every registered family in the denominator. Report per arm and capture: completed phases, model failures, budget exceedances, task-oracle success, agent-observed source coverage, independent probe success, hard violations, tool/turn/token counts, elapsed time, and operator setup cost. Summarize repeats by family; do not count 32 repeated observations as 32 independent families. There are no promotion criteria and no claim of powered inference. If the corrected runner accepts the captures, that establishes artifact compatibility and gives descriptive feasibility evidence for this one provider/model/workflow only. It does not establish population usability, memory-quality improvement, causal memory benefit, non-inferiority, or generalization to other providers.
