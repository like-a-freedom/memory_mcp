# Bounded fact evidence in `extract` responses

**Status:** Implemented; bounded evidence is included by default, with an explicit compact opt-out
**Date:** 2026-11-23

## Problem

The `extract` result identifies each stored fact by ID and type, but does not
show the extracted statement or its supporting source quote unless explicitly
requested. An agent that must safely correct an existing fact therefore needs
another `assemble_context` call to verify the write before invalidating the
superseded fact. Maintenance feasibility v1–v3 exceeded the per-phase input
budget. V4's only complete invocation brought all eight candidate formation
phases under budget, but its candidate passed only 7/8 task cases, a required
repeat failed before model turns, and historical receipts cannot prove that
the flag was requested or evidence returned before invalidation.

## Contract

Add an optional `include_fact_evidence` boolean to the existing `extract`
request. It defaults to `true`; callers may explicitly set it to `false` to
request a compact response. When evidence is included, successful extracted
facts include:

- `statement`: the persisted fact content;
- `source_quote`: the source quote persisted with that fact.

The `ExtractResult.episode_id` binds those fact statements and quotes to the
episode processed by this extraction. These fields report what the extractor
stored; they do not assert that the statement is true, that reconciliation is
complete, or that the quote logically entails the statement.

For inline content, the episode is ingested before extraction. A partial
response, empty fact list, absent evidence field, absent episode ID, or failed
write must never be treated as correction verification. Existing idempotent
retry behavior and error status remain unchanged.

## Privacy and compatibility

- Evidence is returned only to the same authorized caller that requested
  extraction. Callers that need the compact response set
  `include_fact_evidence=false`; the CLI provides `--compact`.
- Do not include statement text or quotes in logs, traces, receipt metadata,
  errors, or guidance. Preserve existing redaction boundaries.
- Add `serde(default)` and omit absent response fields during serialization;
  old requests and stored task payloads remain readable. Existing consumers
  may ignore the additive response fields.
- When requested, include evidence only when every fact has a nonempty
  statement and quote and the complete serialized evidence addition fits the
  4 KiB wire budget. Otherwise omit all evidence pairs and set
  `fact_evidence_omitted=true`; never return a partial subset that could be
  mistaken for complete correction evidence. A complete response reports
  `fact_evidence_omitted=false`. When evidence is not requested, omit this
  status field and keep the compact response shape.
- This adds a parameter to the existing tool; it does not add an MCP tool or
  expand the eight-tool surface. Update the generated tool schema fingerprint
  and compatibility documentation for the parameter and bounded default response.
- CLI and non-agent callers receive bounded evidence by default and can opt out
  with `--compact`.
- `episode_heuristic` can persist the full episode as both statement and quote.
  The response policy therefore bounds the complete serialized evidence
  addition at 4 KiB and omits it atomically when it does not fit. A global
  default would still change source-text exposure and context consumption.

## Safe maintenance sequence

The revised agent flow keeps correction verification while avoiding the extra
context read:

1. Read the exact active fact and retain its `fact_id`.
2. Call inline `extract` with the registered correction source and
   `include_fact_evidence=true`.
3. Continue only when the extraction succeeded, returned an episode ID and
   nonempty fact evidence, and the extracted statement is consistent with the
   correction while `source_quote` supports it. Then invalidate only the
   previously read stale fact ID. Otherwise preserve the old fact and ask for
   clarification or retry the incomplete extraction.
4. The runner starts fresh read-only recall and captures an independent fixed
   probe from the same store.

The agent does not invalidate based only on the number or type of extracted
facts. The independent probe is an evaluator-owned post-write check; it does
not replace the pre-invalidation evidence check.

## Verification and acceptance

Use TDD at the public `extract` tool boundary:

- Omitted `include_fact_evidence` defaults to evidence in both request
  deserialization and the published MCP schema.
- Explicit `include_fact_evidence=false` and CLI `--compact` preserve compact
  output.
