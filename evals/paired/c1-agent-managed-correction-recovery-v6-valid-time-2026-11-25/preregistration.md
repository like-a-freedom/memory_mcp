# Preregistration: C1 correction recovery with valid-time diagnostics — v6

**Plan ID:** `c1-agent-managed-correction-recovery-v6-valid-time-2026-11-25`  
**Stage:** development pilot  
**Independent synthetic families:** 4  
**Capture:** 01 only; capture 02 only if every capture-01 gate passes  
**Memory profile:** `agent_maintenance_v6`  
**Artifact schema:** `memory-mcp-agent-paired/v9`

## Question and scope

Can an agent apply one explicit correction safely, verify the corrected fact, invalidate only its exact superseded fact, and answer a later question from corrected memory? The four new synthetic histories cover Mossvale Glacier Herbarium, Amberwick Tidal Observatory, Bellpine Meteorology Lab, and Wrenfield Coastal Seed Bank. Their facts, IDs, sources, and values are disjoint from previous C1 cohorts.

The diagnostic addition tests whether the first and any subsequent memory-read request use the registered valid time, and whether the response snapshot reports that same valid time. The private oracle's `read_expectation.valid_at` must equal the probe's `valid_at` before a run is scored. For each read the artifact stores only `requested_valid_at_match` and `returned_valid_at_match` booleans (or null when unavailable). No raw query, timestamp, returned text, or fact identifier is persisted for this diagnostic. These counts are descriptive, are not promotion gates, and do not estimate a treatment effect.

This is a narrowly scoped same-product workflow qualification on four families. Both arms use identical product binaries and configuration, public MCP schema, model alias, profile, prompts, task data, and budgets; only the registered arm order varies. It does not establish general memory quality or agent utility.

The registered product binary and product configuration hashes are identical to C1 v5. This capture adds telemetry and uses a fresh cohort; it does not introduce a memory or response-behavior intervention relative to v5.

## Frozen inputs and execution

The exact tasks and private answer/evidence oracle are in `tasks.json`; gates are in `criteria.json`; arm identities and budgets are pinned in `plan.json`. `fingerprints.json` records their hashes plus the evaluator, validator, binary, profile, prompt, protocol, and environment identities. `freeze-manifest.sha256` records the immutable source and run inputs. The evaluator is a standalone Python program over artifact files; Codex is only the registered agent runner for this local scenario, not a scoring dependency.

Execution is restricted to local native Apple Silicon. The pinned Codex CLI is invoked with the registered model alias. Only the four synthetic histories and fixed prompts are sent for remote inference. LM Studio is not used. The runner uses an isolated temporary home/state and workspace, copies local authentication material into that temporary home for the invocation, and removes it on exit. User Codex configuration and state are not used or written. Host skill discovery, shell, web, applications, plugins, memories, and permission tools are disabled. No actual personal, project, customer, or production memory is read or sent.

Each task starts from an isolated store. The frozen run permits one attempt per arm and family, no runner retries, an 8,192-token independent-probe context budget, 65,536-token agent context budget, eight tool calls, eight turns, 1,024 output tokens, and a 120-second phase wall-time limit. Preserve all capture artifacts; do not overwrite or rerun after a no-go result.

## Frozen gates and stop rule

Capture 01 passes only if **each arm** meets every criterion in `criteria.json`:

- at least 3/4 exact task answers with the required source citation;
- 4/4 independent-probe successes;
- 4/4 successful exact-once correction extractions and invalidations;
- 4/4 exact stale-fact target checks proving the fact was observed before invalidation;
- 4/4 evidence responses before invalidation;
- one or more observed agent reads per trial and complete read telemetry;
- zero unauthorized retries, unsupported/prohibited sources, hard violations, resource exits, or timeouts.

There is no retry-exposure floor because a permitted query refinement is conditional on the observed response. The first-read exact registered-question match and valid-time match counts are diagnostic only. Valid-time mismatch counts never override a task or independent-probe failure and never authorize a code change by themselves.

After capture 01, `compare-agent-managed` checks paired artifacts and the standalone `evaluate_c1.py` applies the frozen deterministic gates. Capture 02 may run once only if the report says `eligible_for_capture_02=true`. Any failed gate stops the experiment. Do not alter task wording, tasks, prompts, profile, memory implementation, binaries, budgets, model alias, or criteria after exposure.

## Reading the diagnostic

- Requested-time mismatch: the agent submitted a selector different from the private oracle time.
- Requested match with returned-snapshot mismatch: server-side temporal selection or snapshot reporting is suspect.
- Both match with empty/partial evidence: query formulation or another selector/channel remains unresolved.
- Missing match: report as unknown; do not infer success.

These interpretations localize the next investigation; they are not causal proof. Any memory change still requires a new preregistered deterministic evaluation and regression checks.
