# A11 snapshot-bound retry model screen

A one-capture, four-family synthetic feasibility screen for the registered
`agent_maintenance_v7` snapshot-bound retry contract. Read
[`preregistration.md`](preregistration.md) before interpreting results.

## Reproduce

From the repository root on native Apple Silicon, after building `memory_mcp`
and `memory-eval` with the registered feature profile, run:

```sh
bash evals/paired/agent-maintenance-v7-snapshot-retry-screen-2026-11-25/run-command.txt
```

The runner verifies the frozen input hashes, refuses to overwrite a capture,
uses a temporary Codex home, and sends only the four synthetic histories for
remote inference. It does not use LM Studio. `compare-agent-managed` is an
offline deterministic replay and requires no model call.

## Scope

This is a development screen, not a product comparison: both arms use the same
memory binary and configuration. If none of the four tasks produces an
eligible empty partial response, the run is inconclusive about model adherence
to the retry contract. A repeat requires a new disjoint cohort and a new
registration.
