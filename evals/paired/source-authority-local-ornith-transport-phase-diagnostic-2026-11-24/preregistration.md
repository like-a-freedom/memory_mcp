# Ornith transport-stage diagnostic preregistration

**Plan ID:** `source-authority-local-ornith-35b-a3b-2026-11-24-transport-phase-diagnostic`  
**Stage:** one-capture development diagnostic; not powered confirmation  
**Frozen before model-task capture:** 2026-11-24 UTC (after pre-capture audit below)

## Question

For transport failures in a fresh source-authority screen, does the provider-neutral runner fail during request send or while reading the response body after successful HTTP headers? The runner retains only one of the closed values `request_send` and `response_body_read`; it does not persist HTTP response content.

This is diagnostic only. Both arms use the same memory binary, store configuration, model alias, sampler, prompts and budgets. The contrast does not estimate a memory treatment effect. The earlier HTTP-status diagnostic and all other cohorts are exposed and immutable; all family IDs in this registration are new.

## Frozen configuration

- Model alias: `memory-local-eval-ornith-v2` on the local OpenAI-compatible Chat Completions endpoint.
- Endpoint: numeric loopback `http://127.0.0.1:1234/v1`; no bearer token is configured or required for this local endpoint.
- Sampler: `temperature=0.6`, `top_p=0.95`, `top_k=20`.
- Task manifest v3, source-authority prompts, retry policy, host policy, simulator, product configuration, product binary and evaluator binary are fingerprinted in `plan.json` and `fingerprints.json`.
- Four fresh synthetic families: two literal conversational memory controls and two operator-import source-file cases.
- One registered capture, no retries, 8 turns, 8 tool calls, 1,024 output tokens, 120 seconds and 15,360 context tokens per phase.
- Model weights and LM Studio server build are not independently fingerprinted; exact alias, LM Studio CLI commit, loaded context and provider inventory are recorded. No model output text will be stored.

## Diagnostic recording and gates

For each terminal transport failure, capture only normalized error class and the closed stage. `request_send` means the request did not yield a usable HTTP response; `response_body_read` means successful response headers were received but body reading failed. A non-success HTTP response has its separate numeric-status diagnostic and no transport stage. Never retain response headers, bodies, prompts, tool arguments or memory contents in stage receipts.

The capture is valid only if complete paired artifacts are emitted and the offline comparator accepts them. Apply these frozen development-screen gates as descriptive qualification evidence:

1. Task-oracle success is at least 3/4 in each arm.
2. Independent fixed-query probes pass 4/4 in each arm.
3. Candidate source-file tasks pass at least 1/2.
4. Both candidate imports and extractions succeed (2/2).
5. Hard violations and budget-exceeded trials are both zero.

A failed gate is a no-go for this exact model/runner/sampler configuration. Never rerun this exposed cohort or tune its tasks. A later qualification needs fresh families and a new registration. Passing this diagnostic screen does not substitute for a separately powered confirmation, sample-size calculation, or memory-effect estimate.

## Reproduction commands

From the repository root, verify the registered input hashes first:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/freeze-manifest.sha256
```

Then run the one registered capture:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01"
```

After capture, compare only the registered artifacts offline:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- compare-agent-managed \
  --plan evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/plan.json \
  --incumbent evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01/incumbent.json \
  --candidate evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01/candidate.json \
  --incumbent-stages evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01/agent-managed-incumbent.json \
  --candidate-stages evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01/agent-managed-candidate.json \
  --output evals/paired/source-authority-local-ornith-transport-phase-diagnostic-2026-11-24/runs/run-01/agent-managed-comparison.json
```

Do not rerun the cohort or use its families for qualification after the first model-task request.
