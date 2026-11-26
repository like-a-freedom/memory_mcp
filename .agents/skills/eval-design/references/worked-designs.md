# Worked designs

Illustrative designs, not executed evaluations or prescribed targets. Read the relevant example only. Contents: **1 — predictive ML**, **2 — retrieval-augmented answers**, **3 — stateful agents across implementations**. Translate the same questions to the user's actual domain; do not copy the example's apparatus when unnecessary.

## 1. Predictive ML: prioritize equipment inspection

**Decision.** Compare two failure-risk models for deciding which equipment to inspect next week under a fixed inspection capacity. The unit is an equipment-week; target is a qualifying failure in the following week. Multiple weeks belong to the same device/site, and inspection itself may affect the eventual label.

**Case.** At Monday 08:00, score the permitted historical sensor and maintenance data for device D. A repair ticket written on Wednesday is unavailable at scoring time. Declare the target window and when failure labels become mature. An immature label remains pending, not a negative example.

**Design.** Choose the split for the deployment claim: future weeks at known sites, new sites, or both need different group/time boundaries. Fit imputers, feature selection, any resampling, probability calibration and inspection thresholds within permitted training/selection data. Compare the locked systems on the same eligible test windows, preserving grouped dependence. Include an incumbent/simple baseline with the same information access. A shuffled row split or future ticket text is not an independent test of future performance.

**Evidence and scoring.** Independent maintenance/failure records establish labels, with disputed or intervention-affected records identifiable. Report ranking and precision/recall at the inspection capacity, calibration with support if probabilities matter, and the explicit missed-failure/inspection costs. Lower Brier alone does not establish better calibration. Risk-enriched test cases expose rare faults; they cannot supply field precision without a justified prevalence/weighting model.

**Decision rule.** Choose sample size and practical improvement/non-inferiority margins from operational costs and independent sites/weeks. Predeclare how pending labels and critical slices affect the decision. An offline ranking gain supports the model comparison; proving reduced downtime requires a suitable field design that handles intervention effects.

**Controls.** Check time availability and group leakage, constant/majority and incumbent predictions, labels aligned to the wrong week, and a reference calculation on a small independently verified subset. No interactive agent, tool-calling protocol or LLM judge is needed.

## 2. RAG: answer a service-plan eligibility question

**Decision.** Compare two retrieval/answer configurations for correctly answering customer questions from a versioned policy corpus within a declared latency/cost budget.

**Case.** The customer asks whether a battery replacement is covered for plan P, purchased on date T in region R. Provide the required customer facts and the authorized corpus as of the decision time. The corpus includes current and superseded policies; some test variants deliberately omit the decisive clause or contain unresolved conflicts.

**Evidence and scoring.** Domain reviewers establish applicable clauses and essential conclusions with document/version provenance. Score retrieval of applicable evidence separately from answer correctness, citation support and essential-information coverage. A fluent answer with an obsolete citation fails the relevant correctness requirement. Returning no claims is not perfect factuality: use not-applicable precision plus a failed coverage/result predicate. Appropriate uncertainty or a specific clarification can satisfy an underdetermined case; it does not count as a definitive coverage decision.

**Variants and controls.** Keep paraphrases and related policy documents in one split/root family. Include answerable, genuinely unanswerable, conflicting-version and misleading-near-match cases. A known-good answer with different wording must pass; a copied citation with an unsupported conclusion must fail. Validate any semantic judge against withheld reviewer labels and inspect false passes/failures; deterministic document/version checks remain separate. Do not add an LLM judge when qualified labels and simple checks suffice.

**Comparison.** For an overall product comparison, each system may use its declared corpus/index and report that difference. To isolate retrieval or generation, hold the other component fixed. Compare matched requests and aggregate at independent question/policy families. Predeclare the practical quality margin, critical mistakes, missing-evidence treatment and resource gates before the final run. Include the full inference/retrieval cost; report grader cost separately.

**Result limitation.** An independent offline corpus can establish bounded answer quality. It does not establish the prevalence of those cases in live support or that customers will resolve their actual issue faster.

## 3. Stateful agents: reschedule an appointment

**Decision.** Compare different agents and different scheduling implementations on the same user job, while separately measuring interface conformance and agent task utility.

**Visible request.** “Move my appointment to the earliest available slot after 17:00 next week at the same location. Keep my existing appointment if that cannot be done. Do not create a second booking.” Provide the user's relevant identity, week/time zone and declared authority. If the product requires a review/approval, the case specifies an external actor response; it is not supplied ad hoc by an evaluator.

**World.** An initial appointment and slot inventory, controlled competing reservations and a known observation horizon. Some cases offer a valid slot, some offer none, and one changes availability during execution. A fault variant loses a response after a write; the environment records whether the write actually happened.

**Invariant task core.** For a feasible case, the independently observed booking belongs to the correct customer, location and permitted earliest slot, the old booking is replaced, and duplicate bookings never occur over the observed transaction interval. The no-slot case requires preserving the original booking and accurately reporting the limitation. A transient duplicate followed by cleanup violates the stated constraint even if the final snapshot looks correct. Where atomic replacement is required but absent, record unsupported coverage in advance rather than silently weakening this case.

**Replaceable profiles.** Implementation A might offer one reschedule operation; B might offer a hold/commit mechanism. Agent X may use MCP discovery directly; Y may use a host's programmatic tool interface. Bind fixture semantics, native requests/results and evidence capture without choosing the right slot or repairing participant mistakes. Use each system's authentic tool names and schemas; do not require the same tool-call order. State capability differences and report common-scope comparisons separately from coverage.

**Oracle and evidence.** The hidden ledger and independently controlled booking observer establish initial, intermediate and final facts. An accepted request, agent completion statement or generated confirmation email is not the booking oracle. If independent observations cannot resolve completion, distinguish that unknown from observed failure. If the ledger proves completion but the participant cannot observe it, record fulfilled effects separately from its knowledge and communication; do not impose an unrequested participant-verification requirement. Reconcile through supported reads; a measurement grader must not repeat the booking action to discover the answer. Reset bookings, holds, locks, caches and clock between trials.

**Execution boundary.** Inventory participant-visible shell/browser/REST paths and credentials. An allowed direct REST action may satisfy the domain goal; it does not demonstrate MCP utility. For a comparison specifically attributing success to MCP, prevent or separately classify alternate execution paths through actual isolation/observation. Keep solution labels and verifier controls inaccessible. Native adapters may translate protocol syntax but may not solve the case for the agent.

**Controls.** Verify a reference implementation can finish, a success-only message cannot pass, an unchanged booking cannot pass a feasible case, a duplicate-producing shortcut fails, an alternative valid transaction path passes, and missing observer data cannot become a safety pass. In a no-slot case, leaving state unchanged can be the correct result.

**Comparison and release decision.** Run matched isolated cases with declared authority, native harness versions and full budgets including discovery and assistance. Distinguish task failures, violated constraints, unresolved effects and confirmed harness defects; preserve scheduled denominators and rerun lineage. Choose repeats from the reliability question, cluster related variants, and preregister critical constraints plus practical utility/resource margins. A protected invariant checked in a finite fixture suite supports only that scope, not guaranteed absence of all booking errors.
