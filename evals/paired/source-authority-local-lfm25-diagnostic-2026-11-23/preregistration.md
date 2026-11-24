# Preregistration — LFM2.5 failure-class diagnostic

**Status before run:** frozen; no model call has been made with this registration.  
**Registration:** source-authority-local-lfm25-diagnostic-2026-11-23  
**Purpose:** use the newly available content-free failure classes to diagnose why the prior four-family LFM pilot had zero task-oracle successes. This is a diagnostic repeat on four newly authored synthetic families, not a workflow acceptance trial.

## Frozen design

- One paired capture, four independent synthetic families, one repetition per family.
- Same product binary/configuration in both arms. The experiment cannot estimate a memory treatment effect.
- Same LFM alias, local numeric-loopback Chat Completions endpoint, prompts, task rules, protocol, budgets, host policy, simulator and zero-retry policy as the prior pilot.
- New facts, family IDs, source paths and event IDs. None of these families appeared in prior model captures.
- The adapter stores normalized event types, token/tool/time counts, hashes and closed error categories only. It does not store raw model output.
- No tuning, retries, substitutions or repeated capture on these families. If preflight rejects the frozen files before a model call, correct the registration and re-freeze first.

## Questions and interpretation

1. Do model turns reach the provider and return a response envelope accepted by the adapter?
2. If a trial does not complete, does its closed category identify an invalid response envelope, final-answer schema, tool-call schema, timeout, budget exhaustion or another registered class?
3. Do any trials complete the task oracle under the frozen workflow?

This sample is descriptive only. Even a 4/4 pass would establish no general model reliability, usability, memory improvement, or promotion criterion. The prior model result remains a no-go until a separate powered, preregistered comparison with a qualified workflow model passes. These artifacts must not be included in confirmatory sample-size estimation.

## Data boundary

All task text is synthetic. The endpoint is numeric loopback; no source corpus, user memory or credentials are supplied. LM Studio is only the selected local provider for this capture. The runner contract and deterministic tests remain provider-neutral and reproducible with any compatible local Chat Completions service.
