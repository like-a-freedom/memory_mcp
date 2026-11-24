# Ornith HTTP-status diagnostic preregistration

**Plan ID:** `source-authority-local-ornith-35b-a3b-2026-11-24-http-status-diagnostic`  
**Stage:** one-capture development diagnostic; not powered confirmation  
**Frozen before model-task capture:** 2026-11-23 UTC

## Question

Does a fresh, disjoint four-family source-authority screen produce provider
HTTP status codes for the `transport` formation failures observed in the prior
Ornith cohort, now that the runner records a numeric status without retaining
response content?

This is diagnostic only. Both arms use the same memory binary, store
configuration, model alias, sampler, prompts and budgets. The contrast does not
estimate a memory treatment effect. The prior corrected-sampler cohort is
exposed and immutable; all four family IDs here are new.

## Frozen configuration

- Model alias: `memory-local-eval-ornith-v2` on the locally configured LM
  Studio OpenAI-compatible Chat Completions endpoint.
- Endpoint: numeric loopback `http://127.0.0.1:1234/v1`; no bearer token is
  configured or required for this local endpoint.
- Sampler: `temperature=0.6`, `top_p=0.95`, `top_k=20`.
- Task manifest v3, source-authority prompts, retry policy, host policy,
  simulator, product configuration, product binary and evaluator binary are
  fingerprinted in `plan.json` and `fingerprints.json`.
- Four fresh synthetic families: two literal conversational memory controls
  and two operator-import source-file cases.
- One registered capture, no retries, 8 turns, 8 tool calls, 1,024 output
  tokens, 120 seconds and 15,360 context tokens per phase.
- Model weights and LM Studio server build are not independently fingerprinted;
  the exact alias, LM Studio CLI commit, loaded context and provider inventory
  are recorded. No model output text will be stored.

## Diagnostic recording and gates

For each formation failure, record the closed normalized error class and, only
when the provider returned non-success HTTP, its numeric status code. Do not
read, log, hash, or persist response headers or bodies. A missing status means
the observed failure did not provide a non-success HTTP response to the
adapter; it does not distinguish send failure from response-body read failure.

The capture is valid only if the runner emits complete paired artifacts and
the offline comparator accepts them. Apply these frozen development-screen
gates as descriptive qualification evidence:

1. Task-oracle success is at least 3/4 in each arm.
2. Independent fixed-query probes pass 4/4 in each arm.
3. Candidate source-file tasks pass at least 1/2.
4. Both candidate imports and extractions succeed (2/2).
5. Hard violations and budget-exceeded trials are both zero.

A failed gate is a no-go for this exact model/runner/sampler configuration.
Never rerun this exposed cohort or tune its tasks. A later qualification needs
fresh families and a new registration. Passing this screen does not substitute
for a separately powered confirmation, sample-size calculation, or
memory-effect estimate.
