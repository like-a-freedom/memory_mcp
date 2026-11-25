# C1 bounded read recovery — v2

**Decision: NO-GO; capture 01 completed on native Apple Silicon.** Capture 02
was not run because the frozen gates failed. The v1 registration remains an
invalid setup artifact and is not pooled with this run.

The v1 registration remains archived as an invalid setup: Codex exited before
any model turn because the sandboxed runner could not access the user's local
Codex state. A PONG-only synthetic preflight succeeded with direct host launch.
This v2 registration therefore uses four entirely fresh synthetic histories,
new entity names, values and event IDs; v1 receipts are not pooled or scored.

This screen tests whether an agent can apply one explicit correction, verify
and invalidate only its superseded fact, and answer a later question. The
`agent_maintenance_v5` profile permits one query-only retry only after a
successful empty result with exact refinement guidance. Failed, partial,
nonempty and unsupported reads must not be retried.

After capture 01, run `compare-agent-managed` and `evaluate_c1.py`. Eligibility
requires the frozen task floor, exact cited answers, independent probes matched
to the hidden source oracle, exactly one successful extract and invalidate per
trial, evidence returned before invalidation, exact stale-fact target and event
order, complete read telemetry, at least one authorized retry per arm, and no
safety or resource violations. The gate pins all inputs and evaluator code by
SHA-256 and matches artifact identity to the registered plan. Capture 02 is
allowed once only after every capture-01 gate passes.

All harness and memory-server work runs locally on native Apple Silicon. The
optional Codex CLI uses the existing local login and receives only this
registration's synthetic task content. Its agent sandbox remains read-only;
no shell, web, apps, plugins, memories or permission tools are enabled. No LM
Studio process, CLI or endpoint is used.

## Capture 01 results

| Arm | Exact task answers | Agent reads | Independent probes | Extracts | Invalidations | Evidence before invalidation | Exact stale-target order | Authorized retries | Hard violations | Resource exits | Recall mean / median / max |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Incumbent | 0/4 | 0/4 nonempty | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 0/4 | 0 | 0 | 17,330 / 17,117 / 19,800 ms |
| Candidate | 0/4 | 0/4 nonempty | 4/4 | 4/4 | 4/4 | 4/4 | 4/4 | 0/4 | 0 | 0 | 14,815 / 14,846 / 16,376 ms |

All eight agent sessions completed without a timeout or hard violation. The
agent read returned an empty result in all four cases per arm, and every final
answer abstained rather than guessing. The fixed independent probe found the
new correction source in all eight trials. This isolates a failure at the
agent-read/answer boundary after successful correction, extraction,
verification and invalidation. Both arms used the same product binary and
configuration; these numbers do not estimate a treatment effect.

## Qualification defect found

The frozen evaluator correctly rejected this cohort, but its retry exposure
floor was not reachable under the registered read contract. The recall system
prompt forbids retrying a partial read. All eight agent reads were empty with
`completeness=partial`, and their guidance digest matches the product's
registered partial-history guidance: “Historical context is partial; use the
returned snapshot and typed relation roles, then verify omitted selector
channels before relying on an empty or reduced result.” That guidance does not
explicitly authorize a query-only retry. The agent's zero retries therefore
followed the protocol; the `minimum_guidance_authorized_retry_trials_per_arm`
gate was inconsistent with this cohort and cannot be interpreted as an agent
failure. Keep the preregistered NO-GO result unchanged; correct this gate in a
new registration by using an eligible retry fixture or by testing only that
unauthorized retries never occur.

The receipts retain query and non-query argument hashes, not raw query text.
The recorded query hashes differ from the registered natural-language
questions, so the agent did not pass those exact strings to the tool. The
current content-free artifact cannot tell whether the paraphrases omitted an
entity/fact anchor or changed a temporal/selector argument. Before making a
memory retrieval change, add a privacy-preserving, deterministic query-shape
diagnostic for synthetic cases; do not infer a specific retrieval bug from
the empty outcomes alone.

The offline evaluator wrote
[`capture-01/evaluation.json`](capture-01/evaluation.json), and the Rust
comparison wrote
[`capture-01/agent-managed-comparison.json`](capture-01/agent-managed-comparison.json).
The 566-entry source/input freeze manifest still verifies after capture. C2
remains ineligible until a materially changed workflow passes a fresh C1
qualification.
