# Maintenance and sources

Read for current/as-of claims, ongoing evaluation, benchmark repairs or maintenance of this skill. Source baseline reviewed **24 November 2026**. This is a dated synthesis, not a claim of exhaustive coverage or a continuously updated service.

## Refresh only the relevant evidence

Refresh when the user asks for latest practice, a protocol/provider/data version changes, a cited page conflicts with observed behavior, or a decision depends on a potentially stale claim. Use an available research tool; no particular CLI, provider or subscription is required. If retrieval is unavailable, identify the unverified current claim and continue with the usable evidence.

Prefer original papers, official specifications/documentation and first-party engineering reports. Distinguish normative requirements, empirical findings, practitioner recommendations and this skill's synthesis. A fresh preprint can reveal a failure mode without establishing a universal standard. Check the actual body, version and publication/submission date; retrieval date is not publication date, and a draft is not the stable release.

For material claims record the supporting URL, title/version, publication date if established, retrieval/review date, source status and what it supports. Keep a digest or archived artifact when reproduction needs it and access permits. A successful fetch or matching title does not establish that the source supports the claim. Resolve conflicts by applicability and evidence, not just recency. Do not silently upgrade an unknown into a fact.

## Maintain an evaluation program

- Version separately what can change the result: task/data split, labels and maturity, environment, system/model/prompt/index, adapter/harness, grader/rubric, budget and aggregation policy. Preserve run manifests and actual settings, including provider versions that cannot be pinned.
- Use production feedback where authorized. Report population sampling separately from incident/risk-enriched sampling; record selection probabilities when using them for population estimates. User ratings, task completion proxies and downstream labels need validation. Delayed or censored labels remain identifiable; a still-unobserved error is not a confirmed success.
- Diagnose a falling score before attribution: product regression, distribution shift, changed ground truth, a broken dependency and an incorrect verifier imply different actions. Track eval invalidity and reference feasibility as well as system performance. Repeated peeking at a frozen holdout turns it into development evidence; preserve new confirmatory evidence or use a justified sequential protocol.
- Review candidate tasks from real failures before promotion. Keep sensitive data minimized and authorized, deduplicate root episodes, qualify expected outcomes, and check known-good, shortcut and valid-alternative controls. Keep enough held-out material to avoid teaching both participant and grader the final test.
- Repair defective tasks or graders with explicit lineage. Preserve original results; determine whether saved evidence supports regrading or new execution is required. Bridge old/new versions on matched cases when comparability matters. Never backfill an improved score without identifying the changed measurement.
- Choose monitoring, review and refresh cadence from drift, consequence and label delay; there is no universal weekly/monthly cadence. Define who can revise truth, approve the gate and adjudicate disputes as functions, without assuming separate people.

An eval design alone does not authorize collecting new personal data, replaying production actions, exporting traces, scheduling jobs or making live changes. Distinguish observing already authorized work, an isolated replay, a shadow deployment and an interventional field experiment. Offline task success cannot by itself prove field impact; choose controlled rollout/randomization or an explicitly limited observational design for that claim.

## Maintain this skill

Treat observed misrouting or poor decisions as evidence for a focused change. First preserve a realistic request and minimal raw artifacts that reproduce the problem. Check whether the cause is the trigger description, missing guidance, inaccessible evidence or a task-specific constraint; do not turn every example into a universal rule.

Keep the entrypoint short, with one-level, conditional references. Test positive and plausible negative invocation cases, reference selection and a narrow request as well as a full design. Test substantially changed guidance with an independent consumer given the skill and raw request, without the intended answer or suspected fix. Review its actual output and resource reads; heading/string matches are not behavioral validation.

Re-run the host's skill validator and local-link checks after edits. Test any executable helpers if added. Remove stale duplication and unused resources after checking their callers. Change `knowledge-reviewed` only after reviewing the relevant evidence, and state the scope of that review; changing a date is not refreshing knowledge. Recheck affected examples after a substantive method change.

## Primary source ledger

All items below were reviewed or retrieved in the November 2026 research underlying this skill. Living documents have no asserted publication date. Dates are source dates where established, not a promise of future freshness. Use only the relevant rows for a given task.

| Source and status | Supports / limit |
|---|---|
| [scikit-learn: common pitfalls](https://scikit-learn.org/stable/common_pitfalls.html), official living documentation, retrieved 2026-11-24 | Leakage and train-only fitting of learned preprocessing/feature selection; independence of final testing. Does not prescribe one split for every deployment. |
| [scikit-learn: cross-validation](https://scikit-learn.org/stable/modules/cross_validation.html), official living documentation, retrieved 2026-11-24 | Tuning versus final testing; group/time-aware generalization. Fold choice depends on the claim and dependencies. |
| [scikit-learn: probability calibration](https://scikit-learn.org/stable/modules/calibration.html), official living documentation, retrieved 2026-11-24 | Calibration versus discrimination; held-out/out-of-fold calibration; Brier/log loss combine several properties. |
| [Anthropic: demystifying evals for AI agents](https://www.anthropic.com/research/demystifying-evals-for-ai-agents), first-party methodology, 2026-01-09 | Tasks, trials, graders and outcomes; repeated reliability; multiple legitimate solution paths. Agent methodology, not a requirement for all ML. |
| [OpenAI: evaluation best practices](https://platform.openai.com/docs/guides/evaluation-best-practices), official living guidance, reviewed 2026-11-24 | Task-specific criteria, grader calibration, comparative and continuous evaluation. Platform examples are optional implementations. |
| [NIST statistical handbook: binomial proportion intervals](https://www.itl.nist.gov/div898/handbook/prc/section2/prc241.htm), official reference, reviewed 2026-11-24 | Interval assumptions and small-sample cautions. A confidence bound is not proof of absence of risk. |
| [τ-bench](https://arxiv.org/html/2406.12045v1), primary paper, 2024-06 | Stateful outcomes and repeated-trial reliability. Its task distribution and measures do not automatically transfer to every domain. |
| [ToolSandbox](https://arxiv.org/html/2408.04682v1), primary paper, 2024-08 | Stateful interaction, milestones and constraints rather than an exact golden trace. |
| [Hugging Face: Agentic Evals](https://huggingface.co/blog/agentic-evals), first-party engineering guidance, 2026-11-23 | Reference feasibility, verifier controls, accepting valid alternatives and maintaining environments. Practitioner evidence, not a standard. |
| [Benchmarks as Software](https://arxiv.org/html/2611.08417v1), primary preprint, submitted 2026-11-05 | Dependency drift, verifier defects and versioned benchmark repair. Fresh research, not settled consensus or a source of universal thresholds. |
| [MCP tools specification, 2026-07-28](https://modelcontextprotocol.io/specification/2026-07-28/server/tools) and [authorization](https://modelcontextprotocol.io/specification/2026-07-28/basic/authorization), official stable snapshot checked 2026-11-24 | Conditional interface conformance for the negotiated era. Separate normative text, non-normative advice and local policy; refresh stable status when needed. |
| [Anthropic: skill authoring best practices](https://platform.claude.com/docs/en/agents-and-tools/agent-skills/best-practices), official living guidance, retrieved 2026-11-24 | Concision, appropriate freedom, progressive disclosure and testing a skill with a separate consumer. Host-specific skill contracts still govern packaging. |

The statistical planning rules and portable contract in the other references are a synthesis, not a quotation or a new standard. These sources do not establish a universal sample size, score threshold, retry budget, tool taxonomy or required eval platform.
