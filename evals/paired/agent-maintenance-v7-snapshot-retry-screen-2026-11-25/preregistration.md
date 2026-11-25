# A11 snapshot-bound retry model screen

**Registration:** frozen on 2026-11-25 before model capture. One capture only.

## Question

On four new synthetic correction histories, does the current maintenance agent follow the snapshot-bound retry contract when an eligible empty partial read occurs? Record the actual read evidence, task answer, independent post-write probe, safety outcomes, and read latency separately.

This is a development feasibility screen. Both arms use the same memory binary and configuration, so it cannot estimate a memory treatment effect or population-level usability. A capture with no eligible empty partial read is inconclusive about model adherence to the retry instruction.

## Frozen cohort and identity

`tasks.json` contains four synthetic families with new entities, prior values, correction values, source identifiers, and event identifiers. Each task registers the SHA-256 of the exact current empty-partial guidance and requires one retry whenever the tool response is eligible. `plan.json` pins the task order, `agent_maintenance_v7` profile identity, Codex CLI and memory-eval fingerprints, memory binary, tool schema, configuration, budgets, and native Apple Silicon environment. `freeze-manifest.sha256` covers all registration inputs and the runner.

The run uses the local `memory_mcp` binary and local embedded store on native Apple Silicon. Remote inference receives only this synthetic cohort and its fixed instructions. No LM Studio runtime is used. The Codex CLI receives no project or personal memory; it runs with a temporary home containing only a private copy of the existing authentication file, while user configuration and exec rules are disabled by the runner.

## Measurements and interpretation

Report, per arm:

- answer plus citation oracle outcomes and observed memory source coverage;
- each actual read's source evidence, completeness, and elapsed time;
- retry registration, eligible-trigger count, observed retries, and compliant/noncompliant transitions;
- correction extraction, exact invalidation, and independent probe outcomes;
- hard violations, timeouts/resource exits, turns, tool calls, tokens, and latency.

`NotEligible` means the tool did not return the registered empty partial guidance; it is not evidence that the model can follow the retry protocol. If an eligible trigger occurs, the required retry must be observed and pass the deterministic state-transition scorer. One successful answer does not override a retry violation or a failed independent probe.

There is no second capture under this registration. Any follow-up needs a materially changed, disjoint cohort frozen before another model call. Do not claim that memory quality or general agent usability improved from this same-product screen.
