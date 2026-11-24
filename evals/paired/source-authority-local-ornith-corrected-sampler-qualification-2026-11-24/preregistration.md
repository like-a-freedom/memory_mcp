# Ornith corrected-sampler source-authority qualification

**Plan ID:** `source-authority-local-ornith-35b-a3b-2026-11-24-corrected-sampler-qualification`  
**Stage:** one-capture development qualification screen, not powered confirmation  
**Frozen before model-task capture:** 2026-11-23 22:39 UTC

## Question and limits

Does the exact local Ornith 1.5 35B A3B configuration pass the frozen source-authority agent workflow screen when sampled with the model card's recommended general-task parameters and the current literal-source/v2 concise-answer task protocol? The prior Ornith screen used temperature 0 and no explicit `top_p`, contrary to its model card's recommended `0.6 / 0.95 / 20`. This registration changes only that model-generation configuration and uses four fresh synthetic families. The v2 recall instruction and exact-answer oracle remain unchanged.

Both arms use the same current memory binary. Incumbent agents receive source facts as literal text and use `ingest`/`extract`; candidate files are imported by the evaluator before a read-only recall session. The screen measures this workflow/configuration only. It is not a memory treatment effect, non-inferiority study, or general agent-utility claim. No model output text will be stored.

## Frozen configuration

- Local alias: `memory-local-eval-ornith-v2`, mapped to the installed `ornith-1.5-35b-a3b-mlx` model. Weight digest and LM Studio server build identity are unavailable; the in-invocation alias and CLI identity are recorded.
- Endpoint: numeric loopback `http://127.0.0.1:1234/v1`; no token or external endpoint.
- Sampler: `temperature=0.6`, `top_p=0.95`, `top_k=20`; these values match the official model card's general-task recommendation. Other sampling controls stay at the pinned runner defaults.
- Preflight: content-free local two-turn tool round-trip passed with the exact sampler; see [`smoke-receipt.json`](smoke-receipt.json). It is a transport/parser smoke only.
- Four fresh families: two conversational source-memory controls and two explicit operator-import source-file cases. These family IDs do not occur in the existing paired cohorts.
- Task manifest v3, current v2 answer-format prompt, protocol, host policy, retry policy, simulator, task/oracle fingerprints, product binary and evaluator CLI SHA are frozen in this directory.
- Per-phase budgets remain 8 turns, 8 tool calls, 1,024 output tokens, 120 seconds and 15,360 context tokens. Runner retries are zero.

## Preregistered gates

Advance only if capture and offline comparison are valid and all gates pass:

1. Task-oracle success is at least 3/4 in each arm.
2. Independent fixed-query probes pass 4/4 in each arm.
3. Candidate source-file tasks pass at least 1/2.
4. Both candidate imports and extraction steps succeed (2/2).
5. Hard violations and budget-exceeded trials are both zero.

A failure is a no-go for this exact model/runner/sampler configuration. Do not retry the capture, tune the exposed cohort, or alter gates. Any further screen requires a materially justified configuration, new preregistration, and disjoint families. A separate powered comparison requires its own sample-size calculation and registered practical effect; it is not authorized by passing this screen.
