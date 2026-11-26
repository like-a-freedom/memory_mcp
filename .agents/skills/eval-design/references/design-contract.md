# Design contract

Use for a full spec, a portable benchmark or an audit of what a score claims. Choose only fields that affect the requested decision; do not force a large schema on a small task.

## Decision and system boundary

State the decision and the claim it needs: choosing a model, improving retrieval, accepting a feature, comparing interfaces, opening a risky capability or monitoring deployed behavior. Identify the evaluated bundle and versions: model, prompts/policy, retrieval/corpus, orchestration, tools, environment and budgets where relevant.

Separate these questions:

| Claim | Suitable evidence | Frequent substitution error |
|---|---|---|
| Predictive/component quality | Held-out inputs and independently justified targets | Training fit treated as generalization |
| Answer/retrieval quality | Relevant evidence, correctness, coverage and abstention | Citation presence treated as factual support |
| Interface/application behavior | Real schemas, validation, errors and observable actions | Accepted request treated as achieved effect |
| Stateful task completion | Authoritative effects plus relevant temporal constraints | Final prose or a single final snapshot treated as full truth |
| Field value and reliability | Representative operational evidence with outcome follow-up | Curated benchmark score treated as production prevalence |

Do not invent unavailable observation channels or implementation guarantees. An explicitly fixed product, provider or benchmark may be evaluated as requested; portability means isolating those choices from reusable task semantics, not erasing the user's constraints.

## Reusable case contract

Record meaningful subsets of:

- Stable case/version, origin, root/source grouping, split and licensing/privacy constraints.
- Visible request or input, intended user outcome, target semantics and relevant time.
- Input data or initial world; environment prerequisites and controlled external events.
- Authority/allowed paths, material constraints and protected behavior if applicable.
- Desired outputs/effects, acceptable limited outcomes and disallowed outcomes.
- Independent labels/oracle, evidence requirements, semantic rubric and uncertainty states.
- Eligibility/capability requirements, budgets, termination and observation horizon.
- Links/digests to actual immutable artifacts; illustrative references are marked as such.

Hide answers and future test instances, not the requirements necessary to understand the task. A reference answer or trajectory is one valid solution unless exact form/order is itself part of the task. Require temporal order only where correctness or policy needs it.

For portability, separate:

1. **Domain case:** goals, constraints, input/world semantics and scoring.
2. **Participant adapter:** presents the request and captures the participant's native behavior.
3. **Environment/binding profile:** real capabilities, IDs, protocol and implementation semantics.
4. **Evidence adapter:** faithful mapping of observed facts; no filling in missing actions or fixing scope.
5. **Oracle/grader and reporter:** independent checks and transparent aggregation.

A simple classifier need not implement five software components. These are separable responsibilities, not a mandatory architecture or library dependency.

## Outcomes, uncertainty and denominators

Define a result model appropriate to the task. A useful pattern separates:

- task result: fulfilled, appropriate abstention/handoff, unresolved or unsupported;
- measurement: pass/fail, inconclusive, or invalid because of a confirmed evaluation defect;
- per-constraint observations, violations, evidence coverage and attribution confidence.

Classify at the level of the claim. Correctly reporting an unknown outcome can satisfy a decision/communication requirement without fulfilling the operational goal. Missing truth does not justify a fabricated conclusion. An unknown required safety predicate prevents a safety-pass claim, even if utility is observed. A known violation is not erased by another grader's failure.

Determine task eligibility before seeing results. Unexpected lack of an advertised required capability stays visible; a condition cannot improve its score by declaring failed cases unsupported afterward. Use separate coverage reporting for capabilities excluded in advance.

Maintain scheduled, evaluated, invalid, unknown and unsupported counts with reasons. Product failures and budget exhaustion are not automatically evaluator defects. Preserve originals and link reruns; a rerun policy cannot quietly select the best attempt. Different metrics may have different declared denominators, but none may hide inconvenient records.

## Acceptance and reporting

Choose limits from risk, baseline performance, operating costs and uncertainty; do not import numerical gates from another domain. A pilot may determine a justified policy before confirmatory testing. If evidence cannot establish the gate, report inconclusive rather than adjusting the gate after the result.

Report primary metrics and numerators/denominators, critical slices, unknowns/exclusions, uncertainty, costs, coverage and failure evidence. Separate exploratory findings from confirmatory decisions. Prevent a composite score from hiding an unacceptable failure dimension. State what the result does and does not establish.

For release programs, distinguish technical correctness, task utility, risk controls, backend/environment characterization and operational readiness. Passing one is not proof of all others. Name responsible decision/review functions without assuming separate teams or mandatory headcount.
