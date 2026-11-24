# Qwen3.8 Splash JSON Schema qualification v1

**Plan ID:** `source-authority-local-qwen-splash-json-schema-2026-11-23-v1`  
**Status:** frozen before model load and capture on 2026-11-23.

## Question and scope

Can the registered Qwen3.8 Splash model and the provider-neutral Chat
Completions adapter complete source-authority tasks when the same strict JSON
Schema response format is sent on tool-call and final-answer turns? This screen
tests the agent workflow and structured-output/tool-call compatibility. Both
arms use the same memory product binary, so this cannot estimate a memory
treatment effect or general agent benefit.

LM Studio's current docs describe JSON Schema output and tool calls separately.
The combination is therefore tested empirically. The selected model is the
locally installed 4-bit `incoai/Qwen3.8-27B-Splash`; its model card describes
OpenAI Chat Completions with tool calls and JSON Schema, but does not establish
success on this benchmark. The exact model and server build hashes are not
available from the local inventory.

## Frozen design

- Four new synthetic families, disjoint from all prior Codex, LFM, Qwen, Flint,
  maintenance and source-authority cohorts.
- Two conversational formation tasks and two source-file tasks. Incumbent
  receives exact factual text through supported literal `ingest`; candidate
  receives the same bytes through registered operator file import. Candidate
  recall is read-only after import.
- Same current memory product binary, MCP schema, model alias, endpoint,
  sampler, prompt, budgets and response format in both arms.
- Task success floor: at least 3/4 in each arm; independent probes: 4/4 in each
  arm; candidate source-file task success: at least 1/2; operator imports and
  extractions: 2/2; hard violations and budget exceedances: zero.
- No retries, prompt edits, tuning, or reuse of these exposed tasks after
  capture. A failed screen is a configuration-specific NO-GO.

## Capture

Verify `freeze-manifest.sha256` from the repository root before loading the
model. Load the registered alias and execute the frozen command in `README.md`
once. Use no bearer/API token and do not overwrite `runs/run-01`. Raw model
messages remain transient; only the existing content-free receipts are stored.

This is a small feasibility/qualification screen, not a powered experiment. A
pass would only permit a separately preregistered pilot. It would not close the
powered memory-agent benefit requirement.
