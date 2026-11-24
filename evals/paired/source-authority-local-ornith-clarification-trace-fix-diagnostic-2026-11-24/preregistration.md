# Ornith clarification-trace fix diagnostic preregistration

**Plan ID:** `source-authority-local-ornith-35b-a3b-2026-11-24-clarification-trace-fix-diagnostic`  
**Stage:** one-capture development diagnostic; not powered confirmation  
**Frozen before model-task capture:** 2026-11-24 UTC, after the pre-capture audit.

## Question

Does the deterministic clarification-counter/trace fix allow a fresh four-family paired capture to complete and pass its offline artifact comparator, and where are any terminal transport failures observed? This tests evaluator validity and runner diagnostics. It does not test a memory change: incumbent and candidate use the same memory binary, configuration and feature set.

## Frozen configuration

- Model alias: `memory-local-eval-ornith-v2`, at numeric-loopback endpoint `http://127.0.0.1:1234/v1`; no bearer token/API key is configured or needed.
- Sampler: `temperature=0.6`, `top_p=0.95`, `top_k=20`.
- Task manifest v3, prompts, retry policy, host policy, simulator, endpoint/sampler, product and evaluator identities are hash-bound in `plan.json`, `fingerprints.json` and `freeze-manifest.sha256`.
- Four new synthetic families: two conversation controls and two operator-import source-file cases. Family IDs: `stormpetal_index, ironwillow_registry, glassharbor_depot, bluecinder_lockbox.`
- Exactly one capture, no retries: 8 turns, 8 tool calls, 1,024 output tokens, 120 seconds and 15,360 context tokens per phase.
- Model weights and server build are not independently fingerprinted; alias, model inventory, CLI commit and loaded context are recorded. Raw model output is not retained.

## Validity and frozen gates

The diagnostic is valid only if both complete paired artifacts are emitted and `compare-agent-managed` accepts them offline. Record completeness, counter/trace validation and transport-stage counts. For descriptive workflow feasibility, use the same frozen screen: task-oracle success at least 3/4 per arm; independent probes 4/4 per arm; candidate source-file tasks at least 1/2; both candidate imports and extractions successful; zero hard violations and budget overruns. Failing a gate is a NO-GO for this exact workflow configuration, not a memory effect. Do not repeat, tune, or reuse these families. A pass would still not establish benefit or replace powered confirmation.

## Reproduction

Verify frozen inputs from the repository root:

```sh
shasum -a 256 -c evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/freeze-manifest.sha256
```

Capture once:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/plan.json \
  --tasks evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/tasks-v1.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/sampling-config.json \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01"
```

Compare the emitted artifacts offline once capture ends:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- compare-agent-managed \
  --plan evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/plan.json \
  --incumbent evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01/incumbent.json \
  --candidate evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01/candidate.json \
  --incumbent-stages evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01/agent-managed-incumbent.json \
  --candidate-stages evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01/agent-managed-candidate.json \
  --output evals/paired/source-authority-local-ornith-clarification-trace-fix-diagnostic-2026-11-24/runs/run-01/agent-managed-comparison.json
```
