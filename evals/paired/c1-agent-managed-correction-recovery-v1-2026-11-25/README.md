# C1 bounded read recovery — v1

**Status: invalid setup; retained unscored.** Capture 01 exited before any
model turn because the sandboxed Codex runner could not access the existing
local Codex state. The PONG-only synthetic host preflight succeeded. The
disjoint v2 cohort and its separate result are documented in
[`../c1-agent-managed-correction-recovery-v2-2026-11-25/README.md`](../c1-agent-managed-correction-recovery-v2-2026-11-25/README.md).

This frozen development screen asks whether an agent can perform a correction,
verify and invalidate the superseded fact, and answer a later question. Its
`agent_maintenance_v5` profile permits one query-only retry only after a
successful empty result whose exact guidance says to refine the query. Failed,
partial, nonempty, and unsupported reads must not be retried. It uses four
fresh synthetic history families.

The exact question, safety and stopping gates are in
[`preregistration.md`](preregistration.md). The plan and task manifest pin the
runner, model alias, prompts, profile, product binary/configuration, public
schema, budgets, source oracles and tool policies. The capture is eligible to
repeat once only if every capture-01 gate passes. A repeat does not add
independent families or support a causal product claim.

After capture 01, run `compare-agent-managed` and `evaluate_c1.py`. The gate
requires the frozen task floor, exact cited answers, independent probes that
match the hidden source oracle, exactly one successful extract and invalidate
per trial, fact evidence returned before invalidation, exact stale-fact target
verification, complete read telemetry, and at least one authorized bounded
retry per arm. The retry check requires exact empty-result guidance, a changed
query digest, and an unchanged digest for every other read argument. The gate
also rejects hard violations, unsupported or prohibited probe sources, and
timeouts or budget exits. Its report includes read outcomes, source coverage,
and latency. Tests are in `test_validate_recovery.py` and
`test_evaluate_c1.py`; Python's standard library is sufficient.

Before scoring, the evaluator checks that the registered task, criteria, plan,
recovery validator and evaluator files still match `fingerprints.json`. It also
checks each artifact's shared and product identity against the registered plan
and verifies exact trial coverage. The command file uses fail-fast shell mode,
so a no-go from capture 01 cannot fall through to capture 02.

The Codex parser keeps fact IDs and their source episode IDs only in transient
process memory. It scores whether a successful nonempty read response exposed
the exact fact ID from a source the frozen oracle marks as superseded before
the single invalidate request for that ID. Event sequence numbers and IDs
remain transient. The v6 stage sidecar stores only that boolean; neither fact
IDs nor read content are added to persisted receipts. Older v1-v5 sidecars
remain readable under their original schemas.

The code, scorer, memory server and persistence run locally on native Apple
Silicon. The authorized Codex CLI adapter uses the existing local Codex login
for remote inference on these synthetic tasks. No LM Studio process, CLI or
endpoint is used. The deterministic memory probe and offline comparator do not
need remote inference.

A temporary empty-history public-probe preflight confirmed the product tool
schema and launch profile before this registration: no task values were used,
no model was called, the response contained zero source events, and the schema
SHA-256 matched the pinned plan. The preflight request, response and empty
history are retained under [`preflight/`](preflight/).
