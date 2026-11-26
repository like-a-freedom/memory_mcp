# Agents, tools and stateful systems

Read for interactive agents, tool/API/MCP interfaces, multi-agent workflows or consequential actions. Apply permission/state machinery only where relevant; a read-only text task does not need a mutation protocol.

## Portable task, native interface

Define what the participant must accomplish, what information it can access, allowed effects, protected behavior, time/resource limits and sufficient evidence. Keep tool names, API paths, model SDKs and orchestration classes in binding/participant profiles. The participant should see the actual interface being evaluated.

An adapter may translate I/O and observed facts faithfully. It may not choose the right tool, repair arguments, infer a missing identity, silently execute omitted steps or reinterpret accepted as completed. Otherwise the experiment evaluates the adapter's assistance. Record substantive help as a separate condition and cost.

Accept different valid trajectories and native interfaces. Require milestones/order only where task semantics or policy needs them. No mandatory number of queries/edits or exact patch/trace unless that is an explicit requirement. Use a participant×binding matrix to isolate interface and harness effects; claims of portability need demonstrated reuse, not merely a diagram.

## Declare the full execution surface

Inventory native shell/browser/CLI/HTTP/other MCP tools, helper agents, mounts, credentials, network access and available external actors. Protect hidden truth, graders, reset/injection controls and privileged observer across every path, not only the evaluated tool API. Test the actual boundary with denied-access/integrity controls.

A domain task may legitimately allow CLI/API alternatives. Success through them can remain domain success, but does not establish the utility of a bypassed MCP. Interface comparisons fix allowed surfaces or report bypass/coverage. An unplanned bypass is a deviation/invalid interface comparison where appropriate, not automatically a harmful action. Proven unauthorized effects are policy failures; unobserved critical channels make the corresponding assurance inconclusive. Preserve scheduled records.

Do not request hidden chain of thought. Observable actions, cited evidence and sufficient decision rationale support diagnosis without requiring private reasoning.

## State and effect oracles

Separate prepared artifact, persisted artifact, binding/configuration, accepted operation, observed state and verified goal effect. An acknowledgement or healthy status does not establish causal completion.

Use authoritative inputs/effect ledgers and output/state observations. Specify identity, scope, provenance, clocks, visibility, latency, observation horizon and attribution. Missing logs or empty search are not proof that nothing happened. Negative results need coverage/delivery and completed processing horizons, not just an empty query.

Separate participant knowledge from oracle knowledge. An independently confirmed effect is not operationally unknown merely because the participant cannot observe it. Score actual fulfillment, the participant's evidential grounding and its communication separately. Requiring the participant to verify completion is a case-specific task constraint, not a universal success condition. Hidden oracle truth cannot retroactively justify an unsupported participant claim.

Before/after snapshots miss transient forbidden writes followed by reversal and some disclosures. Use trusted full-interval effects/egress evidence for constraints that require it; otherwise report the observation gap. Evaluate attempted violations separately from realized harm, including blocked attempts where the policy defines them as violations.

For pipelines, use discriminating fresh inputs and positive/negative/protected controls. Testing saved artifacts or replaying already transformed data may not exercise an earlier processing stage. Optional isolated ablations can identify contributions of multiple repairs; they are evaluator controls, not a forced participant trajectory. Minimal sufficient valid repairs pass.

Post-trial artifact tests must not apply an omitted action on behalf of the participant and count it as trial completion. Grade artifact semantics and actual trial effects separately. Do not expose hidden validation answers to justify an earlier ungrounded participant claim.

## Isolation and external actors

Reset all state that can affect the outcome: configurations/bindings, queues, caches, buckets, counters, lists, clocks, delayed outputs and preseeded state. Within-trial cohorts need disjoint semantic keys or a characterized reset/drain. Hidden state carryover can turn a negative example positive.

Model relevant concurrent operators, user actions and approvals with declared actor policies. They may change shared state; do not treat them as passive answer generators. External actors must not coach one condition toward the reference solution. LLM user simulators are optional and require validation of their own behavior.

Reference solutions and verifiers must fit the declared resource envelope. Distinguish agent active time, external wait, review and backend processing. Accelerating a logical clock in a simulator does not prove an equivalent real-time backend behavior.

For consequential writes, test review/authority conditions required by the task, stale targets/versions, partial effects, crash boundaries, resumption and ambiguous replies. Do not require universal preview fields, journals or idempotency-key syntax. Different implementations may satisfy the same invariant differently; verify guarantees rather than matching field names. Never let an eval retry an ambiguous mutation without a demonstrated safe mechanism and authorization.

## MCP-specific profile, when applicable

Pin the actual protocol edition/era, SDK/client/transport and supported capabilities. Check the owning official version: latest/draft pages are not proof of an implementation's conformance. Common legacy and current behaviors can differ.

Relevant conformance checks include:

- Actual discovery surface, schema bounds/defaults, tools/resources support and advertised visibility/list-change behavior.
- Edition-specific request metadata, result shapes and protocol-versus-tool error semantics.
- Real errors/envelopes/guidance mapped faithfully; annotations are hints, not grants of authority.
- Authority rechecked for stored handles where applicable; identity/ownership claims reflect actual authentication.
- Cancellation/timeouts/output limits, partial/completeness semantics, realistic broad filters and named-client behavior.

Flat schemas, fixed descriptor headings, tool-count caps, a particular auth model and proprietary envelope labels are local design choices, not universal MCP requirements. Inspect them when they belong to the evaluated product. Do not silently redesign transport/auth or claim OAuth/deployment certification from an offline eval.

Host-native discovery/programmatic composition and server-provided code execution are different factors. Compare the whole supported workflow, including cold/warm delivery cost and observable effects. Code Mode is optional; do not introduce a runtime or unrestricted egress merely to run a benchmark.

## Risk-enriched testing

When the threat model warrants it, use benign/attacked matched cases, insufficient-information tasks, boundary permissions and adversarial content in tool results, documents, errors or handoffs. Report legitimate utility, attack success and safety separately. Always-refuse is not successful defense on feasible authorized tasks.

State attacker powers, budget and opportunities; static prompt lists do not cover all future attacks. Test adaptive attempts only in an authorized isolated scope. Verify effects independently of agent or attacker self-report.