- Default response returns the exact persisted statement and source quote for
  inline and existing-episode extraction.
- Pending/partial extraction never reports verified fact evidence.
- Evidence addition stays within the serialized 4 KiB budget, including JSON
  escaping and conservative field overhead; oversized evidence is omitted as
  a complete pair with an explicit status.
- Ordinary structured events and human/JSON logs contain neither field, even
  when default evidence is enabled.
- MCP schema and deserialization preserve legacy requests while exposing the
  optional parameter.

Before the agent feasibility run, preserve the existing deterministic memory
decision and source-authority baselines. After implementation, rerun the
registered deterministic decision, retrieval, claims, source-authority and
independent-probe gates on the candidate binary. The 2026-11-23 rerun passed
claims 41/41, retrieval 37/37, source-authority v4 4/4 primary cases on two
matching captures, and the eight-case operator-import preflight; evidence
details and artifact hashes are in the
[regression-controls record](../../../evals/paired/extract-evidence-default-2026-11-23/regression-controls/README.md)
and [response-default report](../../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).
These fixed checks show no observed regression in their registered cases.
They do not estimate a treatment effect; the response-only change is not
expected to alter stored facts or recall metrics.

For the paired agent feasibility check, compare the frozen old binary and the
candidate binary on new disjoint synthetic histories. Both arms use the same
Codex model alias, host policy, tool-call limit and task oracle. The incumbent
uses a second context read because it lacks the opt-in evidence parameter; the
candidate skips that read only after checking the returned statement and
quote. Require complete registered repeats, one successful correction write
and exact invalidation per trial, correct task answers, passing independent
probes, zero unsupported/prohibited sources and hard violations, and all
candidate phases inside the 65,536-token budget. Report all arm results even
when the gate fails. This study measures this workflow's feasibility and
descriptive utility; it is not powered evidence of broad agent benefit or
population non-inferiority.

### Implementation and historical agent gate (2026-11-23)

The request parameter and response fields are implemented with evidence
included by default. The tool applies an all-or-none 4 KiB evidence
wire budget and reports when evidence was omitted; the independent
`eval-harness` contract test measures the actual serialized response delta.
Product tests cover compact and opt-in output, stored and inline extraction,
persisted-quote identity, pending extraction, oversized-source omission, and
log redaction. The v4 protocol and outcomes are recorded in the
[`agent-managed-maintenance-v4` report](../../../evals/paired/agent-managed-maintenance-v4/README.md).
Its preregistered gate did not pass: only one of two invocations completed,
the candidate scored 7/8, and the old capture format omitted normalized
request/response evidence telemetry. The evaluator now records those
content-free facts and preserves legacy artifacts as “not measured”.

At this checkpoint, the flag remained opt-in. The current bounded default
decision below supersedes that policy, but does not change the historical v4
result or imply that v4 measured this response contract.

### Bounded response and v5 diagnostic ruling (2026-11-23)

The implementation now applies a 4 KiB all-or-none serialized evidence cap,
returns `fact_evidence_omitted` when requested evidence cannot be supplied,
and records response size as numeric-only agent telemetry. Public-tool tests
cover compact compatibility, inline and stored extraction, exact persisted
quotes, pending extraction, oversized omission and log redaction. The
deterministic v9 replay on the current product passed, but it measures the
separate claim-reconciliation stage.

The new disjoint v5 task manifest produced one direct-host diagnostic after
the registered sandbox run failed before model turns. In that diagnostic,
candidate formation completed within budget in 7/8 cases versus 0/8 for the
compact incumbent; the candidate used 24 versus 32 formation tool calls.
However, final task success was 7/8 in both arms, one candidate phase still
exceeded the 65,536-token budget, and only one valid host repeat exists. All
8/8 candidate responses contained requested evidence before invalidation and
remained under the size cap; all 8/8 independent probes passed in both arms.
The registered two-repeat/all-candidate-within-budget gate is therefore not
met. At this historical checkpoint the flag remained opt-in; the later
bounded-response default decision below does not claim a memory-quality
improvement from v5.
Details are in the
[`agent-managed-maintenance-v5` diagnostic](../../../evals/paired/agent-managed-maintenance-v5/README.md).

