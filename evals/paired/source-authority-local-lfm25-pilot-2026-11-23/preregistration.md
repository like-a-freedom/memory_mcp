# Preregistration: local LFM2.5 source-authority feasibility pilot

**Plan ID:** `source-authority-local-lfm25-1.2b-2026-11-23-pilot`  
**Stage:** development pilot  
**Independent families:** 4  
**Captures:** one; one attempt per arm and case; zero retries  
**Purpose:** assess local runner compatibility, completion, interaction cost, and observed end-to-end latency before considering a larger study

## Question and inference boundary

Can a small local instruction model complete basic source-authority workflows
through the provider-neutral Chat Completions adapter, including public MCP
tool calls, exact source citations, operator-imported fixtures, and a fixed
post-workflow memory probe, at materially lower wall time than the earlier
27B-model run?

This pilot does **not** compare LFM with Qwen on the same registered cohort,
estimate a memory treatment effect, establish statistical reliability, or
support a general agent-quality claim. Both arms use the same current Memory
MCP product binary; their workflow differs in who prepares the source material.
Four families and one capture are only a feasibility screen.

## Frozen cohort and procedure

The four new synthetic families are two conversation controls and two
source-file imports. They do not reuse the exposed v3 task families. Each
family gets a fresh isolated store per arm. The manifest counterbalances arm
order. The incumbent agent resolves a synthetic relative file and calls the
public `ingest` and `extract` tools. The candidate imports the same bytes with
the root-confined operator command, then uses a fresh read-only agent session.
Both arms finish with the registered independent public-MCP probe. Shell,
external network fetch, and retries are disabled.

Prompts, tool allowlists, budgets, policy, product binary/config/schema,
fixtures, answers, oracle, endpoint, model alias, and runner fingerprints are
fixed in the plan and freeze manifest before these four task families are sent
to the model. All content is synthetic. The local provider is called only at
numeric loopback. This endpoint restriction proves the runner's destination,
not the provider's internal traffic behavior.

## Provider identity and measured limits

LM Studio metadata reports model key `liquid/lfm2.5-1.2b`, 1.2B parameters,
8-bit quantization, tool-use training, and a maximum context of 128,000 tokens.
The loaded alias is `memory-local-eval-lfm25` with a runtime context of 16,384
tokens and one parallel request. The CLI commit is `dcefd5a`. LM Studio does
not provide immutable model-weight or server-build hashes in the queried local
inventory, so the alias and provider inventory are fingerprinted and that
identity limit is explicit. The runner budgets at most 15,360 input tokens,
1,024 output tokens, eight tool calls and turns, and 120 seconds per phase.

## Outcomes and reporting

Keep all four families in the denominator. Report arm-level completion,
model/tool failures, task-oracle success, agent-observed evidence, independent
probe success, hard violations, operator setup outcomes, turns, tool calls,
tokens, and elapsed time. Report model-generation timing separately when the
provider exposes it; do not compare one small smoke call directly with the
full Qwen capture as if they were matched workloads. A successful capture
would justify using LFM for fast iteration and planning a larger disjoint
pilot. It would not prove memory improvement, agent benefit, repeatability,
non-inferiority, or population performance. Any larger study requires its own
frozen registration and independent cases.
