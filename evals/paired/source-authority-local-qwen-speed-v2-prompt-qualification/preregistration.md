# Qwen speed source-authority qualification v2 preregistration

**Plan ID:** `source-authority-local-qwen-speed-2026-11-24-v2-prompt-qualification`  
**Stage:** development qualification screen, not powered confirmation  
**Frozen:** 2026-11-23 21:55:59 UTC

## Question and limits

Does the frozen v2 concise-answer prompt let this exact Qwen3.8 local configuration complete the registered source-authority workflow well enough to justify designing a separately powered paired study? V2 explicitly asks for a direct value with no preamble or explanation because the deterministic task oracle requires an exact normalized answer. No scoring threshold changes. The four task families are new and disjoint from prior exposed cohorts.

Both arms use the same current memory binary and identical product configuration. This is a workflow/configuration screen only: it does not estimate a memory treatment effect, establish non-inferiority, or prove general agent utility. Failure is a no-go for the exact tested model, prompt, sampler, and budgets. Do not tune or replay these families.

## Frozen inputs and model

- Four synthetic families: two literal-source conversational controls and two candidate operator-import tasks.
- Model alias: `memory-local-eval-qwen-speed-v2`; LM Studio reports model path `Youssofal/Qwen3.8-27B-MTPLX-Optimized-Speed`, family `qwen3_5`, 27B, 4-bit, 21,313,944,611 bytes, context 16,384, parallelism 1.
- Sampling: temperature 1.0, top-p 0.95, top-k 20, min-p 0, presence penalty 0, repeat penalty 1; pinned in `sampling-config.json`.
- Numeric-loopback endpoint: `http://127.0.0.1:1234/v1`; no bearer token, provider request parameters, or response-format override.
- Product and evaluator executables, prompt fingerprints, corpus, oracle, protocol, and budgets are pinned in `plan.json` / `fingerprints.json`. Model weights and server build cannot be byte-pinned; their absence is explicit in `provider-inventory.json`.
- No model output text is included in result artifacts; only the evaluator's content-free receipt diagnostics are retained.

## Frozen decision gates

Advance to a separately designed and powered study only if the capture and offline comparison are valid and every gate passes:

1. Task-oracle success is at least 3/4 in each arm.
2. Independent fixed-query probes pass 4/4 in each arm.
3. Candidate source-file tasks pass at least 1/2.
4. Both candidate operator imports and extraction steps succeed (2/2).
5. Hard violations and budget-exceeded trials are both zero.

Otherwise stop this exact configuration. A powered follow-up requires a separate preregistration, fresh families, a justified sample-size calculation, and a predeclared practically meaningful effect. Same-binary arms do not support memory-effect attribution under any outcome.
