# C1 correction-recovery v7: selector diagnostics

This is a fresh, disjoint synthetic screen following the v6 valid-time pilot. It adds field-level comparison of the agent's actual `assemble_context` selectors against the private task registration. The four scenarios are Highmere Planetarium, Rillbank Orchid Seed Vault, Caldera Bird Observatory, and Fernwick Tidal Lab. Their entities, values, sources, event IDs, and correction histories are new.

## What this screen measures

For every actual read, the v10 paired artifact records only `selector_expectation_registered` and a closed map of per-field match booleans for `fact_types`, `as_of`, `valid_at`, `known_at`, `known_revision`, `budget`, `view_mode`, `window_start`, `window_end`, and `compact`. The scorer normalizes values using the product's effective defaults and timestamp rules. The deterministic evaluator reports match, mismatch, and unknown counts per field and arm. These are diagnostic counts; they do not change the frozen C1 task-success gate. Query equality, valid-time match, returned-snapshot match, read outcome, source coverage, and latency remain separate measures.

The selector oracle is complete: empty/default values are registered explicitly. The agent and persisted observation receive only the normal public tool schema and task prompt. Raw queries, selector values, returned memory, and oracle contents are not added to the scored diagnostics.

## Frozen decision rule

Capture 01 is one local native Apple Silicon run. Capture 02 is permitted only when every registered criterion passes for both arms. A failed gate is NO-GO and stops the run. Selector-field mismatches localize the request shape but do not alone authorize a product change. Any proposed memory change needs an independently reproduced product-level defect and a deterministic guard.

## Reproduce

From the repository root, run:

```sh
bash evals/paired/c1-agent-managed-correction-recovery-v7-selector-diagnostics-2026-11-25/run-command.txt
```

The script verifies `freeze-manifest.sha256`, refuses to overwrite capture output, runs the paired Codex adapter using the pinned synthetic plan, compares the artifacts, and writes the deterministic C1 report. It does not use LM Studio and does not start capture 02 automatically. Only these four synthetic histories and fixed prompts are sent for remote inference. The scorer and report are offline and provider-independent.

See `preregistration.md` for the scope, budgets, gates, and stop rule. `capture-01/REPORT.md` records the result after execution.
