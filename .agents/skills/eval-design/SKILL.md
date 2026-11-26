---
name: eval-design
description: "Design and review evaluations of ML/LLM systems, including retrieval and tool-using agents. Use for eval specifications, benchmark audits, scoring and comparison plans; not routine software unit-test authoring."
metadata:
  knowledge-reviewed: "2026-11-24"
---

# Eval Design

Turn a decision about an AI system into an evaluation whose evidence can support that decision. Preserve the user's domain, implementation choices and requested scope. Scale the work: one useful case can be enough for a narrow review; a release program needs a fuller contract.

## Start with the claim

Identify the decision, evaluated system and boundary, target tasks/population, cost of mistakes, available truth and requested deliverable. Separate confirmed requirements, observed evidence and proposed choices. Infer routine details; ask only for missing information that changes the design, and continue independent work.

Distinguish model/component quality, application behavior, interface conformance and real-world outcomes. A correct prediction, plausible answer, successful API call and achieved user goal support different claims. Evaluate the layers needed for this decision rather than silently substituting one for another.

## Load detail only when needed

| Task | Read |
|---|---|
| Full specification, portable tasks, verdicts or acceptance policy | [Design contract](references/design-contract.md) |
| Datasets, labels, retrieval/generation scoring or judge validation | [Data and graders](references/data-and-graders.md) |
| Sample size, uncertainty, model comparisons, repeated trials or release thresholds | [Statistics and comparisons](references/statistics-and-comparisons.md) |
| Interactive/tool-using agents, stateful effects, MCP or cross-harness evaluation | [Agents and tools](references/agents-and-tools.md) |
| Latest/as-of claims, benchmark maintenance or production evaluation | [Maintenance and sources](references/maintenance-and-sources.md) |
| A concrete worked case is needed | Relevant example in [Worked designs](references/worked-designs.md) |

Do not load all references by default. Follow the relevant branch; preserve existing valid project contracts instead of imposing these examples as a replacement framework.

## Design around observable evidence

- Define the unit of work, initial conditions or input distribution, desired result, material constraints and evidence sufficient to score it. Keep domain goals independent of model providers, frameworks and tool names; place implementation specifics in replaceable profiles/adapters.
- Test useful completion and justified abstention separately. Known bad results, missing observations and defective measurement are different conditions. Declare denominators and exclusions before inspecting the comparison.
- Use reliable labels or independent outcome checks where available. Reserve semantic judgment for what requires it; an LLM judge is optional and needs task-specific validation. The system's own success report is evidence to check, not the outcome oracle.
- Include controls that expose easy score inflation and valid alternative solutions that expose overly narrow graders. Where harms matter, score utility, attempted violations and realized effects separately; a final-state snapshot may miss transient harm.
- Keep development, selection/calibration and confirmatory evidence distinct. Choose sample size, repetitions, thresholds and stopping rules for the intended claim; examples in this skill are not universal gates.
- Compare matched tasks and conditions, preserve failed/unknown/invalid records, and report uncertainty at the independent sampling unit. Attribute differences only to factors the experiment actually isolates.

## Deliver and check

For a design, provide enough detail to implement the relevant tasks, evidence paths, scoring, comparisons and decision rules. Include a concrete case, declared assumptions, unverified capabilities and the next validation needed. For an audit, prioritize defects that could change the conclusion and propose the smallest supported correction.

Before calling the design ready, check that a known-good solution can satisfy it, a plausible shortcut cannot pass, missing evidence cannot become success, and another valid method is accepted. State which checks were actually run; a written test plan is not an executed evaluation. Designing evals does not authorize live actions, deployment, data export or paid runs beyond the user's scope.

Knowledge baseline: **November 2026**. For current/as-of claims, refresh relevant primary sources using an available research tool and record the source/version/date. Do not present this saved baseline as a fresh check or promise exhaustive coverage of all best practices. See the maintenance reference for update triggers and evidence standards.
