# Underdog Saluki local qualification v1

**Plan ID:** source-authority-local-underdog-saluki-json-2026-11-23-v1  
**Status:** frozen before loading the model and capturing trials.

## Question and limits

Can the locally installed Underdog Saluki 27B IQ2-mix model, connected through
the provider-neutral Chat Completions runner with the model card's general-use
sampler and the frozen JSON Schema response format, complete source-authority
tasks? Both arms use the same memory product binary, so this qualification
cannot estimate a memory treatment effect or general agent benefit.

The model card reports 88/120 tool-calling tasks on its own frozen BFCL-v4
subset versus 84/120 for full Qwen3.8-27B, and 96% average retention across
nine reported benchmarks. These are publisher-reported results from a
different harness, not evidence of performance on this cohort. The model is
CC BY-NC 4.0; the local evaluation is non-commercial.

## Frozen design and gates

- Four new synthetic families disjoint from all prior cohorts: two literal
  conversational formations and two candidate operator file imports.
- Same model alias, endpoint, product binary, tool schema, prompt/policy,
  sampler, response format and budgets in both arms.
- At least 3/4 task-oracle successes in each arm.
- Independent fixed-query probes 4/4 in each arm.
- Candidate source-file task success at least 1/2.
- Candidate operator imports and extractions 2/2; zero hard violations and
  zero budget-exceeded trials.
- One capture only. No retries, prompt edits, sampler changes or reuse of
  exposed families. A failed screen is a configuration-specific NO-GO.

A pass permits only a separately preregistered powered study; it does not
establish memory improvement or agent benefit.
