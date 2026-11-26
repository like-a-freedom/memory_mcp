# Memory quality evidence closure implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development. Track each completed vertical TDD slice below.

**Goal:** Close the five approved audit actions and publish trustworthy scoped metrics, causal interpretation and a concrete memory-quality action plan.
**Architecture:** Correct existing graders at public CLI/EvalSuite seams; use immutable raw receipts and bounded local experiments to distinguish formation, retrieval, packaging and agent-use failures. No model-backed metric substitutes for deterministic truth.
**Tech Stack:** Rust, Python standard library, public MCP, native Apple Silicon; optional provider-neutral loopback model only for a qualified bounded diagnostic.
**Spec:** Approved five-item audit action list in this session; existing truthful-evaluation and agent-memory-follow-up specifications.

## Global constraints

- Work on master as explicitly authorized. Preserve unrelated untracked skills and historical raw receipts. No dependency changes or production repair/deployment.
- Evals only locally on native Apple Silicon. No LM Studio requirement or remote provider calls.
- Existing CLI, EvalSuite, public memory operations and runner seams are approved by the user's execution instruction. No private-helper tests.
- A deterministic bad value or changed repeated answer must fail. Missing evidence remains unknown/invalid, never success.
- Preserve development/confirmation boundaries and prior C1 stop rule. A confirmation study is conditional on independently qualified workflow; report an explicit eligibility decision either way.

## Review focus

- A numeric substring must not establish the expected value; correct full assertions remain valid.
- Every registered repeated read must retain assertions, evidence, cutoffs and stable semantic content, independent of prose guidance.
- Closed entity/ warning expectations must reject extras; metadata must not depend on failure stage.
- Missing intermediate observations cannot be reported as an identified causal stage; diagnostic failures and deadlines stay visible.
- Fixed replies/registered requests establish feasibility, not autonomous agent benefit. No selection of a successful retry.

## Tasks and completion ledger

1. [x] Grade assertions exactly at the declared assertion boundary. Public CLI negative numeric-prefix and negation cases RED, minimal grader fix GREEN; retain raw replay compatibility and correct-value controls.
2. [x] Reject changed repeated responses. Public CLI second-read mutation RED; grade every read and compare decision-relevant stable content GREEN. Test harmless guidance changes separately.
3. [x] Clarify extraction corpus closed-world contract, entity/warning false positives, label provenance and stable population metadata. Public suite TDD; report actual failures rather than changing expected labels to outputs.
4. [x] Diagnose frozen long-history failures by formation/retrieval/packaging/reader stages, with input hashes and explicit unknowns; run a bounded current public-path comparator where it can isolate a cause. Preserve stage latency and timeout evidence.
5. [x] Freeze a fresh small local agent diagnostic comparing registered query, selected query and sufficient fixed reply. Score exact values, sources and false-premise behavior independently; preserve original C1 and decide C2 eligibility before any larger confirmation.
6. [x] Regrade accepted current evidence and adversarial controls, run one native current gate and focused/full harness checks, required strict workspace clippy and fmt. One independent final review and fixes.
7. [x] Publish a consolidated evidence report with denominators, identities, uncertainty, completed actions and a prioritized implementation action plan for memory quality. Update runbook and commit verified changes.

## Execution rulings

- The user explicitly requested implementation of all five approved actions; no additional plan handoff is required. Failure of qualification closes the conditional confirmation step as NO-GO, not as a positive quality claim.
- Parallel ownership is limited to independent files; no per-task reviewer churn. A fresh final review covers the combined result.

- Request-axis ruling: a historical oracle must not grade auxiliary latest-knowledge reads. Each primary read is evidence-scored when primary is the registered scoring request; explicit snapshots use their snapshot oracle. All repeated primary reads are still compared for stable semantic content. Applying the historical oracle to latest reads was rejected by existing valid-boundary controls.
- Extraction public file-input seam: `ExtractionSuite::from_fixture_path(PathBuf)` selects project-reviewed closed expectations. Existing labels were not changed to match predictions.

## Observed execution evidence

- Tasks 1–2: numeric prefix, negation, slash-unit, changed second-read controls each failed on the prior behavior, then passed after the grader change. Independent review reproduced textual `per day` and ASCII contraction bypasses; those and curly-apostrophe negation now have observed RED → GREEN controls. The public current CLI target passes 18/18, including guidance-only success and an honestly recorded changed read classified as quality-failed rather than invalid.
- Task 3: public suite contract 7/7; native extraction 4/9 passed, 5 quality-failed, 0 invalid; precision .60, recall 1.00, F1 .75. This is a corrected verdict, not an extraction repair. See `docs/evals/EXTRACTION_CLOSED_WORLD_2026-11-26.md`.
- Task 4: frozen strict answers 2/7, complete spans 2/7, one isolated reader failure; earlier missing stages remain unknown. Current debug miniature preserves the full span in both reads, total 7.901 s. LongMem Python controls: 45 passed, 5 explicit historical skips. See `evals/longmemeval_v2/results/2026-11-26-long-history-diagnosis/DIAGNOSIS.md`.
- Task 5: fresh three-family, three-arm diagnostic passed 9/9 in each of two fresh memory captures (18/18 value/source/abstention), 117.060 s observed; no model failure or timeout. Model server/prompt cache shared, fixed arm order; this is not independent statistical confirmation. C2 remains NO-GO because autonomous treatment qualification is not established. The earlier C1 was not repeated. Reusable CLI: 8/8, including startup denominator, opaque model ID and capture-retention controls. The exact captured source remains hash-bound separately from later engineering changes.
- Task 6 final verification: full eval-harness 920 passed, 22 ignored, no failures. Native gate separately passed in 54.90 s; final offline regrade after review fixes passes 8/8 with no regressions. Required strict workspace clippy, fmt and whitespace checks passed. Independent final review has no remaining Important/Critical findings; the deadline wording now distinguishes model eligibility/I/O timeouts from a whole-capture wall bound.
- Task 7: `docs/evals/MEMORY_QUALITY_CLOSURE_2026-11-26.md` publishes scoped metrics, causal unknowns, C2 eligibility and six concrete ordered repair steps. The eval runbook links it; `evals/paired/memory-quality-closure-2026-11-26/verification.json` records final checks and identities. This plan closes the five audit actions, not future product repairs or a claim of universal agent benefit.
