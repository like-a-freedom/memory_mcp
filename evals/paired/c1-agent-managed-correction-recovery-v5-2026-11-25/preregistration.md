# Preregistration: C1 agent-managed correction recovery — v5

**Predecessors:** v1 was an invalid setup and yielded zero model turns. The frozen v2 capture remains NO-GO and unchanged; its retry-exposure requirement was unreachable because the v5 prompt prohibited retrying partial reads. The v3 registration was rejected before any Codex trial: `agent_maintenance_v6` passed identity validation but was missing from the paired writer-profile admission lists. Its only capture-path files are two zero-byte receipts; no model-turn or trial artifact exists. This v5 registration uses disjoint synthetic histories after the v4 run was blocked by the workspace sandbox before any agent turn. The new temporary Codex home avoids writes to the user's normal Codex state. No exposed v4 family is reused.

**Plan ID:** `c1-agent-managed-correction-recovery-v5-2026-11-25`  
**Stage:** development pilot  
**Independent families:** 4  
**Captures:** capture 01; capture 02 only if every capture-01 gate passes  
**Profile:** `agent_maintenance_v6`

## Question and scope

On four new, disjoint synthetic histories, can the v6 agent workflow apply one explicit correction, verify it, invalidate only the superseded fact, and answer the later question from corrected memory? The v6 recall prompt permits one query-only refinement after a successful empty partial read only when the exact product guidance permits it. It preserves the temporal snapshot and every other request argument. Failed, nonempty, unsupported, or unapproved reads must not be retried.

This is workflow qualification on a four-family development pilot. Both arms run the same product binary, configuration, public MCP schema, model alias, profile, task inputs, and budgets. Arm labels only vary order. Results do not estimate a treatment effect, population benefit, or general memory quality.

## Frozen data and execution

`tasks.json` contains four synthetic organizations, old facts, corrections, update instructions, event IDs, valid-time boundaries, questions, and exact answer/source oracles. All names, values, IDs, and event timestamps are fresh relative to archived C1 cohorts. No project, personal, customer, or production memory is included.

Run the harness and memory server locally on native Apple Silicon. The optional Codex CLI adapter sends only this registration's synthetic task content to the pinned model alias `gpt-6.1-sol`. The CLI executable version and SHA-256, product and evaluator binary hashes, prompt/profile identity, plan, task/oracle hashes, policies, and budgets are frozen in `plan.json`, `fingerprints.json`, and `freeze-manifest.sha256`. Each Codex subprocess uses a fresh temporary `HOME` and trial workspace. For this capture, only the local `auth.json` is copied into a private temporary `CODEX_HOME`; the normal Codex state and user configuration are not used. The temp auth copy and state are deleted after the run. Host skill discovery and `skill_search` are disabled. A PONG-only synthetic smoke on the pinned local CLI returned successfully with no personal task data; the CLI emitted its expected warning that `skip_host_skill_discovery` is an under-development feature. The Codex session is ephemeral and read-only; shell, web, apps, plugins, memories, and permission tools are disabled. No LM Studio process, CLI, or endpoint is used.

Each task starts from a fresh isolated store. One attempt is permitted per arm and family; runner retries are zero. Context budget is 65,536 tokens, tool-call and turn budgets are eight, output budget is 1,024 tokens, and each phase has a 120-second wall-time limit. Receipts retain content-free read hashes/status and no raw query, returned text, or fact IDs. A private scorer verifies the fact ID from a successful nonempty read response against the superseded-source oracle before accepting an invalidation. The independent probe is a separate evaluator read through the public memory interface.

## Gates and stop rule

Capture 01 passes only when each arm meets every criterion in `criteria.json`:

- at least 3/4 exact task answers with the required citation;
- 4/4 independent probe successes;
- 4/4 successful correction extractions and exact stale-fact invalidations;
- 4/4 private exact-target checks proving the superseded fact was observed before invalidation;
- 4/4 evidence responses observed before invalidation;
- at least one observed agent read per trial and complete read telemetry;
- zero unauthorized bounded retries, unsupported/prohibited sources, hard violations, budget exits, or timeouts.

There is deliberately no retry-exposure floor: a permitted retry is conditional on the actual read response, so it is not guaranteed to occur. `evaluate_c1.py` reports the number of exact, authorized retries and the number of first reads whose query hash exactly matches the registered question hash. These are descriptive diagnostics, not gates. It also validates every observed retry against the exact partial-read guidance hash, empty/partial first-read state, changed query digest, and unchanged non-query digest. Deterministic evaluator tests are `test_validate_recovery.py` and `test_evaluate_c1.py`.

After capture 01, run `compare-agent-managed` and `evaluate_c1.py`. Capture 02 may run once only when every capture-01 gate passes. If any gate fails, preserve the run and stop; do not change prompts, tasks, profile, code, budgets, model, or criteria after exposure. The repeat remains four independent families.

## Interpretation

Report exact task success, first-read exact-question matches, returned evidence, independent probe success, writes/invalidation, violations, read outcome/completeness, latency, turns, tool calls, and token counts separately. Both arms are same-product workflow observations, not a memory treatment comparison. A passing four-family screen qualifies only this narrowly registered workflow; it does not establish an improvement in general memory quality or agent utility.
