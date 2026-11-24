# Preregistration amendment 02 — temporary-root preflight

Recorded 2026-11-22 before any valid v2 model turn.

The first direct-host launch passed the Codex startup boundary but stopped
before starting any trial because the requested temporary root did not yet
exist. A second CLI preflight then created only two empty receipt journals in
`preflight-setup-failure-01/` before rejecting the already-existing output
path. Both are setup failures: there are no Codex invocations, receipts,
model turns, MCP calls, or memory writes in this directory. It is retained for
audit and excluded from scoring along with `run-01` and `run-02`.

The preregistered task manifest, plan, binaries, prompts, scoring and protocol
are unchanged. The preflight-only output directory has been archived, so
`run-03` remains an unused output path. The two valid repeats will use
`run-03` and `run-04` with the existing temporary root
`/private/tmp/memory-mcp-maintenance-v2-run-03`. All other execution controls
remain as recorded in amendment 01.