### V6 disjoint repeated cohort ruling (2026-11-23)

V6 completed two preregistered direct-host captures on eight correction
families disjoint from v1–v5. Its two comparisons replayed byte-identically.
The candidate stayed under the 65,536-token input budget in 16/16 maintenance
phases, versus 0/16 for the incumbent; used 24 rather than 32 tool calls per
capture; returned evidence before invalidation in 16/16 candidate trials; and
passed all 16 candidate independent post-write probes. The largest serialized
extract response was 3,415 bytes, below the 4,096-byte cap. There were no hard
violations or unsupported/prohibited sources.

However, the frozen candidate-correctness criterion required 8/8 task success
on both captures. The candidate achieved 8/8 on run-01 and 7/8 on run-02, so
v6 is **NO-GO** for promoting the response workflow based on its agent result.
In the run-02 failure,
the independent post-write probe passed; this validates that fixed stored
correction but does not make the agent's end-to-end task correct. The receipt
records `direct_memory_success=false` and `task_oracle_passed=false`, but
intentionally retains only payload hashes, so the exact agent-query result and
final answer are unavailable for classifying the miss as retrieval or answer
formulation. Do not infer a product retrieval regression or attribute a more
specific cause from these receipts. The two captures repeat the same eight
families and are not sixteen independent observations. This workflow
feasibility result does not measure a treatment effect on stored-memory
quality, nor prove broad agent benefit or non-inferiority.

The historical v6 result did not meet its all-candidate-correct gate. That
remains a no-go for claiming agent-managed maintenance success or a measured
agent-utility gain. V6 is not a deterministic product-contract or memory
retrieval comparison: it tested one agent workflow, and its failure does not
show that the bounded response default regresses memory state. The default
claim-reconciliation `evidence` rollout is a separate feature and provides no
evidence for this response flag. Full methods, identity pins, setup failure,
run tables, comparison hashes and caveats are in the
[`agent-managed-maintenance-v6` report](../../../evals/paired/agent-managed-maintenance-v6/README.md).

### Default-on safety ruling (2026-11-23)

The omitted request field now defaults to `true` in deserialization and the
published tool schema. Evidence remains all-or-none under a 4 KiB serialized
addition budget; incomplete or oversized evidence is omitted with an explicit
status. `false` remains a supported compact mode. The CLI defaults to evidence
and exposes `--compact`. Evidence is not written into logs or telemetry.

The transition was checked with deterministic product paths: eight isolated
operator-import fixtures all imported and extracted successfully, returned
complete default evidence, and passed an independent authorized-source/value
probe (8/8); maximum evidence addition was 139 bytes and maximum serialized
result was 1,023 bytes. A frozen public retrieval probe was captured twice
with the pre-default and default-on binaries: every capture completed, used
one result, returned the same authorized source, and had the same normalized
result fingerprint. This narrow check is recorded in
[`extract-evidence-default`](../../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

These checks justify the bounded response behavior and no-regression claim for
the tested public product paths. They do not establish improved stored-memory
metrics, population-wide compatibility with strict third-party consumers, or
better end-to-end agent task success. The v6 all-correct no-go remains in force
for agent-workflow promotion. Callers with strict response-shape requirements
should set `include_fact_evidence=false` or use `--compact`.

## Scope

Do not change extraction decisions, claim reconciliation, retrieval ranking,
or the default claim-reconciliation evidence rollout stage. Do not add a new
MCP tool. Keep agent-managed writes opt-in. The default response evidence is
bounded to 4 KiB of serialized additions; a passing feasibility study would
support only its narrow workflow on the frozen model and fixtures, not
improvement for all agents or future models.
