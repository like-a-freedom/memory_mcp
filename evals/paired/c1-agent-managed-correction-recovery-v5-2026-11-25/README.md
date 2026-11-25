# C1 agent-managed correction recovery — v5

**Status: NO-GO; capture 01 completed and the frozen stop rule prevented capture 02.** This is a fresh four-family development pilot after the v4 capture was blocked by the outer workspace sandbox before any agent turn. v5 uses disjoint synthetic histories and an isolated temporary Codex home. The preregistration remains the unchanged pre-run record; see the [capture report](capture-01/REPORT.md) and [registered evaluator result](capture-01/evaluation.json).

The experiment tests whether the agent can apply one explicit correction safely, verify it, invalidate only the superseded fact, and recall the corrected value in four fresh synthetic families. Both paired arms use the same product binary and configuration; this is a workflow qualification, not a treatment-effect estimate.

## Execution boundary

Run only on local Apple Silicon. Use the installed Codex CLI with the pinned version and model alias. The temporary Codex home contains only a temporary copy of the local authentication file; its HOME, state database and workspaces are temporary and removed after capture. Codex user configuration is ignored, shell/web/apps/plugins/memories/permission tools are disabled, and host skill discovery is disabled. Remote inference receives only the four registered synthetic histories and fixed prompts. No real user, project, customer or production memory is included. No LM Studio process, CLI or endpoint is used.

Capture 01 failed the registered task-success floor at 0/4 in both arms; capture 02 was therefore not run. The v4 blocked run is retained separately and is not a C1 outcome.

The correction lifecycle and independent probes passed 4/4 in both arms, but all 16 agent-facing reads were empty and partial, and all exact cited answers failed. First-query hashes matched across arms within each family but did not match the registered question; non-query argument hashes differed across arms in three of four families. Because raw query and selector arguments are intentionally absent, these observations do not distinguish query formulation from temporal or other selector mismatch. They do not justify a retrieval or response-wire change. The next diagnostic must compare registered and observed selector fields without retaining raw arguments.
