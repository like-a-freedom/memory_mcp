# Current oracle contract correction — 2026-11-26

Local native Apple Silicon (`arm64`), deterministic public MCP; no model, remote
runner, API key or LM Studio. Production memory source was not changed.

## Root cause

The active simplification gate incorrectly reused v9. Its `office-entry`,
`similar-name` and `single-current` oracles require temporal `current` roles on
ordinary reads without explicit snapshot cutoffs. ADR-0083 forbids manufacturing
those roles, and the v10 baseline review on 2026-11-23 had already corrected this
oracle scope. Raw v9 reads have correct sources/assertions; all three explicit
snapshot diagnostics pass. Their ordinary `decision_unobservable` scores remain
preserved in the original archive and in individually named regression controls.
This is an evaluator correction, not three new production memory fixes.

## TDD and contract

The public CLI regression initially returned exit 0 when handed the incompatible
v9 oracle; the test expected exit 2. The current loader now reuses corpus validation
requiring snapshot cutoffs for expected temporal roles, even with oracle schema v1.
Archived registered loading remains supported. Current grading rejects invalid
scope before raw grading, and the native gate validates its baseline before
launching the product. Assertions, exact evidence alternatives, forbidden sources,
valid time, knowledge revision and negative regression controls are unchanged.
V10 has different fixtures: 5/8 v9 → 8/8 v10 is **not** a measured quality gain.

## Native evidence

The fresh baseline uses the unchanged product binary SHA-256
`4b1d1e878874e246a239a4900a04f54d2c00bcec3c319f910fbe8de53af84d3c`.
Both captures have the same product digest, corpus, configuration and public schema;
revision labels differ (`1e56a992` and `ci-current`). Raw receipts retain identities.

| Measure | Baseline | Candidate |
| --- | ---: | ---: |
| Primary evidence | 8/8 | 8/8 |
| Primary decisions | 8/8 | 8/8 |
| Primary complete probes | 8/8 | 8/8 |
| Temporal complete probes | 8/8 | 8/8 |
| Invalid / unobserved | 0 / 0 | 0 / 0 |
| Capture elapsed, ms | 52161 | 53254 |

[Offline comparison](../current-contract-candidate-2026-11-26/current-regression.json):
`all_cases_pass=true`, `no_regressions=true`, no current failures or regressions.
The native gate took 55.45 seconds including grading. These timings are observations,
not a controlled speed comparison. Empty partial snapshots support abstention;
they do not prove global absence.

Baseline [profile](profile.json), [run](run.json), [raw index](receipts/index.json)
and all sibling receipts are retained. Candidate [run](../current-contract-candidate-2026-11-26/current/run.json)
and [raw index](../current-contract-candidate-2026-11-26/current/index.json) are
retained. Active reproduction instructions are in [the eval runbook](../../../docs/evals/README.md).
Use fresh output directories; do not replace the frozen baseline automatically.

## Verification

- Complete eval-harness: 904 passed, 22 ignored, zero failures across 41 test targets.
- Native ignored current gate: 1 passed, 8/8 complete cases and no regressions.
- Required workspace all-target clippy with fs-watch,mcp-apps,streamable-http,test-fixtures
  and `--locked -- -D warnings`: passed after removing an unused test import.
- Formatting and diff whitespace checks passed.

These exposed finite synthetic cases do not establish model-mediated agent utility,
population-level quality or external benchmark improvement.
