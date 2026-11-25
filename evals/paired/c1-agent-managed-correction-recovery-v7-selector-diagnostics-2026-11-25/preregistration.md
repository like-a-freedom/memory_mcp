# Preregistration: C1 correction recovery with selector diagnostics — v7

**Plan ID:** `c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25`  
**Stage:** development pilot  
**Independent synthetic families:** 4  
**Capture:** 01 only; capture 02 only if every capture-01 gate passes  
**Artifact schema:** `memory-mcp-agent-paired/v10`  
**Memory tool profile:** `agent_maintenance_v6` (unchanged configuration)

## Question and scope

Can an agent apply one explicit correction safely, verify the corrected fact, invalidate only its exact superseded fact, and answer a later question from corrected memory? Four disjoint synthetic histories cover a planetarium, a seed vault, a bird observatory, and a tidal laboratory. Each history has one existing durable fact and one explicit user correction. The cohort is distinct from C1 v1–v6 in names, values, source IDs, event IDs, and story content.

The diagnostic question is which effective `assemble_context` selectors the agent actually used on each read. The private oracle registers the full fixed allowlist: `fact_types`, `as_of`, `valid_at`, `known_at`, `known_revision`, `budget`, `view_mode`, `window_start`, `window_end`, and `compact`. Every actual read in paired v10 records a registration boolean and a per-field match map. The evaluator reports each field's match/mismatch/unknown count by arm. The field diagnostics are descriptive and are not task-success gates. The aggregate query fingerprint, exact-query match, requested/returned valid-time matches, result outcome, source coverage, and recall latency are also kept as separate measures.

The scorer normalizes default and equivalent selector values against the product contract. Omitted default fields have effective values: empty `fact_types`, no time/revision/window boundary, budget 5, default view, and compact true. The Caldera task registers a non-UTC RFC3339 offset to exercise instant normalization. Raw query text, selector values, returned content, episode IDs, and oracle text are not added to the new receipt fields.

This is a same-product workflow screen on four families. It is not a treatment-effect estimate and does not establish general agent utility. No product memory behavior is changed by this registration.

## Frozen inputs and execution

`tasks.json` contains the four synthetic histories and private answer, evidence, time, and selector oracle. `criteria.json` contains the fixed gates and selector field allowlist. `plan.json` pins task order, model alias, runner, binaries, product config, tool schema, prompts, budgets, and features. `fingerprints.json` and `freeze-manifest.sha256` bind all registered files and executable identities before any model call.

Execution is restricted to native Apple Silicon (`aarch64-apple-darwin`). The registered Codex CLI and model alias run locally as the harness; only the four synthetic histories and fixed prompts are sent for remote inference. LM Studio is not used. The evaluator runs locally offline. The Codex adapter uses a temporary private home for the run and removes it on exit. No personal, customer, project, or production memory is read or sent.

Each task gets a fresh isolated store. The frozen run allows one attempt per arm and family, no runner retries, an 8,192-token independent-probe context budget, 65,536-token agent context budget, eight tool calls, eight turns, 1,024 output tokens, and a 120-second phase wall-time limit. Preserve every artifact; do not overwrite or rerun after a no-go result.

## Gates and stop rule

Capture 01 passes only if each arm satisfies every criterion in `criteria.json`: at least 3/4 exact cited task answers; 4/4 independent probes; 4/4 exact-once correction extractions and invalidations; 4/4 verified exact stale-fact targets and pre-invalidation evidence; complete observed read capture; and zero unauthorized retries, unsupported/prohibited sources, hard violations, resource exits, or timeouts.

After capture 01, `compare-agent-managed` and the standalone `evaluate_c1.py` validate artifacts and apply the deterministic gates. Capture 02 may run once only when `eligible_for_capture_02=true`. Any failed gate stops the experiment. Do not change task wording, histories, prompts, profile, code, binary, budgets, model alias, or criteria after exposure.

Selector diagnostics do not affect eligibility. A consistent field mismatch identifies a request-shape difference to investigate. It does not prove that the mismatch caused task failure or authorize a memory change. A product fix requires a reproduced defect at the public memory seam and deterministic product-level regression coverage.
