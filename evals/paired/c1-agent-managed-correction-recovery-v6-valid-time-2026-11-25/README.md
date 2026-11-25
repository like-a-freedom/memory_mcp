# C1 agent-managed correction recovery — v6 valid-time diagnostics

**Status: registered; capture 01 not yet run.** This is a fresh four-family synthetic diagnostic cohort prepared after v5 showed empty, partial reads but could not distinguish query formulation from selector mismatch. See the preregistration and the eventual `capture-01/REPORT.md` for results.

The experiment asks whether an agent can apply one explicit correction safely, verify it, invalidate only the superseded fact, and recall the corrected value. It adds content-free diagnostic checks for whether each memory-read request and returned snapshot use the registered valid time. These checks are descriptive and do not change C1 promotion gates.

## Execution boundary

Run only on local Apple Silicon. The run uses the pinned local Codex CLI and model alias, but no LM Studio process, CLI, or endpoint. Remote inference receives only the four synthetic histories and fixed prompts registered here. A private temporary Codex home contains a temporary copy of local authentication material; its state is isolated and removed at exit. The normal Codex state and user configuration are not used. No real user, project, customer, or production memory is included.

Both arms use the same product binary, configuration, MCP schema, profile, model, prompts, tasks, and budgets. This is a workflow qualification, not a treatment-effect estimate. Capture 02 is allowed only if every frozen capture-01 gate passes.

## Valid-time diagnostic

The private task oracle and independent probe must register the same non-empty `valid_at`. The evaluator reports per arm the counts of requested and returned snapshot times that match, mismatch, or are unknown. Persisted read receipts contain only these booleans plus the existing query and non-query digests; raw timestamps, queries, returned memory text, and fact IDs are not added. A request mismatch points to agent-side selection; a returned-snapshot mismatch points to server-side selection; matches with empty results leave query/other selector behavior unresolved. The diagnostic itself is not a gate and cannot justify a product change without supporting evidence.

## Reproduction

From the repository root, run `bash evals/paired/c1-agent-managed-correction-recovery-v6-valid-time-2026-11-25/run-command.txt`. It verifies the frozen manifest, refuses to overwrite capture output, runs capture 01, compares paired artifacts, and writes a deterministic evaluation report. It does not automatically run capture 02.
