# Current memory regression oracle contract

**Scope:** Resolve office-entry, similar-name and single-current without inventing
product semantics or rewriting frozen results. Local native Apple Silicon only.
**Basis:** ADR-0083, v10 corpus and the corrected baseline section of
[MEMORY_DECISION_BASELINE](../../evals/MEMORY_DECISION_BASELINE.md).

1. [x] Inspect registered queries, raw responses, expected assertions and roles,
   scoring/normalization and product temporal contract. Reproduce all three archived
   scope errors while preserving correct evidence and explicit snapshot results.
2. [x] Freeze a fresh v10 capture with current schemas on the unchanged product.
3. [x] RED: exercise public compare-current-memory CLI with archived v9. It must
   reject a role oracle without explicit temporal cutoffs; initially returned 0.
4. [x] GREEN: reuse the owning corpus validation for current replay loading.
   Preserve archived loading; do not manufacture roles, weaken assertions, change
   production memory or modify frozen corpus/raw receipts.
5. [x] Switch active profile, CLI controls and native gate to compatible v10.
   Validate baseline scope before starting product. Keep reason/dimension negative
   controls and independent evidence for the three archived cases.
6. [x] Run the complete native gate, harness tests, required workspace lint and fmt.
7. [x] Document exact finite-cohort results, preserve failures and commit verified work.

The three former failures are evaluation-contract defects. Passing the corrected
oracle proves these finite cases only; it does not demonstrate new agent utility,
semantic extraction performance or population-level quality improvement.

**Verification:** 904 harness tests passed, 22 ignored; native gate 8/8 with no regressions; strict workspace clippy and fmt passed. [Evidence](../../../evals/paired/current-contract-baseline-2026-11-26/README.md).
