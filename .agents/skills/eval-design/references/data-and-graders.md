# Data and graders

Read for dataset construction, reliable labels, retrieval/generation criteria and semantic judges. Design only the measures that support the intended decision.

## Select tasks and targets

Use real task evidence where authorized; supplement rare or expensive cases with explicit synthetic assumptions. Keep realistic prevalence samples distinct from risk-enriched challenge sets. Synthetic truth is useful for causal checks, but does not establish field realism or deployment prevalence.

Split at the leakage boundary: person, organization, session, document/source, incident/root episode, near-duplicate family, generator or time as appropriate. Paraphrases and repeated runs do not create independent cases. For forecasting or changing systems, preserve temporal availability: a feature, retrieval document or label created later must not leak into an earlier decision.

Fit preprocessing, feature selection, resampling, threshold selection and calibration on the permitted training/development portions only. Keep confirmatory data out of those decisions. Group/time-aware splits can be more appropriate than a random row split. If a split is too small for the claim, expand evidence or narrow the claim rather than calling related variants independent.

Specify who labels, what evidence is available, how disagreement/ambiguity is represented and how labels mature. Annotation agreement measures consistency, not truth. Maintain contested items and correction lineage; do not delete hard cases silently. For online-derived labels, track delay, censoring and selective review.

## Match metrics to behavior

| System | Relevant choices | Trap to expose |
|---|---|---|
| Classifier/ranker | Operating-point precision/recall, ranking utility, calibrated probabilities, costs and subgroup coverage | Accuracy hides rare-class failure; threshold tuned on test |
| Regressor/forecaster | Error units, asymmetric loss, horizon, baseline, prediction-interval coverage/width | Averaging across horizons/entities hides important error; future leakage |
| Retriever/RAG | Retrieval relevance/coverage, answer correctness, grounded claims, required-answer coverage, citation support and abstention | More citations or lexical overlap treated as grounded correctness |
| Generator/extractor | Task-specific semantic or structural constraints; alternatives and uncertainty | Exact reference text rejects valid answers; fluent unsupported answer passes |
| Interactive agent | Actual goal/effects, applicable policy constraints and evidence | Plausible final message or cheap trajectory treated as task success |

For RAG, evaluate retrieval and final answers separately when diagnosing causes, then test the whole workflow. Do not claim that better retrieval necessarily improves user outcomes. Missing documents, unanswerable requests and conflicting/stale sources need declared expected behavior.

For probabilistic predictions, distinguish discrimination/ranking, calibration and decision utility. Brier/log loss combine several properties; a lower value alone does not establish better calibration. Use calibration evidence with bin/support counts where relevant, and fit calibrators on held-out or out-of-fold predictions rather than in-sample base-model outputs.

Report both supported-claim precision and coverage of task-required conclusions. A silent or always-abstaining system must not get perfect grounding from making zero claims: mark that precision not applicable, and score task coverage/utility separately. Correct refusal can be a positive outcome on designated cases without masking unnecessary refusal on feasible cases.

## Select the oracle

Prefer trustworthy labels, independent environment/artifact checks or deterministic constraints for facts they can establish. Semantic grading fills the remaining need; it need not be an LLM. Do not treat textual equality as meaning unless exact representation matters.

For executable results, compare behavior and required constraints, not only syntax or successful execution. For state changes, use authoritative observations rather than the system's own success message. A grader must not repeat an operational action merely to find out what happened.

## Validate semantic judges

Use atomic, evidence-based dimensions with anchored pass/fail/uncertain examples. Separate factual correctness, completeness, policy constraints and style; style should not compensate for a material error.

If an LLM judge is useful:

- Calibrate against independent expert labels on cases spanning successes, failures, boundaries and expected abstentions. Reserve a separate judge holdout at the root/source-group boundary.
- Blind model/condition identity when practical. Examine order, length, formatting, self-preference and model-family biases; include polished wrong answers and concise correct alternatives.
- Report confusion and class-specific false accepts/rejects, uncertainty, class counts and human-human disagreements. High aggregate agreement on an imbalanced panel is insufficient.
- Choose tolerable judge errors for the actual decision. No fixed agreement percentage or panel size is universally sufficient. Use expert review or declare inconclusive when calibration cannot support the claim.
- Version rubric, judge/model/settings and evidence bundle. Regrade a bridge panel when changing them, while permitted evidence still exists.

Untrusted content in answers/documents/tool results remains data, not grader instructions. Limit grader permissions and data exposure. A grader parse/error is measurement failure or a grading retry over the same immutable evidence, not a reason to rerun a consequential task. Do not let a semantic judge alone certify a high-consequence fact that needs authoritative verification.

## Test the measurement system

Use controls appropriate to the task:

- A known-good/reference solution is accepted; valid alternatives are also accepted.
- Known-bad, plausible shortcuts and fabricated completion are rejected.
- No-op/always-abstain/always-act reveal degeneracy across suitable positive and negative cases. No-op may be correct for a specific task; check whether it satisfies that task's actual requirements.
- Perturb one relevant fact, omit required evidence or add a forbidden effect; the owning check responds.
- Grading preserves immutable input/reference/world digests. Deterministic regrading is repeatable; stochastic semantic variation is measured.
- Broken labels, corrupted artifacts, reset failures and grader exceptions are surfaced explicitly.

Do not accept a benchmark solely because its reference solution passes: a narrow verifier can still reject valid methods or accept shortcuts. Independent expert/adversarial review is valuable when those defects could change the decision.
