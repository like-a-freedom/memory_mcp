# Qwen speed source-authority qualification v1

**Plan ID:** `source-authority-local-qwen-speed-2026-11-23-v1-qualification`  
**Stage:** development qualification screen, not powered confirmation  
**Date frozen:** 2026-11-23 UTC

## Question and limits

Can this exact Qwen3.8 speed-optimized local configuration complete the registered source-authority workflow well enough to justify designing a separately powered paired study? Both arms use the same current memory binary. This screen does not estimate a memory treatment effect, establish non-inferiority, or prove broad agent utility. A failed screen is a no-go for this exact model, prompt, sampler, and budget configuration. Do not tune or replay these exposed families.

## Frozen inputs and model

- Four fresh synthetic families: two literal-source conversational controls and two candidate operator-import tasks.
- Model alias: `memory-local-eval-qwen-speed`; LM Studio-reported model path `qwen3.8-27b-mtplx-optimized-speed`; local context length 16,384, parallelism 1.
- Sampling configuration is byte-pinned in `sampling-config.json`: temperature 1.0, top-p 0.95, top-k 20, min-p 0, presence penalty 0, repeat penalty 1.
- Endpoint is numeric loopback `http://127.0.0.1:1234/v1`; no bearer token is used. The optional request parameters and response-format schema are absent.
- Product and evaluator executables are pinned in `plan.json`. Model weights and LM Studio server build cannot be hashed here; `provider-inventory.json` records this limitation and the within-invocation alias identity protocol.
- The local compatibility smoke passed once using a synthetic prompt and in-memory fake MCP server. It is transport evidence only and is excluded from all quality metrics.

## Decision gates

Advance only if the registered capture and offline comparison are valid and all of these hold:

1. Task-oracle success is at least 3/4 in each arm.
2. Independent fixed-query probes pass 4/4 in each arm.
3. Candidate source-file tasks pass at least 1/2.
4. Both operator imports and extraction steps succeed (2/2).
5. Hard violations and budget-exceeded trials are both zero.

Otherwise stop this exact configuration. Any powered follow-up needs a separate preregistration, fresh families, a justified sample-size calculation, and a predeclared practically meaningful effect. No failure may be attributed to memory from this same-binary workflow screen alone.
